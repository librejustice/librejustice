//! Call path to the redaction module, fetched on demand and absent from the
//! hydration bundle, so its types are restated here rather than imported.
#![cfg(feature = "hydrate")]

use js_sys::{Array, Function, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};

use super::state::PageWord;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Where one occurrence sits: `quads` on page `page`, and `start..end` in the
/// character feed the text view shows.
#[derive(Clone, Debug, Deserialize)]
pub struct Hit {
    pub needle: usize,
    pub page: Option<usize>,
    pub quads: Vec<[[f64; 2]; 4]>,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Block {
    pub start: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

/// A string the office module replaces by its label, with its family, and the
/// label the user set, `None` leaving the module to compute it. A held one is
/// not replaced, and keeps its letters from the others.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub text: String,
    pub kind: String,
    pub label: Option<String>,
    pub held: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Label {
    pub text: String,
    pub label: String,
}

/// A rectangle the viewer drew, in page coordinates.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Rect {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub struct Applied {
    pub bytes: Vec<u8>,
    pub found: usize,
    pub removed: usize,
    pub leaks: Vec<String>,
}

fn why(value: JsValue) -> String {
    value
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| value.as_string())
        .unwrap_or_else(|| "erreur inconnue".into())
}

fn get(object: &JsValue, name: &str) -> Result<JsValue, String> {
    Reflect::get(object, &JsValue::from_str(name)).map_err(why)
}

fn call(object: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let method: Function = get(object, name)?
        .dyn_into()
        .map_err(|_| format!("le module n'expose pas `{name}`"))?;
    let list = Array::new();
    for arg in args {
        list.push(arg);
    }
    Reflect::apply(&method, object, &list).map_err(why)
}

async fn awaited(name: &str) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("pas de fenêtre")?;
    let promise: js_sys::Promise = get(&window, name)?
        .dyn_into()
        .map_err(|_| "le chargeur du module n'a pas été posé".to_string())?;
    JsFuture::from(promise).await.map_err(why)
}

/// A module namespace, awaited from the promise its loader posed as `global`.
pub async fn module(global: &str) -> Result<JsValue, String> {
    awaited(global).await
}

/// A text reader, whose loader poses the same shape of promise as `global`.
pub async fn reader(global: &str) -> Result<JsValue, String> {
    awaited(global).await
}

/// One word a reader found, in the coordinates of the raster it read.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Word {
    pub page: usize,
    pub line: usize,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Point the reader's progress at `callback`, which it calls with the stage it
/// names and how far through it is. Posed once: the worker keeps it for life.
pub fn watch_progress(reader: &JsValue, callback: impl Fn(String, f64) + 'static) {
    let closure =
        wasm_bindgen::closure::Closure::wrap(Box::new(move |stage: JsValue, at: JsValue| {
            callback(
                stage.as_string().unwrap_or_default(),
                at.as_f64().unwrap_or(0.0),
            );
        }) as Box<dyn Fn(JsValue, JsValue)>);
    let _ = Reflect::set(
        reader,
        &JsValue::from_str("progress"),
        closure.as_ref().unchecked_ref(),
    );
    closure.forget();
}

/// Start the reader's workers and fetch what they read with, before any page.
pub async fn warm(reader: &JsValue) -> Result<(), String> {
    let promise: js_sys::Promise = call(reader, "prepare", &[])?
        .dyn_into()
        .map_err(|_| "le lecteur n'a pas rendu de promesse".to_string())?;
    JsFuture::from(promise).await.map(|_| ()).map_err(why)
}

/// Read one page image and hand back the words it carries.
pub async fn read(reader: &JsValue, image: &JsValue, page: usize) -> Result<Vec<Word>, String> {
    let promise: js_sys::Promise = call(
        reader,
        "words",
        &[image.clone(), JsValue::from_f64(page as f64)],
    )?
    .dyn_into()
    .map_err(|_| "le lecteur n'a pas rendu de promesse".to_string())?;
    let value = JsFuture::from(promise).await.map_err(why)?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

/// A page image the module made ready for the reader, held until its words
/// are carried back onto the source image.
pub struct Prepared {
    handle: JsValue,
}

/// Hand straight-alpha RGBA to the module to flatten, straighten and enlarge.
pub fn prepare_scan(
    module: &JsValue,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<Prepared, String> {
    let handle = call(
        module,
        "prepareScan",
        &[
            Uint8Array::from(pixels).into(),
            JsValue::from_f64(width as f64),
            JsValue::from_f64(height as f64),
        ],
    )?;
    Ok(Prepared { handle })
}

impl Prepared {
    /// Width, height and opaque RGBA of the image the reader is handed.
    pub fn image(&self) -> Result<(u32, u32, Vec<u8>), String> {
        let width = get(&self.handle, "width")?.as_f64().unwrap_or(0.0) as u32;
        let height = get(&self.handle, "height")?.as_f64().unwrap_or(0.0) as u32;
        let pixels: Uint8Array = get(&self.handle, "pixels")?
            .dyn_into()
            .map_err(|_| "la préparation n'a pas rendu de pixels".to_string())?;
        Ok((width, height, pixels.to_vec()))
    }

    /// The box on the source image that covers a word read on the prepared one.
    pub fn source_box(&self, word: &Word) -> Result<[f64; 4], String> {
        let value = call(
            &self.handle,
            "sourceBox",
            &[word.x, word.y, word.width, word.height].map(JsValue::from_f64),
        )?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        let _ = call(&self.handle, "free", &[]);
    }
}

/// A face the pages draw with: a file vendored under `/redactor/fonts`, or
/// one a Word document embeds, whose bytes the module hands over.
pub struct Face {
    pub family: String,
    pub source: Source,
}

pub enum Source {
    File(String),
    Embedded(Uint8Array),
}

fn applied(value: JsValue) -> Result<Applied, String> {
    let bytes: Uint8Array = get(&value, "bytes")?
        .dyn_into()
        .map_err(|_| "le retrait n'a rendu aucun octet".to_string())?;
    let leaks: Vec<String> =
        serde_wasm_bindgen::from_value(get(&value, "leaks")?).map_err(|e| e.to_string())?;
    Ok(Applied {
        bytes: bytes.to_vec(),
        found: get(&value, "found")?.as_f64().unwrap_or(0.0) as usize,
        removed: get(&value, "removed")?.as_f64().unwrap_or(0.0) as usize,
        leaks,
    })
}

fn strings(needles: &[String]) -> Array {
    let list = Array::new();
    for needle in needles {
        list.push(&JsValue::from_str(needle));
    }
    list
}

/// The label the module gives each entry of a Word document not held, in the
/// order given.
pub fn labels(module: &JsValue, entries: &[Entry]) -> Result<Vec<Label>, String> {
    let entries = serde_wasm_bindgen::to_value(entries).map_err(|e| e.to_string())?;
    serde_wasm_bindgen::from_value(call(module, "labels", &[entries])?).map_err(|e| e.to_string())
}

/// The picture a scanned page is made of, and the matrix that stretches its
/// unit square, top left corner at the origin, over the page in points, y down.
pub struct ScanPicture {
    pub mime: String,
    pub data: Vec<u8>,
    pub transform: [f64; 6],
}

/// A document the module holds, addressed through its handle. A PDF and a
/// Word document name what they share the same way, so one handle serves either.
#[derive(Clone)]
pub struct Document {
    handle: JsValue,
}

impl Document {
    pub fn open(module: &JsValue, bytes: &[u8]) -> Result<Self, String> {
        let buffer = Uint8Array::from(bytes);
        let handle = call(module, "open", &[buffer.into()])?;
        Ok(Self { handle })
    }

    /// The picture a scanned page is made of, as the file stores it, when one
    /// covers most of the page.
    pub fn page_scan(&self, index: usize) -> Result<Option<ScanPicture>, String> {
        let value = call(&self.handle, "pageScan", &[JsValue::from_f64(index as f64)])?;
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }
        let data: Uint8Array = get(&value, "data")?
            .dyn_into()
            .map_err(|_| "l'image de la page n'a pas été rendue".to_string())?;
        let transform: js_sys::Float64Array = get(&value, "transform")?
            .dyn_into()
            .map_err(|_| "matrice d'image invalide".to_string())?;
        let picture = ScanPicture {
            mime: get(&value, "mime")?.as_string().unwrap_or_default(),
            data: data.to_vec(),
            transform: transform
                .to_vec()
                .try_into()
                .map_err(|_| "matrice d'image invalide".to_string())?,
        };
        let _ = call(&value, "free", &[]);
        Ok(Some(picture))
    }

    /// A document for an image, which the module never parses.
    pub fn open_scan(module: &JsValue) -> Result<Self, String> {
        Ok(Self {
            handle: call(module, "openScan", &[])?,
        })
    }

    /// Replace the characters with what a reader found on the pages.
    pub fn adopt_words(&self, words: &[Word]) -> Result<usize, String> {
        let value = serde_wasm_bindgen::to_value(words).map_err(|e| e.to_string())?;
        let count = call(&self.handle, "adoptWords", &[value])?;
        Ok(count.as_f64().unwrap_or(0.0) as usize)
    }

    fn number(&self, name: &str) -> usize {
        get(&self.handle, name)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as usize
    }

    pub fn page_count(&self) -> usize {
        self.number("pageCount")
    }

    pub fn character_count(&self) -> usize {
        self.number("characterCount")
    }

    /// The pages a reader has something to find on, in a PDF.
    pub fn pages_to_read(&self) -> Result<Vec<usize>, String> {
        let value = call(&self.handle, "pagesToRead", &[])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn page_size(&self, index: usize) -> Result<PageSize, String> {
        let value = call(&self.handle, "pageSize", &[JsValue::from_f64(index as f64)])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn blocks(&self) -> Result<Vec<Block>, String> {
        let value = call(&self.handle, "blocks", &[])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn page_text(&self, index: usize) -> Result<Vec<Vec<PageWord>>, String> {
        let value = call(&self.handle, "pageText", &[JsValue::from_f64(index as f64)])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn suggestions(&self) -> Result<Vec<Suggestion>, String> {
        let value = call(&self.handle, "suggestions", &[])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn search(&self, needles: &[String]) -> Result<Vec<Hit>, String> {
        let value = call(&self.handle, "search", &[strings(needles).into()])?;
        serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
    }

    pub fn apply(&self, needles: &[String], rects: &[Rect]) -> Result<Applied, String> {
        let drawn = serde_wasm_bindgen::to_value(rects).map_err(|e| e.to_string())?;
        applied(call(
            &self.handle,
            "apply",
            &[strings(needles).into(), drawn],
        )?)
    }

    /// The faces a Word document is laid out with.
    pub fn fonts_needed(&self) -> Result<Vec<Face>, String> {
        faces(call(&self.handle, "fontsNeeded", &[])?)
    }

    /// The faces the last drawing of a PDF page writes its text in.
    pub fn page_fonts(&self, index: usize) -> Result<Vec<Face>, String> {
        faces(call(
            &self.handle,
            "pageFonts",
            &[JsValue::from_f64(index as f64)],
        )?)
    }

    /// The Adobe CMaps a PDF's text asked for and did not find.
    pub fn cmaps_needed(&self) -> Result<Vec<String>, String> {
        serde_wasm_bindgen::from_value(get(&self.handle, "cmapsNeeded")?).map_err(|e| e.to_string())
    }

    pub fn add_font(&self, file: &str, bytes: &Uint8Array) -> Result<(), String> {
        call(
            &self.handle,
            "addFont",
            &[JsValue::from_str(file), bytes.clone().into()],
        )
        .map(|_| ())
    }

    /// Break lines and pages with the faces handed over. Page count, sizes and
    /// drawings are only known after.
    pub fn layout(&self) -> Result<(), String> {
        call(&self.handle, "layout", &[]).map(|_| ())
    }

    /// One page as an SVG document whose text stays text.
    pub fn page_svg(&self, index: usize) -> Result<String, String> {
        call(&self.handle, "pageSvg", &[JsValue::from_f64(index as f64)])?
            .as_string()
            .ok_or_else(|| "le module n'a pas rendu de page".to_string())
    }

    /// Replace every occurrence in a word-processing document by its label, re-read.
    pub fn redact(&self, entries: &[Entry]) -> Result<Applied, String> {
        let entries = serde_wasm_bindgen::to_value(entries).map_err(|e| e.to_string())?;
        applied(call(&self.handle, "redact", &[entries])?)
    }
}

/// Hand an Adobe CMap to the module, for every PDF the tab opens after.
pub fn add_cmap(module: &JsValue, name: &str, bytes: &Uint8Array) -> Result<(), String> {
    call(
        module,
        "addCmap",
        &[JsValue::from_str(name), bytes.clone().into()],
    )
    .map(|_| ())
}

fn faces(list: JsValue) -> Result<Vec<Face>, String> {
    let list: Array = list
        .dyn_into()
        .map_err(|_| "le module n'a pas rendu de liste de polices".to_string())?;
    list.iter()
        .map(|entry| {
            let family = get(&entry, "family")?
                .as_string()
                .ok_or("une police sans famille")?;
            let source = match get(&entry, "file")?.as_string() {
                Some(file) => Source::File(file),
                None => Source::Embedded(
                    get(&entry, "bytes")?
                        .dyn_into()
                        .map_err(|_| format!("la police {family} n'a ni fichier ni octets"))?,
                ),
            };
            Ok(Face { family, source })
        })
        .collect()
}
