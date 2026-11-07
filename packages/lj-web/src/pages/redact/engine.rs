//! Everything the workbench does to the document: load it, draw it, locate the
//! named strings, remove them, hand the result back.
//!
//! A PDF and a word-processing document go through the redaction module,
//! fetched only when a document needs it, which draws their pages as SVG. An
//! image is not rewritten by it: the browser already decodes and re-encodes
//! pixels, so a rectangle is filled on a canvas and the file written back from
//! it. Only its reading borrows the module, which prepares every page for the
//! text reader.
#![cfg(feature = "hydrate")]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::bridge;
use super::state::{
    Check, Eye, Format, Hit, Hits, Needle, Offline, PageWord, Reading, Rect, Session, Status,
    TextBlock, Verdict, View,
};

thread_local! {
    static MODULE: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    /// Font files already fetched, by name, the families already declared to
    /// the browser or on their way, and the CMaps handed to the module: the
    /// next document, and the reload of a redacted one, reuse them.
    static FONTS: RefCell<HashMap<String, js_sys::Uint8Array>> = RefCell::new(HashMap::new());
    static DECLARED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static CMAPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static DOCUMENT: RefCell<Option<bridge::Document>> = const { RefCell::new(None) };
    static PICTURE: RefCell<Option<web_sys::ImageBitmap>> = const { RefCell::new(None) };
    static PRODUCED: RefCell<Option<(String, Vec<u8>)>> = const { RefCell::new(None) };
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
    /// Where the tool's files come from, when the page's own origin serves none.
    static FILES: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn fetch_files_from(origin: String) {
    FILES.with(|slot| *slot.borrow_mut() = Some(origin));
}

fn file_url(path: &str) -> String {
    FILES.with(|slot| match &*slot.borrow() {
        Some(origin) => format!("{origin}{path}"),
        None => path.to_string(),
    })
}

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

fn document() -> Option<web_sys::Document> {
    window()?.document()
}

/// Append the loader that poses `global`, unless it already ran.
fn load(global: &str, src: &str) {
    let Some(window) = window() else { return };
    if js_sys::Reflect::has(&window, &JsValue::from_str(global)).unwrap_or(false) {
        return;
    }
    let Some(document) = document() else { return };
    let Ok(script) = document.create_element("script") else {
        return;
    };
    let _ = script.set_attribute("type", "module");
    let _ = script.set_attribute("src", &file_url(src));
    if let Some(root) = document.document_element() {
        let _ = root.append_child(&script);
    }
}

async fn module_ready() -> Result<JsValue, String> {
    if let Some(ready) = MODULE.with(|slot| slot.borrow().clone()) {
        return Ok(ready);
    }
    let global = "ljRedactReady";
    load(global, "/redact.js");
    let ready = async {
        posed(global).await?;
        bridge::module(global).await
    }
    .await
    .map_err(|why| format!("le module d'anonymisation n'a pas pu être chargé ({why})"))?;
    MODULE.with(|slot| *slot.borrow_mut() = Some(ready.clone()));
    Ok(ready)
}

async fn pause(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// Hand the frame back, so a signal set just before is painted before the work
/// that follows freezes the thread.
async fn breathe() {
    pause(0).await;
}

/// A loader is a `<script>` appended a moment ago, and the promise it poses only
/// exists once the browser has run it.
async fn posed(name: &str) -> Result<(), String> {
    for _ in 0..600 {
        let Some(window) = window() else {
            return Err("pas de fenêtre".into());
        };
        if js_sys::Reflect::has(&window, &JsValue::from_str(name)).unwrap_or(false) {
            return Ok(());
        }
        pause(50).await;
    }
    Err("le chargeur du module n'a pas répondu".into())
}

async fn bytes_of(blob: &web_sys::Blob) -> Result<Vec<u8>, String> {
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(|_| "le fichier n'a pas pu être lu".to_string())?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

fn blob_of(bytes: &[u8], mime: &str) -> Result<web_sys::Blob, String> {
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)
        .map_err(|_| "le fichier n'a pas pu être assemblé".to_string())
}

fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"%PDF") {
        return Some(Format::Pdf);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // An OpenDocument package opens on a stored `mimetype` entry naming it.
        let open = bytes.get(30..38) == Some(b"mimetype")
            && bytes
                .get(38..)
                .is_some_and(|rest| rest.starts_with(b"application/vnd.oasis.opendocument"));
        return Some(match open {
            true => Format::OpenDocument,
            false => Format::Word,
        });
    }
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) || web_page(bytes) {
        return Some(Format::Word);
    }
    let webp = bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP";
    if bytes.starts_with(b"\x89PNG") || bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || webp {
        return Some(Format::Image);
    }
    None
}

/// A web page, as Word saves one and often names `.doc`.
fn web_page(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let head = match head {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(
            &rest
                .chunks_exact(2)
                .map(|u| u16::from_le_bytes([u[0], u[1]]))
                .collect::<Vec<_>>(),
        ),
        [0xFE, 0xFF, rest @ ..] => String::from_utf16_lossy(
            &rest
                .chunks_exact(2)
                .map(|u| u16::from_be_bytes([u[0], u[1]]))
                .collect::<Vec<_>>(),
        ),
        _ => String::from_utf8_lossy(head).into_owned(),
    };
    let head = head
        .trim_start_matches('\u{FEFF}')
        .trim_start()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html")
        || head.starts_with("<html")
        || (head.starts_with('<') && (head.contains("<html") || head.contains("<body")))
}

fn produced_name(name: &str, format: Format) -> String {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, extension),
        _ => (name, ""),
    };
    match format {
        Format::Image => format!("{stem}-anonymise.png"),
        _ if extension.is_empty() => format!("{stem}-anonymise"),
        _ => format!("{stem}-anonymise.{extension}"),
    }
}

/// Open the file picker the workbench keeps hidden.
pub fn pick() {
    if let Some(input) = document()
        .and_then(|d| d.get_element_by_id("redact-file"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.click();
    }
}

/// Take the file the user chose, whatever its kind. Reloading the document the
/// tool just produced goes through `ingest` instead, which keeps the list, the
/// scroll and the set of pages on screen. A file chosen while another is being
/// opened, read or rewritten is ignored: the two would write the same session.
pub fn adopt(session: Session, file: web_sys::File) {
    if session.status.get_untracked() == Status::Loading || session.busy.get_untracked() {
        return;
    }
    let name = file.name();
    session.status.set(Status::Loading);
    session.verdict.set(None);
    session.rects.set(Vec::new());
    session.needles.set(Vec::new());
    session.labels.set(HashMap::new());
    session.applied.set_value(HashMap::new());
    session.pass.set_value(0);
    session.hits.set(Hits::default());
    session.focus.set(None);
    session.visible.set(vec![0]);
    session.page.set(0);
    if let Some(viewport) = document().and_then(|d| d.get_element_by_id("redact-viewport")) {
        viewport.set_scroll_top(0);
    }
    PRODUCED.with(|slot| *slot.borrow_mut() = None);
    leptos::task::spawn_local(async move {
        let outcome = match bytes_of(&file).await {
            Ok(bytes) => {
                breathe().await;
                ingest(session, &name, bytes, true).await
            }
            Err(why) => Err(why),
        };
        match outcome {
            Ok(()) => session.status.set(Status::Ready),
            Err(why) => session.status.set(Status::Failed(why)),
        }
    });
}

/// Open `bytes` and fill the workbench with what they yield. `propose` is false
/// when reloading what the tool just produced: the list is the user's by then,
/// and the rules have nothing left to say about it.
async fn ingest(session: Session, name: &str, bytes: Vec<u8>, propose: bool) -> Result<(), String> {
    let format = sniff(&bytes)
        .ok_or("seuls les PDF, les documents Word ou OpenDocument et les images s'ouvrent ici")?;
    session.name.set(name.to_string());
    session.format.set(format);
    session.read_by_eye.set(false);

    match format {
        Format::Image => {
            session.characters.set(0);
            session.to_read.set(vec![0]);
            session.blocks.set(Vec::new());
            session.view.set(View::Pages);
            open_image(session, &bytes).await?;
        }
        _ => open_document(session, bytes, format, propose).await?,
    }
    session.generation.update(|n| *n += 1);
    Ok(())
}

async fn open_image(session: Session, bytes: &[u8]) -> Result<(), String> {
    let blob = blob_of(bytes, "application/octet-stream")?;
    let window = window().ok_or("pas de fenêtre")?;
    let refused = || "cette image n'a pas pu être décodée".to_string();
    let promise = window
        .create_image_bitmap_with_blob(&blob)
        .map_err(|_| refused())?;
    let bitmap: web_sys::ImageBitmap = JsFuture::from(promise)
        .await
        .map_err(|_| refused())?
        .dyn_into()
        .map_err(|_| refused())?;
    session
        .pages
        .set(vec![(bitmap.width() as f64, bitmap.height() as f64)]);
    PICTURE.with(|slot| *slot.borrow_mut() = Some(bitmap));
    DOCUMENT.with(|slot| *slot.borrow_mut() = None);
    Ok(())
}

async fn open_document(
    session: Session,
    bytes: Vec<u8>,
    format: Format,
    propose: bool,
) -> Result<(), String> {
    let module = module_ready().await?;
    let mut document = bridge::Document::open(&module, &bytes)?;
    if format == Format::Pdf {
        document = with_cmaps(&module, document, &bytes).await?;
    }
    if format.laid_out() {
        lay_out(&document).await?;
    }

    let pages = (0..document.page_count())
        .map(|index| match document.page_size(index) {
            Ok(size) => (size.width, size.height),
            Err(_) => (595.0, 842.0),
        })
        .collect::<Vec<_>>();
    session.characters.set(document.character_count());
    session.to_read.set(match format {
        Format::Pdf => document.pages_to_read()?,
        _ => Vec::new(),
    });
    session.pages.set(pages);
    session.view.set(View::Pages);
    adopt_text(session, &document, propose)?;
    PICTURE.with(|slot| *slot.borrow_mut() = None);
    DOCUMENT.with(|slot| *slot.borrow_mut() = Some(document));
    Ok(())
}

/// Hand the module the CMaps a PDF's text asked for, and read it again with
/// them. A CMap can name a parent, and a character collection its UCS2 table,
/// which the next reading asks for in turn, until none is missing: each name is
/// fetched once, and one not served refuses the document.
async fn with_cmaps(
    module: &JsValue,
    mut document: bridge::Document,
    bytes: &[u8],
) -> Result<bridge::Document, String> {
    loop {
        let missing: Vec<String> = document
            .cmaps_needed()?
            .into_iter()
            .filter(|name| CMAPS.with(|cmaps| !cmaps.borrow().contains(name)))
            .collect();
        if missing.is_empty() {
            break;
        }
        let urls: Vec<String> = missing
            .iter()
            .map(|name| format!("/redactor/cmaps/{name}.cmap"))
            .collect();
        for (name, bytes) in missing.iter().zip(fetch_all(&urls).await?) {
            bridge::add_cmap(module, name, &bytes)?;
            CMAPS.with(|cmaps| cmaps.borrow_mut().insert(name.clone()));
        }
        document = bridge::Document::open(module, bytes)?;
    }
    Ok(document)
}

/// Fetch the font files the office module asks for, all at once and each only
/// once, hand it their bytes, declare the same bytes to the browser under the
/// family its pages name, then lay out: what is drawn is what was measured.
async fn lay_out(document: &bridge::Document) -> Result<(), String> {
    let faces = document.fonts_needed()?;
    let missing: Vec<String> = faces
        .iter()
        .filter_map(|face| match &face.source {
            bridge::Source::File(file) => Some(file.clone()),
            bridge::Source::Embedded(_) => None,
        })
        .filter(|file| FONTS.with(|fonts| !fonts.borrow().contains_key(file)))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let urls: Vec<String> = missing
        .iter()
        .map(|file| format!("/redactor/fonts/{file}"))
        .collect();
    let fetched = fetch_all(&urls).await?;
    FONTS.with(|fonts| fonts.borrow_mut().extend(missing.into_iter().zip(fetched)));

    let loading = js_sys::Array::new();
    let mut added: HashSet<String> = HashSet::new();
    for face in faces {
        let bytes = match face.source {
            bridge::Source::File(file) => {
                let bytes = FONTS
                    .with(|fonts| fonts.borrow().get(&file).cloned())
                    .ok_or_else(|| format!("la police {file} n'a pas été reçue"))?;
                if added.insert(file.clone()) {
                    document.add_font(&file, &bytes)?;
                }
                bytes
            }
            bridge::Source::Embedded(bytes) => bytes,
        };
        if let Some(loaded) = declare(&face.family, &bytes)? {
            loading.push(&loaded);
        }
    }
    JsFuture::from(js_sys::Promise::all(&loading))
        .await
        .map_err(|_| "une police du document n'a pas pu être lue".to_string())?;
    document.layout()
}

async fn fetch_all(urls: &[String]) -> Result<Vec<js_sys::Uint8Array>, String> {
    let window = window().ok_or("pas de fenêtre")?;
    let unreachable =
        |_| "les polices ou les tables du document n'ont pas pu être chargées".to_string();
    let requests = js_sys::Array::new();
    for url in urls {
        requests.push(&window.fetch_with_str(&file_url(url)));
    }
    let responses: js_sys::Array = JsFuture::from(js_sys::Promise::all(&requests))
        .await
        .map_err(unreachable)?
        .into();
    let bodies = js_sys::Array::new();
    for (url, response) in urls.iter().zip(responses.iter()) {
        let response: web_sys::Response = response.dyn_into().map_err(unreachable)?;
        if !response.ok() {
            return Err(format!("{url} n'a pas été servi ({})", response.status()));
        }
        let body = response.array_buffer().map_err(unreachable)?;
        bodies.push(&body);
    }
    let buffers: js_sys::Array = JsFuture::from(js_sys::Promise::all(&bodies))
        .await
        .map_err(unreachable)?
        .into();
    Ok(buffers
        .iter()
        .map(|b| js_sys::Uint8Array::new(&b))
        .collect())
}

/// Give the page's font set a face under `family`, once per family, and hand
/// back the promise that settles when the browser has parsed it.
fn declare(family: &str, bytes: &js_sys::Uint8Array) -> Result<Option<js_sys::Promise>, String> {
    if DECLARED.with(|declared| declared.borrow().contains(family)) {
        return Ok(None);
    }
    let refused = |_| format!("la police {family} n'a pas pu être déclarée");
    let face = web_sys::FontFace::new_with_js_u8_array(family, bytes).map_err(refused)?;
    document()
        .ok_or("pas de document")?
        .fonts()
        .add(&face)
        .map_err(refused)?;
    DECLARED.with(|declared| declared.borrow_mut().insert(family.to_string()));
    face.loaded().map(Some).map_err(refused)
}

/// Locate every string of the list, ticked or not, so a count stays readable
/// after the user unticks one.
pub fn refresh(session: Session, texts: Vec<String>) {
    if texts.is_empty() {
        session.hits.set(Hits::default());
        return;
    }
    let found = DOCUMENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|document| document.search(&texts))
    });
    match found {
        Some(Ok(hits)) => session.hits.set(Hits {
            needles: texts,
            hits: hits
                .into_iter()
                .map(|h| Hit {
                    needle: h.needle,
                    page: h.page,
                    quads: h.quads,
                    start: h.start,
                    end: h.end,
                })
                .collect(),
        }),
        Some(Err(why)) => session.status.set(Status::Failed(why)),
        None => session.hits.set(Hits::default()),
    }
}

/// The strings labels are given to, as the office module takes them, in
/// order. `labelled` carries the labels the user set.
fn entries(session: Session, labelled: bool) -> Vec<bridge::Entry> {
    session
        .labelled()
        .into_iter()
        .map(|(n, held)| bridge::Entry {
            text: n.text,
            kind: n.kind,
            label: n.label.filter(|_| labelled),
            held,
        })
        .collect()
}

/// Compute the label of each ticked string of a Word document, as its
/// redaction will.
pub fn relabel(session: Session) {
    if !session.format.get().laid_out() {
        if session.labels.with_untracked(|labels| !labels.is_empty()) {
            session.labels.set(HashMap::new());
        }
        return;
    }
    let entries = entries(session, false);
    let Some(module) = MODULE.with(|slot| slot.borrow().clone()) else {
        return;
    };
    match bridge::labels(&module, &entries) {
        Ok(found) => session
            .labels
            .set(found.into_iter().map(|l| (l.text, l.label)).collect()),
        Err(why) => session.status.set(Status::Failed(why)),
    }
}

fn drawn_svg(page: usize) -> Result<String, String> {
    DOCUMENT
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|document| document.page_svg(page))
        })
        .ok_or("aucun document ouvert")?
}

/// One page, as the SVG the module drew. The faces a PDF page writes its
/// text in are fetched once it is on screen: the browser redraws the text when
/// they arrive.
pub fn page_svg(page: usize, format: Format) -> Result<String, String> {
    let svg = drawn_svg(page)?;
    if format == Format::Pdf {
        let faces = DOCUMENT
            .with(|slot| slot.borrow().as_ref().map(|d| d.page_fonts(page)))
            .ok_or("aucun document ouvert")??;
        let wanted: Vec<(String, String)> = faces
            .into_iter()
            .filter_map(|face| match face.source {
                bridge::Source::File(file) => Some((face.family, file)),
                bridge::Source::Embedded(_) => None,
            })
            .filter(|(family, _)| REQUESTED.with(|r| r.borrow_mut().insert(family.clone())))
            .collect();
        if !wanted.is_empty() {
            leptos::task::spawn_local(async move {
                let _ = declare_files(wanted).await;
            });
        }
    }
    Ok(svg)
}

/// Fetch the files not yet fetched and declare each under its family.
async fn declare_files(faces: Vec<(String, String)>) -> Result<(), String> {
    let missing: Vec<String> = faces
        .iter()
        .map(|(_, file)| file.clone())
        .filter(|file| FONTS.with(|fonts| !fonts.borrow().contains_key(file)))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let urls: Vec<String> = missing
        .iter()
        .map(|file| format!("/redactor/fonts/{file}"))
        .collect();
    let fetched = fetch_all(&urls).await?;
    FONTS.with(|fonts| fonts.borrow_mut().extend(missing.into_iter().zip(fetched)));
    for (family, file) in faces {
        if let Some(bytes) = FONTS.with(|fonts| fonts.borrow().get(&file).cloned()) {
            declare(&family, &bytes)?;
        }
    }
    Ok(())
}

/// Draw the picture into `canvas` at `scale`.
pub fn paint(canvas: &web_sys::HtmlCanvasElement, scale: f64) -> Result<(), String> {
    let ratio = window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let context: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .ok_or("le navigateur n'a pas rendu de contexte 2d")?
        .dyn_into()
        .map_err(|_| "le navigateur n'a pas rendu de contexte 2d".to_string())?;
    let bitmap = PICTURE
        .with(|slot| slot.borrow().clone())
        .ok_or("aucune image ouverte")?;
    let width = (bitmap.width() as f64 * scale * ratio).round();
    let height = (bitmap.height() as f64 * scale * ratio).round();
    canvas.set_width(width as u32);
    canvas.set_height(height as u32);
    context
        .draw_image_with_image_bitmap_and_dw_and_dh(&bitmap, 0.0, 0.0, width, height)
        .map_err(|_| "cette image n'a pas pu être dessinée".to_string())
}

/// The words of `page`, line by line, or none when nothing on it can be read.
pub fn page_text(page: usize) -> Vec<Vec<PageWord>> {
    DOCUMENT
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|document| document.page_text(page))
        })
        .and_then(Result::ok)
        .unwrap_or_default()
}

/// 144 dpi against the 72 units of a PDF page. Tesseract asks for 300; a scanned
/// decision rasterised at 150 came back with every name and every accent, and
/// the cost of a scale is its square.
const READING_SCALE: f64 = 2.0;

/// Share of a word's height added to each side of the box the reader returns,
/// which stops up to 4 px short of a first or last letter at 144 dpi: the curve
/// of an O, the stem of a D.
const WORD_PAD: f64 = 0.2;

/// Read the pages with the eye rather than with the file, for a document that
/// carries no text of its own.
pub fn read_pages(session: Session, eye: Eye) {
    if session.busy.get_untracked() {
        return;
    }
    session.busy.set(true);
    session.eye.set(eye);
    session
        .reading
        .set(Some(Reading::Stage("chargement".into(), 0.0)));
    leptos::task::spawn_local(async move {
        breathe().await;
        match reading(session).await {
            Ok(()) => session.reading.set(None),
            Err(why) => session.reading.set(Some(Reading::Failed(why))),
        }
        session.busy.set(false);
    });
}

/// What the pages are read on: the picture the user dropped, or a document the
/// module holds.
enum Sheet<'a> {
    Picture(&'a web_sys::ImageBitmap),
    Document(&'a bridge::Document),
}

/// How a box read on a page's pixels reaches the page: unchanged on a dropped
/// picture, by the scale of the drawing, or through the matrix that places a
/// scanned page's stored picture.
enum Placement {
    Same,
    Drawn,
    Stored {
        transform: [f64; 6],
        width: f64,
        height: f64,
    },
}

impl Placement {
    /// `[x, y, width, height]` on the page, y down.
    fn page_box(&self, [x, y, w, h]: [f64; 4]) -> [f64; 4] {
        match self {
            Placement::Same => [x, y, w, h],
            Placement::Drawn => [x, y, w, h].map(|v| v / READING_SCALE),
            Placement::Stored {
                transform: [a, b, c, d, e, f],
                width,
                height,
            } => {
                let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)].map(|(px, py)| {
                    let (u, v) = (px / width, py / height);
                    (a * u + c * v + e, b * u + d * v + f)
                });
                let xs = corners.map(|p| p.0);
                let ys = corners.map(|p| p.1);
                let (x0, x1) = (
                    xs.into_iter().fold(f64::MAX, f64::min),
                    xs.into_iter().fold(f64::MIN, f64::max),
                );
                let (y0, y1) = (
                    ys.into_iter().fold(f64::MAX, f64::min),
                    ys.into_iter().fold(f64::MIN, f64::max),
                );
                [x0, y0, x1 - x0, y1 - y0]
            }
        }
    }
}

/// The pixels of an encoded picture, at its own size.
async fn bitmap_of(bytes: &[u8], mime: &str) -> Result<web_sys::ImageBitmap, String> {
    let blob = blob_of(bytes, mime)?;
    let window = window().ok_or("pas de fenêtre")?;
    let refused = || "cette image n'a pas pu être décodée".to_string();
    let promise = window
        .create_image_bitmap_with_blob(&blob)
        .map_err(|_| refused())?;
    JsFuture::from(promise)
        .await
        .map_err(|_| refused())?
        .dyn_into()
        .map_err(|_| refused())
}

/// The image of one page, made ready by `module` and put on a canvas of its own:
/// the reader takes a canvas and refuses raw pixels. A scanned page is read on
/// the picture the file stores, pixel for pixel, so that its boxes and the
/// rectangles that follow never pass through a drawing. Any other page goes
/// through the browser's own SVG renderer, as an image, which loads nothing
/// from outside the SVG.
async fn page_image(
    module: &JsValue,
    sheet: &Sheet<'_>,
    index: usize,
    size: (f64, f64),
) -> Result<(JsValue, bridge::Prepared, Placement), String> {
    let canvas = canvas_element()?;
    let context: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .ok_or("le navigateur n'a pas rendu de contexte 2d")?
        .dyn_into()
        .map_err(|_| "le navigateur n'a pas rendu de contexte 2d".to_string())?;
    let read_back = |width: u32, height: u32| {
        context
            .get_image_data(0.0, 0.0, width as f64, height as f64)
            .map(|data| data.data().0)
            .map_err(|_| "la page n'a pas pu être lue".to_string())
    };
    let put_bitmap = |bitmap: &web_sys::ImageBitmap| {
        let (width, height) = (bitmap.width(), bitmap.height());
        canvas.set_width(width);
        canvas.set_height(height);
        context
            .draw_image_with_image_bitmap(bitmap, 0.0, 0.0)
            .map_err(|_| "cette image n'a pas pu être dessinée".to_string())?;
        Ok::<_, String>((width, height, read_back(width, height)?))
    };

    let (width, height, pixels, placement) = match sheet {
        Sheet::Picture(bitmap) => {
            let (width, height, pixels) = put_bitmap(bitmap)?;
            (width, height, pixels, Placement::Same)
        }
        Sheet::Document(document) => match document.page_scan(index)? {
            Some(picture) => {
                let bitmap = bitmap_of(&picture.data, &picture.mime).await?;
                let (width, height, pixels) = put_bitmap(&bitmap)?;
                let placement = Placement::Stored {
                    transform: picture.transform,
                    width: width as f64,
                    height: height as f64,
                };
                (width, height, pixels, placement)
            }
            None => {
                let picture = svg_image(&document.page_svg(index)?).await?;
                let (width, height) = (
                    (size.0 * READING_SCALE).round() as u32,
                    (size.1 * READING_SCALE).round() as u32,
                );
                canvas.set_width(width);
                canvas.set_height(height);
                context.set_fill_style_str("#ffffff");
                context.fill_rect(0.0, 0.0, width as f64, height as f64);
                context
                    .draw_image_with_html_image_element_and_dw_and_dh(
                        &picture,
                        0.0,
                        0.0,
                        width as f64,
                        height as f64,
                    )
                    .map_err(|_| "la page n'a pas pu être dessinée".to_string())?;
                (width, height, read_back(width, height)?, Placement::Drawn)
            }
        },
    };
    let prepared = bridge::prepare_scan(module, &pixels, width, height)?;
    let (width, height, pixels) = prepared.image()?;
    let data =
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(&pixels), width, height)
            .map_err(|_| "la page n'a pas pu être convertie".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    context
        .put_image_data(&data, 0.0, 0.0)
        .map_err(|_| "la page n'a pas pu être dessinée".to_string())?;
    Ok((canvas.into(), prepared, placement))
}

/// `svg` decoded as an image, through a `data:` URL, which the page's
/// `img-src` admits where it refuses a `blob:` one.
async fn svg_image(svg: &str) -> Result<web_sys::HtmlImageElement, String> {
    let refused = |_| "la page n'a pas pu être dessinée".to_string();
    let picture = web_sys::HtmlImageElement::new().map_err(refused)?;
    let encoded: String = js_sys::encode_uri_component(svg).into();
    picture.set_src(&format!("data:image/svg+xml;charset=utf-8,{encoded}"));
    JsFuture::from(picture.decode()).await.map_err(refused)?;
    Ok(picture)
}

const READERS: [(&str, &str); 2] = [("ljOcrReady", "/ocr.js"), ("ljHtrReady", "/htr.js")];

/// The reader of `session`, loaded once, its progress shown on `session`.
async fn text_reader(session: Session) -> Result<JsValue, String> {
    let (global, src) = match session.eye.get_untracked() {
        Eye::Fast => READERS[0],
        Eye::Slow => READERS[1],
    };
    let reader = loaded_reader(global, src).await?;
    bridge::watch_progress(&reader, move |stage, at| {
        session.reading.set(Some(Reading::Stage(stage, at)));
    });
    Ok(reader)
}

async fn loaded_reader(global: &str, src: &str) -> Result<JsValue, String> {
    load(global, src);
    posed(global).await?;
    bridge::reader(global).await
}

/// Megabytes each step of the offline preparation downloads, brotli: the
/// module, the fonts, the CMaps, the fast reader, the slow one.
const STEPS: [f64; 5] = [1.5, 11.8, 0.7, 1.4, 13.0];

/// Fetch into the page every file the tool may ask for, and start both readers,
/// so the network can be cut before a document is dropped.
pub fn prepare(session: Session) {
    if matches!(
        session.offline.get_untracked(),
        Offline::Loading(_) | Offline::Ready
    ) {
        return;
    }
    session.offline.set(Offline::Loading(0.0));
    leptos::task::spawn_local(async move {
        session.offline.set(match fetch_everything(session).await {
            Ok(()) => Offline::Ready,
            Err(why) => Offline::Failed(why),
        });
    });
}

async fn fetch_everything(session: Session) -> Result<(), String> {
    let report = move |step: usize, at: f64| {
        let before: f64 = STEPS[..step].iter().sum();
        let total: f64 = STEPS.iter().sum();
        let done = before + STEPS[step] * at.clamp(0.0, 1.0);
        session.offline.set(Offline::Loading(done / total));
    };
    let module = module_ready().await?;

    let fonts: Vec<String> = listing("/redactor/fonts")
        .await?
        .into_iter()
        .filter(|file| FONTS.with(|fonts| !fonts.borrow().contains_key(file)))
        .collect();
    let urls: Vec<String> = fonts
        .iter()
        .map(|file| format!("/redactor/fonts/{file}"))
        .collect();
    let fetched = fetch_counted(&urls, |at| report(1, at)).await?;
    FONTS.with(|slot| slot.borrow_mut().extend(fonts.into_iter().zip(fetched)));

    let cmaps: Vec<String> = listing("/redactor/cmaps")
        .await?
        .into_iter()
        .filter(|name| CMAPS.with(|cmaps| !cmaps.borrow().contains(name)))
        .collect();
    let urls: Vec<String> = cmaps
        .iter()
        .map(|name| format!("/redactor/cmaps/{name}.cmap"))
        .collect();
    for (name, bytes) in cmaps
        .iter()
        .zip(fetch_counted(&urls, |at| report(2, at)).await?)
    {
        bridge::add_cmap(&module, name, &bytes)?;
        CMAPS.with(|slot| slot.borrow_mut().insert(name.clone()));
    }

    for (step, (global, src)) in (3..).zip(READERS) {
        let reader = loaded_reader(global, src).await?;
        bridge::watch_progress(&reader, move |_, at| report(step, at));
        bridge::warm(&reader).await?;
        report(step, 1.0);
    }
    Ok(())
}

/// The names a folder of the tool lists in its `index.txt`.
async fn listing(folder: &str) -> Result<Vec<String>, String> {
    let index = fetch_all(&[format!("{folder}/index.txt")]).await?;
    let text = String::from_utf8(index[0].to_vec())
        .map_err(|_| format!("{folder}/index.txt n'est pas du texte"))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// `fetch_all` by batches, each reporting how far through `urls` it got.
async fn fetch_counted(
    urls: &[String],
    report: impl Fn(f64),
) -> Result<Vec<js_sys::Uint8Array>, String> {
    let mut fetched = Vec::with_capacity(urls.len());
    for batch in urls.chunks(8) {
        fetched.extend(fetch_all(batch).await?);
        report(fetched.len() as f64 / urls.len() as f64);
    }
    Ok(fetched)
}

/// The words of every page of `sheet`, each box on its page: y up on a PDF
/// page, whose `pages` give the size, y down on a picture.
async fn read_sheet(
    module: &JsValue,
    reader: &JsValue,
    sheet: &Sheet<'_>,
    pages: &[(usize, (f64, f64))],
    flip: bool,
) -> Result<Vec<bridge::Word>, String> {
    let mut words: Vec<bridge::Word> = Vec::new();
    for &(index, (width, height)) in pages {
        let (image, prepared, placement) =
            page_image(module, sheet, index, (width, height)).await?;
        let found = bridge::read(reader, &image, index).await?;
        for word in found {
            let pad = word.height * WORD_PAD;
            let source = prepared.source_box(&bridge::Word {
                x: word.x - pad,
                width: word.width + 2.0 * pad,
                ..word.clone()
            })?;
            let [x, y, w, h] = placement.page_box(source);
            let y = match flip {
                true => height - (y + h),
                false => y,
            };
            words.push(bridge::Word {
                x,
                y,
                width: w,
                height: h,
                ..word
            });
        }
        breathe().await;
    }
    Ok(words)
}

async fn reading(session: Session) -> Result<(), String> {
    let reader = text_reader(session).await?;
    let module = module_ready().await?;
    let sizes = session.pages.get_untracked();
    let pages: Vec<(usize, (f64, f64))> = session
        .to_read
        .get_untracked()
        .into_iter()
        .map(|index| (index, sizes[index]))
        .collect();
    let flip = session.format.get_untracked().flips_y();
    let held = DOCUMENT.with(|slot| slot.borrow().clone());
    let words = match (&held, PICTURE.with(|slot| slot.borrow().clone())) {
        (Some(document), _) => {
            read_sheet(&module, &reader, &Sheet::Document(document), &pages, flip).await?
        }
        (None, Some(bitmap)) => {
            read_sheet(&module, &reader, &Sheet::Picture(&bitmap), &pages, flip).await?
        }
        (None, None) => return Err("aucun document ouvert".into()),
    };

    let document = match held {
        Some(document) => document,
        None => {
            let document = bridge::Document::open_scan(&module)?;
            DOCUMENT.with(|slot| *slot.borrow_mut() = Some(document.clone()));
            document
        }
    };
    session.characters.set(document.adopt_words(&words)?);
    session.read_by_eye.set(true);
    adopt_text(session, &document, true)
}

/// The ticked strings a reader still finds on the pages of `bytes`, the file
/// just produced from a document read by eye.
async fn read_again(
    session: Session,
    bytes: &[u8],
    needles: &[String],
) -> Result<Vec<String>, String> {
    if needles.is_empty() {
        return Ok(Vec::new());
    }
    let reader = text_reader(session).await?;
    let module = module_ready().await?;
    let words = match session.format.get_untracked() {
        Format::Image => {
            let bitmap = bitmap_of(bytes, "image/png").await?;
            let pages = [(0, (bitmap.width() as f64, bitmap.height() as f64))];
            read_sheet(&module, &reader, &Sheet::Picture(&bitmap), &pages, false).await?
        }
        format => {
            let document = bridge::Document::open(&module, bytes)?;
            let pages = session
                .to_read
                .get_untracked()
                .into_iter()
                .map(|index| {
                    document
                        .page_size(index)
                        .map(|size| (index, (size.width, size.height)))
                })
                .collect::<Result<Vec<_>, _>>()?;
            read_sheet(
                &module,
                &reader,
                &Sheet::Document(&document),
                &pages,
                format.flips_y(),
            )
            .await?
        }
    };
    session.reading.set(None);
    let read = bridge::Document::open_scan(&module)?;
    read.adopt_words(&words)?;
    let hits = read.search(needles)?;
    Ok(needles
        .iter()
        .enumerate()
        .filter(|(index, _)| hits.iter().any(|hit| hit.needle == *index))
        .map(|(_, needle)| needle.clone())
        .collect())
}

/// Take the blocks, and the proposals when they are wanted, of a document that
/// just changed what it holds.
fn adopt_text(session: Session, document: &bridge::Document, propose: bool) -> Result<(), String> {
    session.blocks.set(
        document
            .blocks()?
            .into_iter()
            .map(|b| TextBlock {
                start: b.start,
                text: b.text,
            })
            .collect(),
    );
    if !propose {
        return Ok(());
    }
    let proposed = document.suggestions()?;
    session.needles.update(|list| {
        for suggestion in proposed {
            if !list.iter().any(|n| n.text == suggestion.text) {
                let on = !matches!(suggestion.kind.as_str(), "person" | "birth-place");
                list.push(Needle {
                    text: suggestion.text,
                    kind: suggestion.kind,
                    on,
                    label: None,
                });
            }
        }
    });
    Ok(())
}

/// Remove what is ticked and what was drawn, then reopen the result so the user
/// sees the document as it will leave the machine.
pub fn apply(session: Session) {
    if session.busy.get_untracked() {
        return;
    }
    session.busy.set(true);
    session.verdict.set(None);
    leptos::task::spawn_local(async move {
        breathe().await;
        if let Err(why) = run(session).await {
            session.status.set(Status::Failed(why));
        }
        session.busy.set(false);
    });
}

/// The rectangles that cover a match, one per run of glyphs on the same line.
/// A single enclosing box would be wrong: the space-stripping fold matches two
/// fragments that sit far apart on the page, and one box over both blackens
/// everything between them.
fn boxes(hit: &Hit) -> Vec<Rect> {
    let Some(page) = hit.page else {
        return Vec::new();
    };
    let mut runs: Vec<Rect> = Vec::new();
    for quad in &hit.quads {
        let xs = quad.iter().map(|p| p[0]);
        let ys = quad.iter().map(|p| p[1]);
        let (x0, x1) = (
            xs.clone().fold(f64::MAX, f64::min),
            xs.fold(f64::MIN, f64::max),
        );
        let (y0, y1) = (
            ys.clone().fold(f64::MAX, f64::min),
            ys.fold(f64::MIN, f64::max),
        );
        if x1 <= x0 && y1 <= y0 {
            continue;
        }
        let joined = runs.last_mut().filter(|run| {
            let shared = run.y.max(y0) < (run.y + run.height).min(y1);
            let gap = x0 - (run.x + run.width);
            shared && gap > -run.height && gap < run.height
        });
        match joined {
            Some(run) => {
                let bottom = (run.y + run.height).max(y1);
                run.width = (run.x + run.width).max(x1) - run.x;
                run.y = run.y.min(y0);
                run.height = bottom - run.y;
            }
            None => runs.push(Rect {
                page,
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            }),
        }
    }
    runs
}

async fn run(session: Session) -> Result<(), String> {
    let by_eye = session.read_by_eye.get_untracked();
    let format = session.format.get_untracked();
    // A PDF read by eye still has the glyphs its file draws: they go as glyphs,
    // and the rectangles take what was read on its pictures.
    let needles = match by_eye && format != Format::Pdf {
        true => Vec::new(),
        false => untrack(|| session.active()),
    };
    let entries = match by_eye {
        true => Vec::new(),
        false => untrack(|| entries(session, true)),
    };
    let mut rects = session.rects.get_untracked();
    let found_by_eye = match by_eye {
        true => {
            let hits = session.live_hits();
            rects.extend(hits.iter().flat_map(boxes));
            hits.len()
        }
        false => 0,
    };
    let name = session.name.get_untracked();

    let (bytes, verdict) = match format {
        Format::Image => {
            let bytes = redact_image(&rects).await?;
            let size = bytes.len();
            let verdict = Verdict {
                found: found_by_eye,
                removed: rects.len(),
                leaks: Vec::new(),
                size,
                by_eye,
                check: Check::Done,
            };
            (bytes, verdict)
        }
        _ => {
            let drawn: Vec<bridge::Rect> = rects
                .iter()
                .map(|r| bridge::Rect {
                    page: r.page,
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                })
                .collect();
            let applied = DOCUMENT
                .with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|document| match format.laid_out() {
                            true => document.redact(&entries),
                            false => document.apply(&needles, &drawn),
                        })
                })
                .ok_or("aucun document ouvert")??;
            let size = applied.bytes.len();
            let verdict = Verdict {
                found: match by_eye {
                    true => found_by_eye,
                    false => applied.found,
                },
                removed: match by_eye {
                    true => rects.len(),
                    false => applied.removed,
                },
                leaks: applied.leaks,
                size,
                by_eye,
                check: Check::Done,
            };
            (applied.bytes, verdict)
        }
    };

    let ticked: Vec<String> = session.active();
    let check = by_eye && !ticked.is_empty();
    let verdict = Verdict {
        check: match check {
            true => Check::Running,
            false => Check::Done,
        },
        ..verdict
    };
    let clean = verdict.leaks.is_empty();
    session.verdict.set(Some(verdict));
    if !clean {
        return Ok(());
    }
    PRODUCED.with(|slot| *slot.borrow_mut() = Some((produced_name(&name, format), bytes.clone())));
    session.rects.set(Vec::new());
    session.keep_places(entries.into_iter().map(|e| (e.text, e.kind)));
    breathe().await;
    let produced = check.then(|| bytes.clone());
    ingest(session, &name, bytes, false).await?;
    session.produced_from.set_value(Some(session.ticked()));
    if let Some(bytes) = produced {
        check_behind(session, bytes, ticked);
    }
    Ok(())
}

/// Read the produced pages again once they are shown, and withdraw the file if
/// a ticked string still reads there or the reading fails. A newer pass or file
/// makes the outcome moot.
fn check_behind(session: Session, bytes: Vec<u8>, ticked: Vec<String>) {
    let generation = session.generation.get_untracked();
    leptos::task::spawn_local(async move {
        let outcome = read_again(session, &bytes, &ticked).await;
        if session.generation.get_untracked() != generation {
            return;
        }
        let (leaks, check) = match outcome {
            Ok(leaks) => (leaks, Check::Done),
            Err(why) => {
                session.reading.set(None);
                (Vec::new(), Check::Failed(why))
            }
        };
        if !leaks.is_empty() || check != Check::Done {
            PRODUCED.with(|slot| *slot.borrow_mut() = None);
        }
        session.verdict.update(|verdict| {
            if let Some(verdict) = verdict {
                verdict.leaks = leaks;
                verdict.check = check;
            }
        });
    });
}

/// Fill the drawn rectangles on a copy of the picture and write it back as PNG,
/// which also leaves the EXIF of the source behind.
async fn redact_image(rects: &[Rect]) -> Result<Vec<u8>, String> {
    let bitmap = PICTURE
        .with(|slot| slot.borrow().clone())
        .ok_or("aucune image ouverte")?;
    let canvas = canvas_element()?;
    canvas.set_width(bitmap.width());
    canvas.set_height(bitmap.height());
    let context: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .ok_or("le navigateur n'a pas rendu de contexte 2d")?
        .dyn_into()
        .map_err(|_| "le navigateur n'a pas rendu de contexte 2d".to_string())?;
    context
        .draw_image_with_image_bitmap_and_dw_and_dh(
            &bitmap,
            0.0,
            0.0,
            bitmap.width() as f64,
            bitmap.height() as f64,
        )
        .map_err(|_| "cette image n'a pas pu être dessinée".to_string())?;
    context.set_fill_style_str("#000000");
    for rect in rects {
        context.fill_rect(rect.x, rect.y, rect.width, rect.height);
    }

    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = canvas.to_blob(&resolve);
    });
    let refused = || "l'image n'a pas pu être réencodée".to_string();
    let blob: web_sys::Blob = JsFuture::from(promise)
        .await
        .map_err(|_| refused())?
        .dyn_into()
        .map_err(|_| refused())?;
    bytes_of(&blob).await
}

fn canvas_element() -> Result<web_sys::HtmlCanvasElement, String> {
    document()
        .ok_or("pas de document")?
        .create_element("canvas")
        .map_err(|_| "canevas impossible".to_string())?
        .dyn_into()
        .map_err(|_| "canevas impossible".to_string())
}

/// Hand a file to the browser's download path.
pub fn save(name: &str, bytes: &[u8]) {
    let Ok(blob) = blob_of(bytes, lj_vocab::files::media_type(name)) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Some(anchor) = document()
        .and_then(|d| d.create_element("a").ok())
        .and_then(|a| a.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    {
        anchor.set_href(&url);
        anchor.set_download(name);
        anchor.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// The file waiting to be downloaded, and its size.
pub fn produced() -> bool {
    PRODUCED.with(|slot| slot.borrow().is_some())
}

/// The name and bytes of the file the last anonymisation produced.
pub fn produced_file() -> Option<(String, Vec<u8>)> {
    PRODUCED.with(|slot| slot.borrow().clone())
}

struct Watch {
    observer: web_sys::IntersectionObserver,
    _callback: Closure<dyn FnMut(js_sys::Array)>,
    root: web_sys::Element,
    scroll: Closure<dyn FnMut()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.observer.disconnect();
        let _ = self
            .root
            .remove_event_listener_with_callback("scroll", self.scroll.as_ref().unchecked_ref());
    }
}

/// The last page whose top has crossed the upper third of the viewport.
fn current_page(root: &web_sys::Element, count: usize) -> usize {
    let line = root.get_bounding_client_rect().top() + root.client_height() as f64 / 3.0;
    let Some(document) = document() else {
        return 0;
    };
    (0..count)
        .take_while(|page| {
            document
                .get_element_by_id(&format!("redact-page-{page}"))
                .is_some_and(|element| element.get_bounding_client_rect().top() <= line)
        })
        .last()
        .unwrap_or(0)
}

/// Follow the pages entering and leaving `root`, which decides what gets
/// drawn, and the scroll, which moves the page counter.
pub fn watch_pages(session: Session, root: &web_sys::Element) {
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
        let mut seen: Vec<(usize, bool)> = Vec::new();
        for entry in entries.iter() {
            let Ok(entry) = entry.dyn_into::<web_sys::IntersectionObserverEntry>() else {
                continue;
            };
            let Some(index) = entry
                .target()
                .get_attribute("data-page")
                .and_then(|value| value.parse::<usize>().ok())
            else {
                continue;
            };
            seen.push((index, entry.is_intersecting()));
        }
        if seen.is_empty() {
            return;
        }
        session.visible.update(|pages| {
            for (index, inside) in &seen {
                pages.retain(|page| page != index);
                if *inside {
                    pages.push(*index);
                }
            }
            pages.sort_unstable();
        });
    });

    let scrolled = root.clone();
    let scroll = Closure::<dyn FnMut()>::new(move || {
        let page = current_page(&scrolled, session.pages.with_untracked(Vec::len));
        if session.page.get_untracked() != page {
            session.page.set(page);
        }
    });
    let _ = root.add_event_listener_with_callback("scroll", scroll.as_ref().unchecked_ref());

    let options = web_sys::IntersectionObserverInit::new();
    options.set_root(Some(root));
    options.set_root_margin("500px 0px 500px 0px");
    let Ok(observer) = web_sys::IntersectionObserver::new_with_options(
        callback.as_ref().unchecked_ref(),
        &options,
    ) else {
        return;
    };
    WATCH.with(|slot| {
        *slot.borrow_mut() = Some(Watch {
            observer,
            _callback: callback,
            root: root.clone(),
            scroll,
        })
    });
    session.visible.set(vec![0]);
}

pub fn observe_page(element: &web_sys::Element) {
    WATCH.with(|slot| {
        if let Some(watch) = slot.borrow().as_ref() {
            watch.observer.observe(element);
        }
    });
}

/// Bring a page into view, which is what every page control ends up doing.
pub fn go_to(session: Session, page: usize) {
    let Some(element) =
        document().and_then(|d| d.get_element_by_id(&format!("redact-page-{page}")))
    else {
        return;
    };
    session.page.set(page);
    let options = web_sys::ScrollIntoViewOptions::new();
    options.set_behavior(web_sys::ScrollBehavior::Instant);
    options.set_block(web_sys::ScrollLogicalPosition::Start);
    element.scroll_into_view_with_scroll_into_view_options(&options);
}

/// Centre `hit` in the viewport: on its page when pages are drawn, on its
/// paragraph in the text view.
pub fn show(session: Session, hit: &Hit) {
    let Some(document) = document() else { return };
    let Some(viewport) = document.get_element_by_id("redact-viewport") else {
        return;
    };
    let page = hit
        .page
        .filter(|_| session.view.get_untracked() == View::Pages);
    let Some(page) = page else {
        let start = session.blocks.with_untracked(|blocks| {
            blocks
                .iter()
                .map(|b| b.start)
                .take_while(|start| *start <= hit.start)
                .last()
        });
        if let Some(element) =
            start.and_then(|s| document.get_element_by_id(&format!("redact-block-{s}")))
        {
            let options = web_sys::ScrollIntoViewOptions::new();
            options.set_behavior(web_sys::ScrollBehavior::Instant);
            options.set_block(web_sys::ScrollLogicalPosition::Center);
            element.scroll_into_view_with_scroll_into_view_options(&options);
        }
        return;
    };
    let Some(element) = document.get_element_by_id(&format!("redact-page-{page}")) else {
        return;
    };
    session.page.set(page);
    let height = session
        .pages
        .with_untracked(|pages| pages.get(page).map(|p| p.1))
        .unwrap_or(0.0);
    let points = || hit.quads.iter().flatten();
    let (x0, x1) = points().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(p[0]), hi.max(p[0]))
    });
    let (y0, y1) = points().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(p[1]), hi.max(p[1]))
    });
    let (x, y) = match hit.quads.is_empty() {
        true => (0.0, 0.0),
        false => {
            let top = match session.format.get_untracked().flips_y() {
                true => height - y1,
                false => y0,
            };
            ((x0 + x1) / 2.0, top + (y1 - y0) / 2.0)
        }
    };
    let zoom = session.zoom.get_untracked();
    let frame = viewport.get_bounding_client_rect();
    let sheet = element.get_bounding_client_rect();
    let left = viewport.scroll_left() as f64 + sheet.left() - frame.left() + x * zoom
        - viewport.client_width() as f64 / 2.0;
    let top = viewport.scroll_top() as f64 + sheet.top() - frame.top() + y * zoom
        - viewport.client_height() as f64 / 2.0;
    viewport.set_scroll_left(left.max(0.0).round() as i32);
    viewport.set_scroll_top(top.max(0.0).round() as i32);
}

/// A wheel zoom in progress: the zoom it started from, the one it has reached,
/// and the point of a page held under the pointer.
struct Pinch {
    from: f64,
    to: f64,
    page: usize,
    at: (f64, f64),
    pointer: (f64, f64),
    ticket: u64,
}

thread_local! {
    static PINCH: RefCell<Option<Pinch>> = const { RefCell::new(None) };
    static PINCH_TICKET: RefCell<u64> = const { RefCell::new(0) };
}

fn html(id: &str) -> Option<web_sys::HtmlElement> {
    document()?.get_element_by_id(id)?.dyn_into().ok()
}

/// Scale by `factor` around the pointer. Resizing every page on each wheel step
/// relays out the whole document, so the gesture only scales the column, and
/// the zoom is applied once, when the wheel has been still for a moment.
pub fn zoom_at(session: Session, factor: f64, event: &web_sys::WheelEvent) {
    let (Some(viewport), Some(column)) = (html("redact-viewport"), html("redact-pages")) else {
        return;
    };
    let frame = viewport.get_bounding_client_rect();
    let pointer = (
        event.client_x() as f64 - frame.left(),
        event.client_y() as f64 - frame.top(),
    );
    let ticket = PINCH_TICKET.with(|t| {
        *t.borrow_mut() += 1;
        *t.borrow()
    });
    let started = PINCH.with(|slot| slot.borrow().is_some());
    if !started {
        let from = session.zoom.get_untracked();
        let (page, at) = held_point(session, event.client_x() as f64, event.client_y() as f64);
        PINCH.with(|slot| {
            *slot.borrow_mut() = Some(Pinch {
                from,
                to: from,
                page,
                at,
                pointer,
                ticket,
            })
        });
        let origin = (
            viewport.scroll_left() as f64 + pointer.0,
            viewport.scroll_top() as f64 + pointer.1,
        );
        let style = column.style();
        let _ = style.set_property(
            "transform-origin",
            &format!("{}px {}px", origin.0, origin.1),
        );
        let _ = style.set_property("will-change", "transform");
    }
    let scale = PINCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        let pinch = slot.as_mut()?;
        pinch.to = (pinch.to * factor).clamp(0.1, 6.0);
        pinch.ticket = ticket;
        Some(pinch.to / pinch.from)
    });
    if let Some(scale) = scale {
        let _ = column
            .style()
            .set_property("transform", &format!("scale({scale})"));
    }
    set_timeout(
        move || settle(session, ticket),
        std::time::Duration::from_millis(140),
    );
}

/// The page under the pointer, and where on it as a fraction of its size.
fn held_point(session: Session, x: f64, y: f64) -> (usize, (f64, f64)) {
    let count = session.pages.with_untracked(Vec::len);
    let document = document();
    for page in 0..count {
        let Some(element) = document
            .as_ref()
            .and_then(|d| d.get_element_by_id(&format!("redact-page-{page}")))
        else {
            continue;
        };
        let rect = element.get_bounding_client_rect();
        if y <= rect.bottom() || page + 1 == count {
            let fx = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
            let fy = ((y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
            return (page, (fx, fy));
        }
    }
    (0, (0.0, 0.0))
}

fn settle(session: Session, ticket: u64) {
    let pinch = PINCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_ref() {
            Some(pinch) if pinch.ticket == ticket => slot.take(),
            _ => None,
        }
    });
    let Some(pinch) = pinch else { return };
    session.fit.set(false);
    session.zoom.set(pinch.to);
    request_animation_frame(move || {
        let (Some(viewport), Some(column)) = (html("redact-viewport"), html("redact-pages")) else {
            return;
        };
        let style = column.style();
        let _ = style.remove_property("transform");
        let _ = style.remove_property("transform-origin");
        let _ = style.remove_property("will-change");
        let Some(page) =
            document().and_then(|d| d.get_element_by_id(&format!("redact-page-{}", pinch.page)))
        else {
            return;
        };
        let frame = viewport.get_bounding_client_rect();
        let rect = page.get_bounding_client_rect();
        let x = rect.left() - frame.left() + pinch.at.0 * rect.width();
        let y = rect.top() - frame.top() + pinch.at.1 * rect.height();
        let left = viewport.scroll_left() as f64 + x - pinch.pointer.0;
        let top = viewport.scroll_top() as f64 + y - pinch.pointer.1;
        viewport.set_scroll_left(left.round() as i32);
        viewport.set_scroll_top(top.round() as i32);
    });
}

/// Zoom that puts the widest page exactly inside the viewport.
pub fn fit(session: Session) {
    let Some(viewport) = document().and_then(|d| d.get_element_by_id("redact-viewport")) else {
        return;
    };
    let widest = session.pages.with_untracked(|pages| {
        pages
            .iter()
            .map(|(width, _)| *width)
            .fold(0.0_f64, f64::max)
    });
    if widest <= 0.0 {
        return;
    }
    let available = (viewport.client_width() as f64 - 48.0).max(200.0);
    session.zoom.set((available / widest).clamp(0.1, 6.0));
}

/// Which edges of a rectangle a press holds; none of them is a move.
#[derive(Clone, Copy)]
struct Grip {
    left: bool,
    right: bool,
    low: bool,
    high: bool,
}

enum Gesture {
    Draw((f64, f64)),
    Edit {
        index: usize,
        grip: Grip,
        from: (f64, f64),
        original: Rect,
    },
}

thread_local! {
    static GESTURE: RefCell<Option<Gesture>> = const { RefCell::new(None) };
}

pub enum Phase {
    Down,
    Move,
    Up,
}

/// Where the pointer sits on the page, in the coordinates the backend redacts
/// in: upwards from the bottom left for a PDF, downwards for an image.
fn point(event: &web_sys::PointerEvent, size: (f64, f64), flip: bool) -> Option<(f64, f64)> {
    let element = event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())?;
    let bounds = element.get_bounding_client_rect();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    let x = (event.client_x() as f64 - bounds.left()) / bounds.width() * size.0;
    let y = (event.client_y() as f64 - bounds.top()) / bounds.height() * size.1;
    Some((
        x.clamp(0.0, size.0),
        match flip {
            true => (size.1 - y).clamp(0.0, size.1),
            false => y.clamp(0.0, size.1),
        },
    ))
}

fn span(page: usize, from: (f64, f64), to: (f64, f64)) -> Rect {
    Rect {
        page,
        x: from.0.min(to.0),
        y: from.1.min(to.1),
        width: (to.0 - from.0).abs(),
        height: (to.1 - from.1).abs(),
    }
}

/// The topmost rectangle of `page` under `at`, and the edges within `reach`.
fn grab(rects: &[Rect], page: usize, at: (f64, f64), reach: f64) -> Option<(usize, Grip)> {
    rects
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, rect)| rect.page == page)
        .find(|(_, rect)| {
            at.0 >= rect.x - reach
                && at.0 <= rect.x + rect.width + reach
                && at.1 >= rect.y - reach
                && at.1 <= rect.y + rect.height + reach
        })
        .map(|(index, rect)| {
            (
                index,
                Grip {
                    left: (at.0 - rect.x).abs() <= reach,
                    right: (at.0 - rect.x - rect.width).abs() <= reach,
                    low: (at.1 - rect.y).abs() <= reach,
                    high: (at.1 - rect.y - rect.height).abs() <= reach,
                },
            )
        })
}

fn edited(original: Rect, grip: Grip, delta: (f64, f64), size: (f64, f64)) -> Rect {
    let (mut x0, mut y0) = (original.x, original.y);
    let (mut x1, mut y1) = (x0 + original.width, y0 + original.height);
    if !(grip.left || grip.right || grip.low || grip.high) {
        let dx = delta.0.clamp(-x0, size.0 - x1);
        let dy = delta.1.clamp(-y0, size.1 - y1);
        return Rect {
            x: x0 + dx,
            y: y0 + dy,
            ..original
        };
    }
    if grip.left {
        x0 = (x0 + delta.0).clamp(0.0, size.0);
    }
    if grip.right {
        x1 = (x1 + delta.0).clamp(0.0, size.0);
    }
    if grip.low {
        y0 = (y0 + delta.1).clamp(0.0, size.1);
    }
    if grip.high {
        y1 = (y1 + delta.1).clamp(0.0, size.1);
    }
    span(original.page, (x0, y0), (x1, y1))
}

fn cursor(grip: Option<Grip>, flip: bool) -> &'static str {
    let Some(grip) = grip else { return "" };
    let (top, bottom) = match flip {
        true => (grip.high, grip.low),
        false => (grip.low, grip.high),
    };
    match (grip.left || grip.right, top || bottom) {
        (false, false) => "move",
        (true, false) => "ew-resize",
        (false, true) => "ns-resize",
        _ if (grip.left && top) || (grip.right && bottom) => "nwse-resize",
        _ => "nesw-resize",
    }
}

/// One step of drawing a rectangle over `page`, or of moving or resizing one
/// already there.
pub fn draw(session: Session, page: usize, phase: Phase, event: &web_sys::PointerEvent) {
    let size = session
        .pages
        .with_untracked(|pages| pages.get(page).copied())
        .unwrap_or((595.0, 842.0));
    let flip = session.format.get_untracked().flips_y();
    let reach = 6.0 / session.zoom.get_untracked().max(0.1);
    let element = event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok());
    let Some(at) = point(event, size, flip) else {
        return;
    };
    match phase {
        Phase::Down => {
            let grabbed = session
                .rects
                .with_untracked(|rects| grab(rects, page, at, reach));
            let gesture = match grabbed {
                Some((index, grip)) => {
                    session.selected.set(Some(index));
                    let original = session.rects.with_untracked(|rects| rects[index]);
                    Gesture::Edit {
                        index,
                        grip,
                        from: at,
                        original,
                    }
                }
                None if session.mode.get_untracked() == super::state::Mode::Draw => {
                    session.selected.set(None);
                    session.drawing.set(Some(span(page, at, at)));
                    Gesture::Draw(at)
                }
                None => {
                    session.selected.set(None);
                    return;
                }
            };
            if let Some(element) = &element {
                let _ = element.set_pointer_capture(event.pointer_id());
            }
            GESTURE.with(|slot| *slot.borrow_mut() = Some(gesture));
            event.prevent_default();
        }
        Phase::Move => GESTURE.with(|slot| match &*slot.borrow() {
            Some(Gesture::Draw(start)) => session.drawing.set(Some(span(page, *start, at))),
            Some(Gesture::Edit {
                index,
                grip,
                from,
                original,
            }) => {
                let rect = edited(*original, *grip, (at.0 - from.0, at.1 - from.1), size);
                session.rects.update(|rects| {
                    if let Some(slot) = rects.get_mut(*index) {
                        *slot = rect;
                    }
                });
            }
            None => {
                let hover = session
                    .rects
                    .with_untracked(|rects| grab(rects, page, at, reach))
                    .map(|(_, grip)| grip);
                if let Some(element) = &element {
                    let _ = element.style().set_property("cursor", cursor(hover, flip));
                }
            }
        }),
        Phase::Up => match GESTURE.with(|slot| slot.borrow_mut().take()) {
            Some(Gesture::Draw(start)) => {
                session.drawing.set(None);
                let rect = span(page, start, at);
                if rect.width > 2.0 && rect.height > 2.0 {
                    session.rects.update(|list| list.push(rect));
                    session
                        .selected
                        .set(Some(session.rects.with_untracked(Vec::len) - 1));
                }
            }
            Some(Gesture::Edit { index, .. }) => {
                let degenerate = session.rects.with_untracked(|rects| {
                    rects
                        .get(index)
                        .is_some_and(|rect| rect.width <= 2.0 || rect.height <= 2.0)
                });
                if degenerate {
                    session.remove_rect(index);
                }
            }
            None => {}
        },
    }
}

struct Keys {
    window: web_sys::Window,
    closure: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
}

impl Drop for Keys {
    fn drop(&mut self) {
        let _ = self
            .window
            .remove_event_listener_with_callback("keydown", self.closure.as_ref().unchecked_ref());
    }
}

/// The shortcuts a reader expects: pages, zoom, and back to fit.
pub fn bind_keys(session: Session) {
    let Some(window) = window() else { return };
    let closure =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if event.ctrl_key() || event.meta_key() || event.alt_key() {
                return;
            }
            let typing = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                .map(|element| matches!(element.tag_name().as_str(), "INPUT" | "TEXTAREA"))
                .unwrap_or(false);
            if typing || session.status.get_untracked() != Status::Ready {
                return;
            }
            let pages = session.pages.with_untracked(Vec::len);
            let page = session.page.get_untracked();
            match event.key().as_str() {
                "ArrowLeft" | "PageUp" if page > 0 => go_to(session, page - 1),
                "ArrowRight" | "PageDown" if page + 1 < pages => go_to(session, page + 1),
                "+" | "=" => {
                    session.fit.set(false);
                    session.zoom.update(|zoom| *zoom = (*zoom * 1.2).min(6.0));
                }
                "-" => {
                    session.fit.set(false);
                    session.zoom.update(|zoom| *zoom = (*zoom / 1.2).max(0.1));
                }
                "0" => session.fit.set(true),
                "Delete" | "Backspace" => match session.selected.get_untracked() {
                    Some(index) => session.remove_rect(index),
                    None => return,
                },
                _ => return,
            }
            event.prevent_default();
        });
    let _ = window.add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
    let slot: StoredValue<Option<Keys>, leptos::reactive::owner::LocalStorage> =
        StoredValue::new_local(Some(Keys { window, closure }));
    on_cleanup(move || {
        slot.try_update_value(Option::take);
    });
}
