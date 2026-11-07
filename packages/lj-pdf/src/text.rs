mod appearance;
pub(crate) mod cmap;
mod embedded;
mod glyph_list;
pub(crate) mod program;
mod standard_metrics;
mod symbolic;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use crate::content::Operation;
use crate::draw::state::GraphicsState;
use crate::graphics::{Matrix, Point, Quad, Rect};
use crate::interpret::{Device, ShownGlyph, ShownImage, TransparencyGroup, walk_page};
use crate::objects::{
    ObjectRef, PageInfo, PdfDictionary, PdfError, PdfFile, PdfResult, PdfStream, PdfValue,
    decode_stream, document::get_stream,
};
use serde::{Deserialize, Serialize};

use cmap::{CMap, Code};

pub(crate) use appearance::text_string;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextItem {
    pub text: String,
    pub bbox: Rect,
    pub quad: Option<Quad>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMatch {
    pub text: String,
    pub page_index: usize,
    pub quads: Vec<Quad>,
}

#[derive(Debug, Clone)]
pub enum GlyphLocation {
    Direct {
        operand_index: usize,
        byte_start: usize,
        byte_end: usize,
    },
    Array {
        operand_index: usize,
        element_index: usize,
        byte_start: usize,
        byte_end: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub text: char,
    pub bbox: Rect,
    pub quad: Quad,
    pub page_char_index: usize,
    pub operation_index: usize,
    pub location: GlyphLocation,
    /// False when the glyph was rendered with Tr=3 (invisible mode).
    /// Invisible glyphs are still included for redaction but excluded from
    /// search results and extracted text items.
    pub visible: bool,
    /// Raw advance width in 1/1000 em font units. Used by kern-compensating
    /// redaction modes to preserve text positioning after byte removal.
    pub width_units: f64,
    /// When `Some`, the glyph originated inside a Form XObject invoked by
    /// `Do` on the page's content stream. `operation_index` and `location`
    /// then refer to positions inside that Form's content stream, not the
    /// page's, callers that want to rewrite the bytes must look up the
    /// corresponding Form object, decode its stream, and mutate the
    /// operations of the *Form*, not the page.
    pub source_form: Option<ObjectRef>,
}

#[derive(Debug, Clone)]
pub struct ExtractedPageText {
    pub page_index: usize,
    pub text: String,
    pub items: Vec<TextItem>,
    pub glyphs: Vec<Glyph>,
}

#[derive(Debug, Clone)]
pub struct PageSearchIndex {
    normalized_text: String,
    normalized_to_display: Vec<usize>,
    display_chars: Vec<char>,
    display_to_glyph: Vec<Option<usize>>,
}

pub fn analyze_page_text(
    file: &PdfFile,
    page_index: usize,
    page: &PageInfo,
) -> PdfResult<ExtractedPageText> {
    let context = TextContext::new(page.page_box.normalized_transform());
    let mut context = walk_page(file, page, context)?;
    apply_structure_texts(file, page.page_ref, &mut context);
    Ok(ExtractedPageText {
        page_index,
        text: context.text,
        items: context.items,
        glyphs: context.glyphs,
    })
}

pub fn search_page_text(page: &ExtractedPageText, query: &str) -> Vec<TextMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let index = build_search_index(page);
    let normalized_query = normalize_search_text(query);
    if normalized_query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut search_offset = 0usize;
    while let Some(position) = index.normalized_text[search_offset..].find(&normalized_query) {
        let normalized_start = search_offset + position;
        let normalized_end = normalized_start + normalized_query.len();
        let display_start = *index
            .normalized_to_display
            .get(normalized_start)
            .unwrap_or(&0);
        let display_end = index
            .normalized_to_display
            .get(normalized_end.saturating_sub(1))
            .copied()
            .unwrap_or(display_start)
            + 1;
        let glyph_indices = (display_start..display_end)
            .filter_map(|display_index| index.normalized_to_glyph(display_index))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let quads = coalesce_match_quads(
            &glyph_indices
                .iter()
                .filter_map(|glyph_index| page.glyphs.get(*glyph_index))
                .map(|glyph| glyph.quad)
                .collect::<Vec<_>>(),
        );
        if !quads.is_empty() {
            matches.push(TextMatch {
                text: index
                    .display_chars
                    .iter()
                    .skip(display_start)
                    .take(display_end.saturating_sub(display_start))
                    .collect::<String>()
                    .trim()
                    .to_string(),
                page_index: page.page_index,
                quads,
            });
        }
        search_offset = normalized_end;
        if search_offset >= index.normalized_text.len() {
            break;
        }
    }
    matches
}

fn coalesce_match_quads(quads: &[Quad]) -> Vec<Quad> {
    let mut rects = quads
        .iter()
        .map(|quad| quad.bounding_rect())
        .collect::<Vec<_>>();
    rects.sort_by(|left, right| {
        let y_delta = (left.y - right.y).abs();
        if y_delta > 1.5 {
            right
                .y
                .partial_cmp(&left.y)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            left.x
                .partial_cmp(&right.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut merged = Vec::<Rect>::new();
    for rect in rects {
        let Some(previous) = merged.last_mut() else {
            merged.push(rect);
            continue;
        };
        if should_merge_match_rects(*previous, rect) {
            let next_x = previous.x.min(rect.x);
            let next_y = previous.y.min(rect.y);
            let next_max_x = previous.max_x().max(rect.max_x());
            let next_max_y = previous.max_y().max(rect.max_y());
            *previous = Rect {
                x: next_x,
                y: next_y,
                width: next_max_x - next_x,
                height: next_max_y - next_y,
            };
        } else {
            merged.push(rect);
        }
    }

    merged
        .into_iter()
        .map(expand_match_rect)
        .map(Rect::to_quad)
        .collect()
}

fn should_merge_match_rects(left: Rect, right: Rect) -> bool {
    let vertical_overlap = left.max_y().min(right.max_y()) - left.y.max(right.y);
    let minimum_height = left.height.min(right.height).max(1.0);
    let horizontal_gap = right.x - left.max_x();
    vertical_overlap >= minimum_height * 0.45 && horizontal_gap <= minimum_height * 0.8
}

fn expand_match_rect(rect: Rect) -> Rect {
    // Return the rect as-is without padding. Padding for visual highlighting
    // should be applied at the UI layer. Using padded geometry for redaction
    // causes over-selection in densely-packed CJK text.
    rect
}

fn build_search_index(page: &ExtractedPageText) -> PageSearchIndex {
    let (display_chars, display_to_glyph) = build_visual_display(page);
    let mut normalized_text = String::new();
    let mut normalized_to_display = Vec::new();
    let mut previous_was_whitespace = false;
    for (display_index, character) in display_chars.iter().copied().enumerate() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                normalized_text.push(' ');
                // One entry per UTF-8 byte so byte offsets from str::find() map correctly
                for _ in 0..' '.len_utf8() {
                    normalized_to_display.push(display_index);
                }
                previous_was_whitespace = true;
            }
        } else {
            for folded in character.to_lowercase() {
                normalized_text.push(folded);
                for _ in 0..folded.len_utf8() {
                    normalized_to_display.push(display_index);
                }
            }
            previous_was_whitespace = false;
        }
    }
    PageSearchIndex {
        normalized_text,
        normalized_to_display,
        display_chars,
        display_to_glyph,
    }
}

impl PageSearchIndex {
    fn normalized_to_glyph(&self, display_index: usize) -> Option<usize> {
        self.display_to_glyph.get(display_index).copied().flatten()
    }
}

fn build_visual_display(page: &ExtractedPageText) -> (Vec<char>, Vec<Option<usize>>) {
    let mut lines = build_visual_lines(page);
    for line in &mut lines {
        line.sort_by(|left, right| {
            page.glyphs[*left]
                .bbox
                .x
                .partial_cmp(&page.glyphs[*right].bbox.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    lines.sort_by(|left, right| {
        let left_y = average_line_center_y(page, left);
        let right_y = average_line_center_y(page, right);
        right_y
            .partial_cmp(&left_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut display_chars = Vec::new();
    let mut display_to_glyph = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if line_index > 0 {
            display_chars.push('\n');
            display_to_glyph.push(None);
        }
        let mut previous_rect: Option<Rect> = None;
        for glyph_index in line {
            let glyph = &page.glyphs[*glyph_index];
            if let Some(previous) = previous_rect {
                let gap = glyph.bbox.x - previous.max_x();
                let threshold = previous.height.min(glyph.bbox.height).max(1.0) * 0.3;
                if gap > threshold {
                    display_chars.push(' ');
                    display_to_glyph.push(None);
                }
            }
            display_chars.push(glyph.text);
            display_to_glyph.push(Some(*glyph_index));
            previous_rect = Some(glyph.bbox);
        }
    }
    (display_chars, display_to_glyph)
}

/// Per-line aggregates maintained during the greedy grouping pass.
/// `anchor_y` is the y-centre of the first glyph placed on the line
/// and never changes, using a fixed reference instead of a running
/// mean prevents accumulated drift letting near-boundary glyphs from
/// adjacent rows creep in. `height_ref` is the maximum glyph bbox
/// height seen so far; updates are O(1) on each join.
struct LineCluster {
    indices: Vec<usize>,
    anchor_y: f64,
    height_ref: f64,
}

fn build_visual_lines(page: &ExtractedPageText) -> Vec<Vec<usize>> {
    let mut indices = (0..page.glyphs.len())
        .filter(|i| page.glyphs[*i].visible)
        .collect::<Vec<_>>();
    // Deterministic sort: primary y descending, secondary x ascending,
    // with no threshold-based bucketing. A tolerant bucket (e.g. 1.5 pt)
    // produced an intransitive comparator, two close-but-distinct lines
    // would interleave in the sorted output, scrambling their characters
    // once per-line x-sorting ran. Line grouping itself is handled by
    // the anchor-based check below, which tolerates intra-line y jitter
    // from subscripts or baseline adjustments via the proportional
    // `height_ref * 0.10` window.
    indices.sort_by(|left, right| {
        let left_center = glyph_center_y(&page.glyphs[*left]);
        let right_center = glyph_center_y(&page.glyphs[*right]);
        right_center
            .partial_cmp(&left_center)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                page.glyphs[*left]
                    .bbox
                    .x
                    .partial_cmp(&page.glyphs[*right].bbox.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut clusters: Vec<LineCluster> = Vec::new();
    for glyph_index in indices {
        let glyph = &page.glyphs[glyph_index];
        let glyph_center = glyph_center_y(glyph);
        let glyph_height = glyph.bbox.height;
        let glyph_x = glyph.bbox.x;

        let target = clusters.iter_mut().find(|cluster| {
            let last_x = cluster.indices.last().map(|i| page.glyphs[*i].bbox.x);
            glyph_fits_line(cluster, glyph_center, glyph_height, glyph_x, last_x)
        });

        match target {
            Some(cluster) => {
                cluster.indices.push(glyph_index);
                if glyph_height > cluster.height_ref {
                    cluster.height_ref = glyph_height;
                }
            }
            None => clusters.push(LineCluster {
                indices: vec![glyph_index],
                anchor_y: glyph_center,
                // Floor at 1pt to keep the tolerance non-degenerate when
                // the first glyph happens to be zero-height (defensive
                // only; real glyphs always have positive bbox height).
                height_ref: glyph_height.max(1.0),
            }),
        }
    }
    clusters.into_iter().map(|c| c.indices).collect()
}

/// Anchor-based line membership check. A glyph fits the line iff its
/// y-centre is within `height_ref * 0.10` of the line's fixed anchor,
/// AND its bbox.x does not fall meaningfully behind the most-recently
/// placed glyph's bbox.x (the x-monotonicity guard, unchanged from the
/// pre-existing logic). The `1e-6` epsilon keeps rows exactly at the
/// boundary distance on the split side.
fn glyph_fits_line(
    cluster: &LineCluster,
    glyph_center: f64,
    glyph_height: f64,
    glyph_bbox_x: f64,
    last_bbox_x: Option<f64>,
) -> bool {
    let new_height_ref = cluster.height_ref.max(glyph_height);
    let y_tolerance = new_height_ref * 0.10;
    if (glyph_center - cluster.anchor_y).abs() + 1e-6 >= y_tolerance {
        return false;
    }
    // X-monotonicity guard. A row of text has glyphs whose starting
    // x coordinates ascend monotonically; a new glyph whose start x
    // sits meaningfully before the last-encountered glyph's start x
    // is almost certainly on a different visual row that happens to
    // be only a point or two apart in y. Use `bbox.x` of each glyph
    // (not `bbox.max_x()`) because aggressive TJ-array kerning in
    // compressed text blocks makes successive glyph bboxes overlap
    // heavily; the start-x of the most-recently appended glyph is a
    // reliable "line watermark" thanks to the (y-desc, x-asc) feeder
    // sort.
    if let Some(last_x) = last_bbox_x {
        let kerning_slack = 1.0;
        if glyph_bbox_x + kerning_slack < last_x {
            return false;
        }
    }
    true
}

fn average_line_center_y(page: &ExtractedPageText, line: &[usize]) -> f64 {
    let total = line
        .iter()
        .map(|glyph_index| glyph_center_y(&page.glyphs[*glyph_index]))
        .sum::<f64>();
    total / line.len().max(1) as f64
}

fn glyph_center_y(glyph: &Glyph) -> f64 {
    glyph.bbox.y + glyph.bbox.height / 2.0
}

fn normalize_search_text(input: &str) -> String {
    let mut output = String::new();
    let mut previous_was_whitespace = false;
    for character in input.chars() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                output.push(' ');
                previous_was_whitespace = true;
            }
        } else {
            for folded in character.to_lowercase() {
                output.push(folded);
            }
            previous_was_whitespace = false;
        }
    }
    output.trim().to_string()
}

pub(crate) enum Appearance {
    Stream(ObjectRef),
    Drawn(ObjectRef, PdfStream),
}

/// The normal appearance of each annotation a viewer draws, with its Rect.
pub(crate) fn annotation_appearances(file: &PdfFile, page: &PageInfo) -> Vec<(Appearance, Rect)> {
    const HIDDEN: i64 = 1 << 1;
    const NO_VIEW: i64 = 1 << 5;
    let need_appearances = file
        .trailer
        .get("Root")
        .and_then(|root| file.resolve_dict(root).ok())
        .and_then(|root| root.get("AcroForm"))
        .and_then(|form| file.resolve_dict(form).ok())
        .and_then(|form| form.get("NeedAppearances"))
        == Some(&PdfValue::Bool(true));
    let mut appearances = Vec::new();
    for annotation_ref in &page.annotation_refs {
        let Ok(annotation) = file.get_dictionary(*annotation_ref) else {
            continue;
        };
        let flags = annotation
            .get("F")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        if flags & (HIDDEN | NO_VIEW) != 0 {
            continue;
        }
        let Some(rect) = annotation
            .get("Rect")
            .and_then(|v| file.resolve(v).ok())
            .and_then(|v| crate::objects::document::parse_rect(v).ok())
        else {
            continue;
        };
        let rect = rect.normalize();
        let normal = annotation
            .get("AP")
            .and_then(|v| file.resolve_dict(v).ok())
            .and_then(|ap| ap.get("N"));
        let field = annotation.get("Subtype").and_then(PdfValue::as_name) == Some("Widget");
        let drawn = || appearance::drawn(file, annotation, rect.width, rect.height);
        let Some(normal) = normal.filter(|_| !(need_appearances && field)) else {
            if let Some(stream) = drawn() {
                appearances.push((Appearance::Drawn(*annotation_ref, stream), rect));
            }
            continue;
        };
        let stream_ref = match normal {
            PdfValue::Reference(reference) if get_stream(file, *reference).is_ok() => {
                Some(*reference)
            }
            other => {
                let state = annotation.get("AS").and_then(PdfValue::as_name);
                file.resolve_dict(other)
                    .ok()
                    .and_then(|states| states.get(state?))
                    .and_then(|v| match v {
                        PdfValue::Reference(reference) => Some(*reference),
                        _ => None,
                    })
            }
        };
        if let Some(stream_ref) = stream_ref {
            appearances.push((Appearance::Stream(stream_ref), rect));
        }
    }
    appearances
}

pub(crate) fn lookup_xobject_ref(
    file: &PdfFile,
    resources: &PdfDictionary,
    name: &str,
) -> Option<ObjectRef> {
    let xobjects = file.resolve_dict(resources.get("XObject")?).ok()?;
    match xobjects.get(name)? {
        PdfValue::Reference(object_ref) => Some(*object_ref),
        _ => None,
    }
}

pub(crate) fn matrix_from_pdf_values(values: &[PdfValue]) -> PdfResult<Matrix> {
    if values.len() != 6 {
        return Err(PdfError::Corrupt(
            "Matrix array must have six numeric entries".to_string(),
        ));
    }
    let mut numbers = [0.0; 6];
    for (slot, value) in numbers.iter_mut().zip(values.iter()) {
        *slot = value
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("Matrix entry is not a number".to_string()))?;
    }
    Ok(Matrix {
        a: numbers[0],
        b: numbers[1],
        c: numbers[2],
        d: numbers[3],
        e: numbers[4],
        f: numbers[5],
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ShowOperand {
    Direct {
        operand_index: usize,
    },
    Array {
        operand_index: usize,
        element_index: usize,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SimpleFont {
    widths: Vec<f64>,
    first_char: u16,
    /// The standard font whose metrics stand in for a missing width: the
    /// font itself when it is one of the 14, a substitute otherwise.
    metrics: StandardMetrics,
    face: Face,
    missing_width: f64,
    /// A Type3 font's widths are in glyph space, which its FontMatrix maps
    /// to text space; every other font's are in thousandths.
    width_scale: f64,
    to_unicode: Option<std::sync::Arc<CMap>>,
    encoding: SimpleEncoding,
    /// What the embedded program's own encoding draws, read when the font
    /// names no base encoding.
    builtin: std::sync::Arc<BTreeMap<u8, String>>,
    /// Characters recovered from `Differences` names outside the glyph list.
    recovered: BTreeMap<u8, char>,
    /// What draws the glyphs, read only for a drawing; `None` when the file
    /// embeds nothing to draw them with.
    glyphs: Option<program::SimpleGlyphs>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct SimpleEncoding {
    base: SimpleEncodingBase,
    /// Byte-level overrides from an `/Encoding` dictionary's `/Differences`
    /// array. Keys are byte codes; values are PDF glyph names (e.g. `"Adieresis"`).
    differences: BTreeMap<u8, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SimpleEncodingBase {
    /// Bytes read as ASCII, anything above as unknown.
    #[default]
    Identity,
    WinAnsi,
    MacRoman,
    Standard,
    Symbol,
    Dingbats,
}

#[derive(Debug, Clone)]
pub(crate) struct CompositeFont {
    encoding: CMap,
    face: Face,
    /// The codes are Unicode, as in the `Uni*-UCS2`, `-UTF16` and `-UTF32`
    /// predefined CMaps and a ToUnicode named `Identity-H`.
    unicode_codes: bool,
    default_width: f64,
    widths: BTreeMap<u32, f64>,
    default_vertical: VerticalMetrics,
    vertical_metrics: BTreeMap<u32, VerticalMetrics>,
    to_unicode: Option<std::sync::Arc<CMap>>,
    /// The Adobe registry and ordering the CIDs are numbered in.
    collection: Option<(String, String)>,
    glyph_text: std::sync::Arc<BTreeMap<u16, char>>,
    /// `None` when the CIDs are the glyph ids.
    cid_to_gid: Option<Vec<u16>>,
    /// A TrueType the file does not embed, whose glyph ids a producer took
    /// from the standard Macintosh order.
    mac_order: bool,
    /// The embedded program, read only for a drawing.
    program: Option<std::sync::Arc<program::Program>>,
}

/// Vertical advance and the position of the horizontal origin, from the
/// vertical one, in thousandths of the text size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VerticalMetrics {
    pub advance: f64,
    pub origin_x: Option<f64>,
    pub origin_y: f64,
}

const DEFAULT_VERTICAL: VerticalMetrics = VerticalMetrics {
    advance: -1000.0,
    origin_x: None,
    origin_y: 880.0,
};

#[derive(Debug, Clone)]
pub(crate) enum LoadedFont {
    Simple(SimpleFont),
    Composite(CompositeFont),
    /// A font that could not be read: each code still takes its place on
    /// the page, as an unknown character.
    Unreadable {
        code_len: usize,
    },
}

impl LoadedFont {
    pub(crate) fn vertical(&self) -> bool {
        matches!(self, LoadedFont::Composite(font) if font.encoding.vertical)
    }

    pub(crate) fn face(&self) -> Face {
        match self {
            LoadedFont::Simple(font) => font.face,
            LoadedFont::Composite(font) => font.face,
            LoadedFont::Unreadable { .. } => Face::default(),
        }
    }

    /// MuPDF shows text drawn with no font, or a font the resources lack, in
    /// Times.
    pub(crate) fn fallback() -> LoadedFont {
        LoadedFont::Simple(SimpleFont {
            widths: Vec::new(),
            first_char: 0,
            metrics: StandardMetrics::TimesRoman,
            face: Face {
                serif: true,
                ..Face::default()
            },
            missing_width: 0.0,
            width_scale: 1.0,
            to_unicode: None,
            encoding: SimpleEncoding {
                base: SimpleEncodingBase::Standard,
                differences: BTreeMap::new(),
            },
            builtin: Default::default(),
            recovered: BTreeMap::new(),
            glyphs: None,
        })
    }

    /// What draws a decoded glyph.
    pub(crate) fn drawn(&self, glyph: &DecodedGlyph) -> program::Drawn<'_> {
        use program::{Drawn, SimpleGlyphs};
        match self {
            LoadedFont::Simple(font) => match &font.glyphs {
                None => Drawn::Text,
                Some(SimpleGlyphs::Program { program, codes }) => {
                    match codes.get(glyph.code as usize).copied().flatten() {
                        Some(gid) => Drawn::Outline(program, gid),
                        None => Drawn::Nothing,
                    }
                }
                Some(SimpleGlyphs::Type3(font)) => match font.procs.get(&(glyph.code as u8)) {
                    Some(procedure) => Drawn::Procedure(font, *procedure),
                    None => Drawn::Nothing,
                },
            },
            LoadedFont::Composite(font) => match (&font.program, glyph.cid) {
                (None, _) => Drawn::Text,
                (Some(program), Some(cid)) => {
                    match program.cid_glyph(cid, font.cid_to_gid.as_deref()) {
                        Some(gid) => Drawn::Outline(program, gid),
                        None => Drawn::Nothing,
                    }
                }
                // A code the encoding CMap does not read, where MuPDF reads
                // more CMaps than the text does.
                (Some(_), None) => Drawn::Text,
            },
            LoadedFont::Unreadable { .. } => Drawn::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedGlyph {
    pub text: String,
    /// The character code, and the CID it selects in a composite font.
    pub code: u32,
    pub cid: Option<u32>,
    pub width_units: f64,
    pub byte_start: usize,
    pub byte_end: usize,
    /// Word spacing applies to the single-byte code 32 alone.
    pub word_space: bool,
    pub vertical: Option<VerticalMetrics>,
}

struct TextContext {
    page_transform: Matrix,
    text: String,
    /// The characters of `text`, kept beside it: counted per glyph, they made
    /// reading a page quadratic in its length.
    text_chars: usize,
    items: Vec<TextItem>,
    glyphs: Vec<Glyph>,
    pending_line_break: bool,
    /// The Form XObject reference whose content is currently being
    /// interpreted. `None` while the page's own content stream is active.
    /// Glyphs pushed while this is `Some(_)` carry the same reference in
    /// their `source_form` field so downstream consumers (redact, in
    /// particular) can identify which stream holds the glyph's bytes.
    current_form: Option<ObjectRef>,
    /// The `current_form` of each form the walk is inside of.
    outer_forms: Vec<Option<ObjectRef>>,
    marked: Vec<Marked>,
    /// The glyphs of each marked sequence of the page's own content that
    /// carries a marked-content identifier: `(mcid, start, end)`.
    mcid_ranges: Vec<MarkedRange>,
    /// The images the page's own content draws: `(operation_index, quad)`.
    images: Vec<(usize, Quad)>,
    show: Option<Show>,
}

/// The string being shown, which becomes one text item.
struct Show {
    operation_index: usize,
    operand: ShowOperand,
    item_start: usize,
    item_quad: Option<Rect>,
    item_text: String,
}

impl TextContext {
    fn new(page_transform: Matrix) -> Self {
        Self {
            page_transform,
            text: String::new(),
            text_chars: 0,
            items: Vec::new(),
            glyphs: Vec::new(),
            pending_line_break: false,
            current_form: None,
            outer_forms: Vec::new(),
            marked: Vec::new(),
            mcid_ranges: Vec::new(),
            images: Vec::new(),
            show: None,
        }
    }
}

impl Device for TextContext {
    const DRAWS: bool = false;

    fn line_break(&mut self) {
        self.pending_line_break = true;
    }

    fn begin_marked(&mut self, file: &PdfFile, resources: &PdfDictionary, operation: &Operation) {
        if operation.operator == "BMC" {
            self.marked.push(Marked {
                start: self.glyphs.len(),
                images: self.images.len(),
                actual: None,
                mcid: None,
            });
            return;
        }
        let open = self.marked.iter().any(|marked| marked.actual.is_some());
        let properties = marked_properties(file, resources, operation);
        let actual = properties
            .filter(|_| !open)
            .and_then(|properties| properties.get("ActualText"))
            .and_then(|value| match file.resolve(value).ok()? {
                PdfValue::String(string) => Some(text_string(&string.0)),
                _ => None,
            });
        let mcid = properties
            .filter(|_| self.current_form.is_none())
            .and_then(|properties| properties.get("MCID"))
            .and_then(PdfValue::as_integer);
        self.marked.push(Marked {
            start: self.glyphs.len(),
            images: self.images.len(),
            actual,
            mcid,
        });
    }

    fn end_marked(&mut self) {
        if let Some(marked) = self.marked.pop() {
            if let Some(text) = &marked.actual {
                replace_with_actual_text(self, marked.start, text);
            }
            if let Some(mcid) = marked.mcid {
                self.mcid_ranges.push(MarkedRange {
                    mcid,
                    glyphs: (marked.start, self.glyphs.len()),
                    images: (marked.images, self.images.len()),
                });
            }
        }
    }

    fn begin_show(&mut self, operation_index: usize, operand: ShowOperand) {
        if std::mem::take(&mut self.pending_line_break) && !self.text.is_empty() {
            self.text.push('\n');
            self.text_chars += 1;
        }
        self.show = Some(Show {
            operation_index,
            operand,
            item_start: self.text_chars,
            item_quad: None,
            item_text: String::new(),
        });
    }

    fn glyph(&mut self, glyph: &ShownGlyph<'_>, state: &GraphicsState) {
        let Some(show) = self.show.as_mut() else {
            return;
        };
        let quad = glyph.local_rect.to_quad().transform(glyph.text_to_page);
        let bbox = quad.bounding_rect();
        show.item_quad = Some(match show.item_quad {
            Some(existing) => existing.union(&bbox),
            None => bbox,
        });
        let decoded = glyph.decoded;
        for character in decoded.text.chars() {
            let page_char_index = self.text_chars;
            let visible = state.text.text_render_mode != 3;
            self.glyphs.push(Glyph {
                text: character,
                bbox,
                quad,
                page_char_index,
                operation_index: show.operation_index,
                source_form: self.current_form,
                location: match show.operand {
                    ShowOperand::Direct { operand_index } => GlyphLocation::Direct {
                        operand_index,
                        byte_start: decoded.byte_start,
                        byte_end: decoded.byte_end,
                    },
                    ShowOperand::Array {
                        operand_index,
                        element_index,
                    } => GlyphLocation::Array {
                        operand_index,
                        element_index,
                        byte_start: decoded.byte_start,
                        byte_end: decoded.byte_end,
                    },
                },
                visible,
                width_units: decoded.width_units,
            });
            if visible {
                self.text.push(character);
                self.text_chars += 1;
                show.item_text.push(character);
            }
        }
    }

    fn end_show(&mut self) {
        let Some(show) = self.show.take() else {
            return;
        };
        if !show.item_text.is_empty() {
            let item_end = self.text_chars;
            self.items.push(TextItem {
                text: show.item_text,
                bbox: show.item_quad.unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                }),
                quad: show.item_quad.map(Rect::to_quad),
                char_start: Some(show.item_start),
                char_end: Some(item_end),
            });
        }
    }

    fn image(&mut self, _file: &PdfFile, image: &ShownImage<'_>, state: &GraphicsState) {
        if self.current_form.is_none() {
            let quad = Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }
            .to_quad()
            .transform(state.ctm.multiply(self.page_transform));
            self.images.push((image.operation_index, quad));
        }
    }

    fn begin_form(
        &mut self,
        form: ObjectRef,
        _group: Option<TransparencyGroup>,
        _state: &GraphicsState,
    ) {
        self.outer_forms.push(self.current_form);
        self.current_form = Some(form);
    }

    fn end_form(&mut self) {
        self.current_form = self.outer_forms.pop().flatten();
    }
}

fn load_single_font(
    file: &PdfFile,
    font_dict: &crate::objects::PdfDictionary,
    draws: bool,
) -> LoadedFont {
    let subtype = font_dict
        .get("Subtype")
        .and_then(PdfValue::as_name)
        .unwrap_or("");
    let loaded = match subtype {
        "Type0" => load_composite_font(file, font_dict, draws).map(LoadedFont::Composite),
        _ => Ok(LoadedFont::Simple(load_simple_font(
            file, font_dict, subtype, draws,
        ))),
    };
    loaded.unwrap_or(LoadedFont::Unreadable {
        code_len: if subtype == "Type0" { 2 } else { 1 },
    })
}

fn load_simple_font(
    file: &PdfFile,
    font_dict: &crate::objects::PdfDictionary,
    subtype: &str,
    draws: bool,
) -> SimpleFont {
    let first_char = font_dict
        .get("FirstChar")
        .and_then(|v| file.resolve(v).ok())
        .and_then(PdfValue::as_integer)
        .unwrap_or(0)
        .clamp(0, 255) as u16;
    let widths = font_dict
        .get("Widths")
        .and_then(|v| file.resolve(v).ok())
        .and_then(PdfValue::as_array)
        .map(|widths| {
            widths
                .iter()
                .map(|value| {
                    file.resolve(value)
                        .ok()
                        .and_then(PdfValue::as_number)
                        .unwrap_or(0.0)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let descriptor = font_dict
        .get("FontDescriptor")
        .and_then(|v| file.resolve_dict(v).ok());
    let flags = descriptor
        .and_then(|d| d.get("Flags"))
        .and_then(PdfValue::as_integer)
        .unwrap_or(0);
    let missing_width = descriptor
        .and_then(|d| d.get("MissingWidth"))
        .and_then(PdfValue::as_number)
        .unwrap_or(0.0);
    let base_font = font_dict
        .get("BaseFont")
        .and_then(PdfValue::as_name)
        .unwrap_or("");
    let metrics = StandardMetrics::for_font(base_font, flags);
    let face = Face::for_font(base_font, flags);
    let mut encoding = parse_simple_encoding(file, font_dict);
    let builtin = match (has_named_base(file, font_dict), descriptor) {
        (false, Some(descriptor)) => embedded::builtin_text(file, descriptor),
        _ => Default::default(),
    };
    let type3 = subtype == "Type3";
    let glyphs = match (draws, type3, descriptor) {
        (false, _, _) => None,
        (true, true, _) => Some(program::SimpleGlyphs::Type3(std::sync::Arc::new(
            program::Type3::load(file, font_dict, &encoding.differences),
        ))),
        (true, false, Some(descriptor)) => {
            program::Program::load(file, descriptor).map(|program| {
                let base = has_named_base(file, font_dict).then_some(encoding.base);
                let codes = program.code_glyphs(&encoding.differences, base, flags & 4 != 0);
                program::SimpleGlyphs::Program {
                    program,
                    codes: codes.into(),
                }
            })
        }
        (true, false, None) => None,
    };
    if type3 {
        if encoding.base == SimpleEncodingBase::Identity && !has_named_base(file, font_dict) {
            encoding.base = SimpleEncodingBase::Standard;
        }
    } else if encoding.base == SimpleEncodingBase::Identity && !has_named_base(file, font_dict) {
        // A font that names no base encoding uses its built-in one, which
        // for a text font is StandardEncoding.
        let name = base_font.rsplit('+').next().unwrap_or(base_font);
        if name.starts_with("Symbol") {
            encoding.base = SimpleEncodingBase::Symbol;
        } else if name.contains("Dingbats") {
            encoding.base = SimpleEncodingBase::Dingbats;
        } else if !matches!(metrics, StandardMetrics::Symbol | StandardMetrics::Dingbats) {
            encoding.base = SimpleEncodingBase::Standard;
        }
    }
    let width_scale = match type3 {
        true => font_dict
            .get("FontMatrix")
            .and_then(|v| file.resolve(v).ok())
            .and_then(PdfValue::as_array)
            .and_then(|m| m.first())
            .and_then(PdfValue::as_number)
            .map_or(1.0, |a| a * 1000.0),
        false => 1.0,
    };
    SimpleFont {
        widths,
        first_char,
        metrics,
        face,
        missing_width,
        width_scale,
        to_unicode: load_to_unicode(file, font_dict).and_then(|u| match u {
            ToUnicode::Map(map) => Some(map),
            ToUnicode::Identity => None,
        }),
        recovered: {
            let mut recovered = recover_glyph_names(&encoding.differences);
            // A Type3 glyph name outside the glyph list is the procedure's own
            // label: viewers read its code in the base encoding.
            if type3 {
                for code in encoding.differences.keys() {
                    if let Some(character) = base_char(encoding.base, *code) {
                        recovered.entry(*code).or_insert(character);
                    }
                }
            }
            recovered
        },
        encoding,
        builtin,
        glyphs,
    }
}

fn numbered(name: &str) -> Option<&str> {
    let digits = name.strip_prefix(['C', 'c'])?;
    (2..=3).contains(&digits.len()).then_some(digits)
}

/// Names a generator made from the code, as `a65`, `g0041`, `G41` or `C65`
/// (`C41`, in hexadecimal, when that reads the codes better).
fn recover_glyph_names(differences: &BTreeMap<u8, String>) -> BTreeMap<u8, char> {
    let unknown = || {
        differences
            .iter()
            .filter(|(_, name)| glyph_name_to_text(name).is_none())
    };
    let matches = |radix| {
        unknown()
            .filter(|(code, name)| {
                numbered(name).and_then(|d| u32::from_str_radix(d, radix).ok())
                    == Some(u32::from(**code))
            })
            .count()
    };
    let c_hex = unknown()
        .any(|(_, name)| numbered(name).is_some_and(|d| d.chars().any(|c| !c.is_ascii_digit())))
        || matches(16) > matches(10);
    unknown()
        .filter_map(|(code, name)| {
            let (digits, radix) = match name.as_bytes() {
                [b'G', _, _] => (&name[1..], 16),
                [b'g', _, _, _, _] => (&name[1..], 16),
                [b'a', _, ..] => (&name[1..], 10),
                _ => (numbered(name)?, if c_hex { 16 } else { 10 }),
            };
            let character = char::from_u32(u32::from_str_radix(digits, radix).ok()?)?;
            (!character.is_control()).then_some((*code, character))
        })
        .collect()
}

/// The fonts a resource dictionary names, each read the first time the content
/// selects it: a dictionary shared by every page may name thousands. A name
/// the dictionary lacks falls back to `parent`'s. `draws` also reads what
/// draws each glyph, which the text alone does not need.
pub(crate) struct Fonts<'a> {
    file: &'a PdfFile,
    fonts: Option<&'a PdfDictionary>,
    states: Option<&'a PdfDictionary>,
    draws: bool,
    parent: Option<&'a Fonts<'a>>,
    loaded: RefCell<HashMap<String, Option<Rc<LoadedFont>>>>,
}

impl<'a> Fonts<'a> {
    pub(crate) fn new(
        file: &'a PdfFile,
        resources: &'a PdfDictionary,
        draws: bool,
        parent: Option<&'a Fonts<'a>>,
    ) -> PdfResult<Self> {
        let fonts = resources
            .get("Font")
            .map(|value| file.resolve_dict(value))
            .transpose()?;
        let states = resources
            .get("ExtGState")
            .and_then(|value| file.resolve_dict(value).ok());
        Ok(Self {
            file,
            fonts,
            states,
            draws,
            parent,
            loaded: RefCell::default(),
        })
    }

    /// The font under `key`: a name of the Font dictionary, or the key
    /// [`Fonts::state_font`] returns.
    pub(crate) fn get(&self, key: &str) -> Option<Rc<LoadedFont>> {
        if let Some(font) = self.loaded.borrow().get(key) {
            return font
                .clone()
                .or_else(|| self.parent.and_then(|p| p.get(key)));
        }
        let own = match key.strip_prefix(STATE_FONT) {
            Some(state) => self.state_entry(state).map(|(dict, _)| dict),
            None => self
                .fonts
                .and_then(|fonts| fonts.get(key))
                .and_then(|value| self.file.resolve_dict(value).ok()),
        }
        .map(|dict| Rc::new(load_single_font(self.file, dict, self.draws)));
        self.loaded
            .borrow_mut()
            .insert(key.to_string(), own.clone());
        own.or_else(|| self.parent.and_then(|p| p.get(key)))
    }

    /// The font key and size the ExtGState `name` selects, if it sets a font.
    pub(crate) fn state_font(&self, name: &str) -> Option<(String, f64)> {
        match self.state_entry(name) {
            Some((_, size)) => Some((format!("{STATE_FONT}{name}"), size)),
            None => self.parent.and_then(|p| p.state_font(name)),
        }
    }

    fn state_entry(&self, name: &str) -> Option<(&'a PdfDictionary, f64)> {
        let state = self.file.resolve_dict(self.states?.get(name)?).ok()?;
        let font = state.get("Font").and_then(PdfValue::as_array)?;
        if font.len() < 2 {
            return None;
        }
        let dict = self.file.resolve_dict(&font[0]).ok()?;
        Some((dict, font[1].as_number().unwrap_or(12.0)))
    }
}

const STATE_FONT: &str = "__gs:";

/// An image drawn in a marked sequence: the glyph position the sequence starts
/// at, the drawing operation and where it lands.
type PlacedImage = (usize, (usize, Quad));

struct MarkedRange {
    mcid: i64,
    glyphs: (usize, usize),
    images: (usize, usize),
}

struct Marked {
    start: usize,
    images: usize,
    /// The `/ActualText` that replaces the sequence's glyphs.
    actual: Option<String>,
    mcid: Option<i64>,
}

fn marked_properties<'a>(
    file: &'a PdfFile,
    resources: &'a PdfDictionary,
    operation: &'a Operation,
) -> Option<&'a PdfDictionary> {
    match operation.operands.get(1)? {
        PdfValue::Name(name) => {
            let all = file.resolve_dict(resources.get("Properties")?).ok()?;
            file.resolve_dict(all.get(name)?).ok()
        }
        value => file.resolve_dict(value).ok(),
    }
}

/// Gives the glyphs of a marked sequence the text its `/ActualText` reads,
/// spread over them in order. A character standing for several glyphs takes
/// their bytes together, which only a single run of one string allows: when
/// the glyphs lie apart, they keep their own text so none escapes removal.
fn replace_with_actual_text(context: &mut TextContext, start: usize, actual: &str) {
    let chars: Vec<char> = actual.chars().collect();
    let Some(replaced) = spread(&drawn_units(&context.glyphs[start..]), &chars) else {
        return;
    };
    let cut = context.glyphs[start..]
        .iter()
        .find(|glyph| glyph.visible)
        .map_or(context.text_chars, |glyph| glyph.page_char_index);
    let byte = context
        .text
        .char_indices()
        .nth(cut)
        .map_or(context.text.len(), |(byte, _)| byte);
    context.text.truncate(byte);
    context.text_chars = cut.min(context.text_chars);
    context
        .items
        .retain(|item| item.char_start.is_none_or(|item_start| item_start < cut));
    context.glyphs.truncate(start);
    let mut item_text = String::new();
    let mut item_box: Option<Rect> = None;
    for mut glyph in replaced {
        glyph.page_char_index = cut + item_text.chars().count();
        if glyph.visible {
            item_text.push(glyph.text);
            context.text.push(glyph.text);
            context.text_chars += 1;
            item_box = Some(item_box.map_or(glyph.bbox, |bbox| bbox.union(&glyph.bbox)));
        }
        context.glyphs.push(glyph);
    }
    if let Some(bbox) = item_box {
        context.items.push(TextItem {
            char_end: Some(cut + item_text.chars().count()),
            text: item_text,
            bbox,
            quad: Some(bbox.to_quad()),
            char_start: Some(cut),
        });
    }
}

/// One entry per glyph drawn, a glyph read as several characters counting once.
fn drawn_units(glyphs: &[Glyph]) -> Vec<Glyph> {
    let mut units: Vec<Glyph> = Vec::new();
    for glyph in glyphs {
        match units.last() {
            Some(last)
                if last.operation_index == glyph.operation_index
                    && last.source_form == glyph.source_form
                    && location_key(&last.location) == location_key(&glyph.location) => {}
            _ => units.push(glyph.clone()),
        }
    }
    units
}

/// The characters spread over the drawn glyphs in order. A character standing
/// for several glyphs takes their bytes together, which only a single run of
/// one string allows: `None` when the glyphs lie apart, so none escapes removal.
fn spread(units: &[Glyph], chars: &[char]) -> Option<Vec<Glyph>> {
    let (count, drawn) = (chars.len(), units.len());
    if count == 0 || drawn == 0 {
        return None;
    }
    if count >= drawn {
        return Some(
            (0..drawn)
                .flat_map(|unit| {
                    let glyph = &units[unit];
                    chars[unit * count / drawn..(unit + 1) * count / drawn]
                        .iter()
                        .map(move |character| Glyph {
                            text: *character,
                            ..glyph.clone()
                        })
                })
                .collect(),
        );
    }
    chars
        .iter()
        .enumerate()
        .map(|(index, character)| {
            let group = &units[index * drawn / count..(index + 1) * drawn / count];
            let glyph = group.iter().skip(1).try_fold(group[0].clone(), follow)?;
            Some(Glyph {
                text: *character,
                ..glyph
            })
        })
        .collect()
}

/// Gives the glyphs of the page's marked sequences the `/ActualText` of the
/// structure element that holds them, spread over all its sequences in order.
fn apply_structure_texts(file: &PdfFile, page: ObjectRef, context: &mut TextContext) {
    if context.mcid_ranges.is_empty() {
        return;
    }
    let texts = crate::structure::page_actual_texts(file, page);
    type Ranges = Vec<(usize, usize)>;
    let mut elements: BTreeMap<ObjectRef, (&str, Ranges)> = BTreeMap::new();
    let mut drawn_images: BTreeMap<ObjectRef, Vec<PlacedImage>> = BTreeMap::new();
    for range in &context.mcid_ranges {
        if let Some((element, text)) = texts.get(&range.mcid) {
            elements
                .entry(*element)
                .or_insert((text, Vec::new()))
                .1
                .push(range.glyphs);
            drawn_images.entry(*element).or_default().extend(
                context.images[range.images.0..range.images.1]
                    .iter()
                    .map(|image| (range.glyphs.0, *image)),
            );
        }
    }
    let mut splices: Vec<(usize, usize, Vec<Glyph>)> = Vec::new();
    for (element, (text, ranges)) in &elements {
        let chars: Vec<char> = text.chars().collect();
        let images = &drawn_images[element];
        if ranges.iter().all(|(start, end)| start == end) && !images.is_empty() {
            splices.extend(written_on_images(&chars, images));
            continue;
        }
        let units: Vec<Vec<Glyph>> = ranges
            .iter()
            .map(|(start, end)| drawn_units(&context.glyphs[*start..*end]))
            .collect();
        let total: usize = units.iter().map(Vec::len).sum();
        let drawn_ranges = units.iter().filter(|unit| !unit.is_empty()).count();
        if total == 0 || chars.len() < drawn_ranges {
            continue;
        }
        let (mut done, mut seen, mut left) = (0, 0, drawn_ranges);
        let mut pieces = Vec::new();
        for (range, unit) in ranges.iter().zip(&units) {
            if unit.is_empty() {
                continue;
            }
            seen += unit.len();
            left -= 1;
            let upto = (chars.len() * seen / total)
                .max(done + 1)
                .min(chars.len() - left);
            match spread(unit, &chars[done..upto]) {
                Some(piece) => pieces.push((range.0, range.1, piece)),
                None => {
                    pieces.clear();
                    break;
                }
            }
            done = upto;
        }
        splices.extend(pieces);
    }
    if splices.is_empty() {
        return;
    }
    splices.sort_by_key(|(start, _, piece)| {
        std::cmp::Reverse((*start, piece.first().map(|glyph| glyph.operation_index)))
    });
    let text: Vec<char> = context.text.chars().collect();
    let mut breaks: Vec<bool> = context
        .glyphs
        .iter()
        .map(|glyph| {
            glyph.visible
                && glyph
                    .page_char_index
                    .checked_sub(1)
                    .is_some_and(|before| text.get(before) == Some(&'\n'))
        })
        .collect();
    let mut floor = usize::MAX;
    for (start, end, piece) in splices {
        if end > floor || (end == floor && start < end) {
            continue;
        }
        floor = start;
        let broken = start == end || breaks[start..end].iter().any(|b| *b);
        let flags: Vec<bool> = (0..piece.len()).map(|index| index == 0 && broken).collect();
        breaks.splice(start..end, flags);
        context.glyphs.splice(start..end, piece);
    }
    context.text.clear();
    context.items.clear();
    let mut count = 0usize;
    let mut item: Option<(usize, Option<ObjectRef>, TextItem)> = None;
    for (glyph, broken) in context.glyphs.iter_mut().zip(breaks) {
        glyph.page_char_index = count;
        if !glyph.visible {
            continue;
        }
        if broken && count > 0 {
            context.text.push('\n');
            count += 1;
            glyph.page_char_index = count;
        }
        let joins = !broken
            && item.as_ref().is_some_and(|(operation, form, _)| {
                *operation == glyph.operation_index && *form == glyph.source_form
            });
        if !joins && let Some((_, _, done)) = item.take() {
            context.items.push(done);
        }
        let (_, _, current) = item.get_or_insert_with(|| {
            (
                glyph.operation_index,
                glyph.source_form,
                TextItem {
                    text: String::new(),
                    bbox: glyph.bbox,
                    quad: None,
                    char_start: Some(count),
                    char_end: Some(count),
                },
            )
        });
        current.text.push(glyph.text);
        current.bbox = current.bbox.union(&glyph.bbox);
        current.quad = Some(current.bbox.to_quad());
        context.text.push(glyph.text);
        count += 1;
        current.char_end = Some(count);
    }
    if let Some((_, _, done)) = item {
        context.items.push(done);
    }
    context.text_chars = count;
}

/// Glyphs standing for a text the page draws as pictures, the characters
/// spread over the images in order, each image cut in as many boxes.
fn written_on_images(chars: &[char], images: &[PlacedImage]) -> Vec<(usize, usize, Vec<Glyph>)> {
    let (count, drawn) = (chars.len(), images.len());
    images
        .iter()
        .enumerate()
        .filter_map(|(index, (at, (operation_index, quad)))| {
            let own = &chars[index * count / drawn..(index + 1) * count / drawn];
            if own.is_empty() {
                return None;
            }
            let [a, b, _, d] = quad.points;
            let along = |t: f64| Point {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            };
            let up = Point {
                x: d.x - a.x,
                y: d.y - a.y,
            };
            let glyphs = own
                .iter()
                .enumerate()
                .map(|(slot, character)| {
                    let (low, high) = (
                        along(slot as f64 / own.len() as f64),
                        along((slot + 1) as f64 / own.len() as f64),
                    );
                    let quad = Quad {
                        points: [
                            low,
                            high,
                            Point {
                                x: high.x + up.x,
                                y: high.y + up.y,
                            },
                            Point {
                                x: low.x + up.x,
                                y: low.y + up.y,
                            },
                        ],
                    };
                    Glyph {
                        text: *character,
                        bbox: quad.bounding_rect(),
                        quad,
                        page_char_index: 0,
                        operation_index: *operation_index,
                        location: GlyphLocation::Direct {
                            operand_index: 0,
                            byte_start: 0,
                            byte_end: 0,
                        },
                        visible: true,
                        width_units: 0.0,
                        source_form: None,
                    }
                })
                .collect();
            Some((*at, *at, glyphs))
        })
        .collect()
}

fn location_key(location: &GlyphLocation) -> (usize, Option<usize>, usize, usize) {
    match *location {
        GlyphLocation::Direct {
            operand_index,
            byte_start,
            byte_end,
        } => (operand_index, None, byte_start, byte_end),
        GlyphLocation::Array {
            operand_index,
            element_index,
            byte_start,
            byte_end,
        } => (operand_index, Some(element_index), byte_start, byte_end),
    }
}

/// The glyph drawn by `glyph` then `next`, when `next` follows it in the same
/// string.
fn follow(mut glyph: Glyph, next: &Glyph) -> Option<Glyph> {
    let (operand, element, _, end) = location_key(&glyph.location);
    let (next_operand, next_element, next_start, next_end) = location_key(&next.location);
    if glyph.operation_index != next.operation_index
        || glyph.source_form != next.source_form
        || (operand, element, end) != (next_operand, next_element, next_start)
    {
        return None;
    }
    match &mut glyph.location {
        GlyphLocation::Direct { byte_end, .. } | GlyphLocation::Array { byte_end, .. } => {
            *byte_end = next_end
        }
    }
    glyph.bbox = glyph.bbox.union(&next.bbox);
    glyph.quad = glyph.bbox.to_quad();
    glyph.width_units += next.width_units;
    glyph.visible |= next.visible;
    Some(glyph)
}

fn decode_simple_byte(font: &SimpleFont, byte: u8) -> String {
    if let Some(mapped) = font.to_unicode.as_ref().and_then(|map| {
        map.unicode(Code {
            value: u32::from(byte),
            len: 1,
        })
    }) && !mapped.is_empty()
    {
        return mapped.into_owned();
    }
    if let Some(glyph_name) = font.encoding.differences.get(&byte) {
        return glyph_name_to_text(glyph_name)
            .or_else(|| font.recovered.get(&byte).map(char::to_string))
            .unwrap_or_else(|| '\u{FFFD}'.to_string());
    }
    if let Some(text) = font.builtin.get(&byte) {
        return text.clone();
    }
    base_char(font.encoding.base, byte)
        .map(|character| character.to_string())
        .unwrap_or_else(|| '\u{FFFD}'.to_string())
}

fn base_char(base: SimpleEncodingBase, byte: u8) -> Option<char> {
    match base {
        // In WinAnsiEncoding every unused code above 040 draws the bullet.
        SimpleEncodingBase::WinAnsi => {
            winansi_byte_to_char(byte).or((byte > 0x20).then_some('\u{2022}'))
        }
        SimpleEncodingBase::MacRoman => mac_roman_byte_to_char(byte),
        SimpleEncodingBase::Standard => standard_byte_to_char(byte),
        SimpleEncodingBase::Identity => identity_byte_to_char(byte),
        SimpleEncodingBase::Symbol => symbolic::symbol_char(byte),
        SimpleEncodingBase::Dingbats => symbolic::dingbats_char(byte),
    }
}

fn identity_byte_to_char(byte: u8) -> Option<char> {
    if byte.is_ascii() {
        Some(byte as char)
    } else {
        None
    }
}

/// Minimal Adobe Glyph List subset: maps PDF glyph names commonly appearing
/// in `/Encoding` `Differences` arrays to their Unicode equivalents. Covers
/// the WinAnsi repertoire plus common additions (Latin supplement, Latin
/// Extended-A, a few math and punctuation glyphs). Names not present here
/// fall through to `U+FFFD`, the caller handles that.
/// The text a glyph name stands for: one character, or, for a ligature named
/// `f_f_i` or `uni00660069`, the characters of its parts.
fn glyph_name_to_text(name: &str) -> Option<String> {
    let base = name.split('.').next().unwrap_or(name);
    if let Some(hex) = base.strip_prefix("uni")
        && hex.len() > 4
        && hex.len().is_multiple_of(4)
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return hex
            .as_bytes()
            .chunks(4)
            .map(|unit| {
                let unit = std::str::from_utf8(unit).ok()?;
                char::from_u32(u32::from_str_radix(unit, 16).ok()?)
            })
            .collect();
    }
    if let Some(character) = glyph_name_to_char(name) {
        return Some(character.to_string());
    }
    base.contains('_')
        .then(|| base.split('_').map(glyph_name_to_char).collect())
        .flatten()
}

fn glyph_name_to_char(name: &str) -> Option<char> {
    let base = name.split('.').next().unwrap_or(name);
    if let Ok(found) = crate::text::glyph_list::GLYPHS.binary_search_by(|(n, _)| (*n).cmp(base)) {
        return Some(crate::text::glyph_list::GLYPHS[found].1);
    }
    let hex = base
        .strip_prefix("uni")
        .map(|h| &h[..h.len().min(4)])
        .or_else(|| base.strip_prefix('u'))?;
    if !(4..=6).contains(&hex.len()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Whether the font's `/Encoding` names a base encoding of its own, in which
/// case an unknown one is not replaced.
fn has_named_base(file: &PdfFile, font_dict: &crate::objects::PdfDictionary) -> bool {
    let known = |name: Option<&str>| {
        matches!(
            name,
            Some("WinAnsiEncoding" | "MacRomanEncoding" | "MacExpertEncoding" | "StandardEncoding")
        )
    };
    match font_dict.get("Encoding") {
        Some(PdfValue::Name(name)) => known(Some(name)),
        Some(value) => file
            .resolve_dict(value)
            .ok()
            .is_some_and(|d| known(d.get("BaseEncoding").and_then(PdfValue::as_name))),
        None => false,
    }
}

/// A font's name without its subset tag, lowercase, without separators.
fn plain_name(base_font: &str) -> String {
    base_font
        .split_once('+')
        .filter(|(tag, _)| tag.len() == 6)
        .map_or(base_font, |(_, rest)| rest)
        .to_ascii_lowercase()
        .replace([' ', '-', ',', '_'], "")
}

/// How a font looks, read from its name and descriptor flags: what a drawing
/// shows its text in when it does not draw the font's own outlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Face {
    pub serif: bool,
    pub fixed: bool,
    pub bold: bool,
    pub italic: bool,
    pub symbol: bool,
}

impl Face {
    fn for_font(base_font: &str, flags: i64) -> Face {
        let name = plain_name(base_font);
        let any = |words: &[&str]| words.iter().any(|word| name.contains(word));
        let fixed = flags & 1 != 0
            || any(&[
                "courier",
                "mono",
                "consol",
                "cmtt",
                "nimbusmon",
                "typewriter",
                "cursor",
            ]);
        let sans = any(&[
            "sans",
            "arial",
            "helvetica",
            "calibri",
            "verdana",
            "tahoma",
            "gothic",
            "cmss",
            "segoe",
            "trebuchet",
            "futura",
            "frutiger",
            "myriad",
            "univers",
            "heros",
            "adventor",
        ]);
        let serif = !sans
            && (flags & (1 << 1) != 0
                || any(&[
                    "times",
                    "roman",
                    "serif",
                    "garamond",
                    "georgia",
                    "cambria",
                    "book",
                    "nimbusrom",
                    "palatino",
                    "palladio",
                    "minion",
                    "century",
                    "baskerville",
                    "caslon",
                    "bodoni",
                    "charter",
                    "utopia",
                    "mincho",
                    "song",
                    "ming",
                    "cmr",
                    "cmbx",
                    "cmti",
                    "cmmi",
                    "sfrm",
                    "lmroman",
                    "ptmr",
                    "ptmb",
                    "ptmri",
                    "utmr",
                    "termes",
                    "pagella",
                    "bonum",
                    "schola",
                    "newtx",
                    "txmi",
                    "stix",
                    "libertin",
                ]));
        Face {
            serif,
            fixed,
            bold: flags & (1 << 18) != 0
                || any(&["bold", "black", "heavy", "demi", "medi", "cmbx", "ptmb"]),
            italic: flags & (1 << 6) != 0 || any(&["italic", "oblique", "ital", "cmti", "cmmi"]),
            symbol: any(&["symbol", "dingbat", "zapf", "wingding", "cmsy"]),
        }
    }
}

/// Which of the 14 standard fonts measures a simple font: itself, or the
/// substitute its name and descriptor flags point to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StandardMetrics {
    Courier,
    Helvetica,
    HelveticaBold,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Symbol,
    Dingbats,
}

impl StandardMetrics {
    fn for_font(base_font: &str, flags: i64) -> Self {
        let name = plain_name(base_font);
        let bold = name.contains("bold")
            || name.contains("black")
            || name.contains("heavy")
            || name.contains("semibold")
            || flags & (1 << 18) != 0;
        let italic = name.contains("italic") || name.contains("oblique") || flags & (1 << 6) != 0;
        if name.contains("symbol") {
            return Self::Symbol;
        }
        if name.contains("dingbat") || name.contains("zapf") {
            return Self::Dingbats;
        }
        let fixed = name.contains("courier") || name.contains("mono") || flags & 1 != 0;
        if fixed {
            return Self::Courier;
        }
        let serif = name.contains("times")
            || name.contains("roman")
            || name.contains("serif") && !name.contains("sans")
            || name.contains("garamond")
            || name.contains("georgia")
            || name.contains("cambria")
            || name.contains("book")
            || (flags & (1 << 1) != 0
                && !["arial", "helvetica", "calibri", "verdana", "tahoma"]
                    .iter()
                    .any(|sans| name.contains(sans)));
        match (serif, bold, italic) {
            (true, true, true) => Self::TimesBoldItalic,
            (true, true, false) => Self::TimesBold,
            (true, false, true) => Self::TimesItalic,
            (true, false, false) => Self::TimesRoman,
            (false, true, _) => Self::HelveticaBold,
            (false, false, _) => Self::Helvetica,
        }
    }

    /// Advance of `text`, the character `byte` decodes to, in thousandths.
    fn width(self, byte: u8, text: &str) -> f64 {
        use crate::text::standard_metrics as m;
        let by_char = |table: &[(char, u16)]| {
            text.chars()
                .next()
                .and_then(|c| table.binary_search_by_key(&c, |(k, _)| *k).ok())
                .map(|found| f64::from(table[found].1))
        };
        let by_code = |table: &[(u8, u16)]| {
            table
                .binary_search_by_key(&byte, |(k, _)| *k)
                .ok()
                .map(|found| f64::from(table[found].1))
        };
        let found = match self {
            Self::Courier => Some(600.0),
            Self::Helvetica => by_char(m::HELVETICA),
            Self::HelveticaBold => by_char(m::HELVETICA_BOLD),
            Self::TimesRoman => by_char(m::TIMES_ROMAN),
            Self::TimesBold => by_char(m::TIMES_BOLD),
            Self::TimesItalic => by_char(m::TIMES_ITALIC),
            Self::TimesBoldItalic => by_char(m::TIMES_BOLD_ITALIC),
            Self::Symbol => by_code(m::SYMBOL),
            Self::Dingbats => by_code(m::ZAPF_DINGBATS),
        };
        found.unwrap_or(500.0)
    }
}

/// Decode a single byte under the PDF `WinAnsiEncoding` table.
///
/// Codes `0x20..=0x7E` are plain ASCII. Codes `0x80..=0x9F` carry the
/// Windows-1252 punctuation repertoire (smart quotes, the Euro sign, the
/// em/en dashes, and so on). Codes `0xA0..=0xFF` are ISO-8859-1.
/// Undefined codes return `None`, which callers render as `U+FFFD`.
fn winansi_byte_to_char(byte: u8) -> Option<char> {
    match byte {
        0x20..=0x7E => Some(byte as char),
        0x80 => Some('\u{20AC}'),
        0x82 => Some('\u{201A}'),
        0x83 => Some('\u{0192}'),
        0x84 => Some('\u{201E}'),
        0x85 => Some('\u{2026}'),
        0x86 => Some('\u{2020}'),
        0x87 => Some('\u{2021}'),
        0x88 => Some('\u{02C6}'),
        0x89 => Some('\u{2030}'),
        0x8A => Some('\u{0160}'),
        0x8B => Some('\u{2039}'),
        0x8C => Some('\u{0152}'),
        0x8E => Some('\u{017D}'),
        0x91 => Some('\u{2018}'),
        0x92 => Some('\u{2019}'),
        0x93 => Some('\u{201C}'),
        0x94 => Some('\u{201D}'),
        0x95 => Some('\u{2022}'),
        0x96 => Some('\u{2013}'),
        0x97 => Some('\u{2014}'),
        0x98 => Some('\u{02DC}'),
        0x99 => Some('\u{2122}'),
        0x9A => Some('\u{0161}'),
        0x9B => Some('\u{203A}'),
        0x9C => Some('\u{0153}'),
        0x9E => Some('\u{017E}'),
        0x9F => Some('\u{0178}'),
        0xA0..=0xFF => Some(byte as char),
        _ => None,
    }
}

/// Decode a single byte under the PDF `StandardEncoding` table (PDF Annex D,
/// table E.5, "Adobe Standard Encoding"). ASCII codes pass through
/// except for `0x27` (`quoteright` = U+2019) and `0x60` (`quoteleft` =
/// U+2018), which are Adobe's historical typographic-quote choice. The
/// high-bit range is sparse and carries Latin-1 supplement glyphs,
/// currency, ligatures, and accented-letter bases. Codes not defined in
/// the encoding return `None`.
fn standard_byte_to_char(byte: u8) -> Option<char> {
    match byte {
        0x27 => Some('\u{2019}'),
        0x60 => Some('\u{2018}'),
        0x20..=0x7E => Some(byte as char),
        0xA1 => Some('\u{00A1}'),
        0xA2 => Some('\u{00A2}'),
        0xA3 => Some('\u{00A3}'),
        0xA4 => Some('\u{2044}'),
        0xA5 => Some('\u{00A5}'),
        0xA6 => Some('\u{0192}'),
        0xA7 => Some('\u{00A7}'),
        0xA8 => Some('\u{00A4}'),
        0xA9 => Some('\u{0027}'),
        0xAA => Some('\u{201C}'),
        0xAB => Some('\u{00AB}'),
        0xAC => Some('\u{2039}'),
        0xAD => Some('\u{203A}'),
        0xAE => Some('\u{FB01}'),
        0xAF => Some('\u{FB02}'),
        0xB1 => Some('\u{2013}'),
        0xB2 => Some('\u{2020}'),
        0xB3 => Some('\u{2021}'),
        0xB4 => Some('\u{00B7}'),
        0xB6 => Some('\u{00B6}'),
        0xB7 => Some('\u{2022}'),
        0xB8 => Some('\u{201A}'),
        0xB9 => Some('\u{201E}'),
        0xBA => Some('\u{201D}'),
        0xBB => Some('\u{00BB}'),
        0xBC => Some('\u{2026}'),
        0xBD => Some('\u{2030}'),
        0xBF => Some('\u{00BF}'),
        0xC1 => Some('\u{0060}'),
        0xC2 => Some('\u{00B4}'),
        0xC3 => Some('\u{02C6}'),
        0xC4 => Some('\u{02DC}'),
        0xC5 => Some('\u{00AF}'),
        0xC6 => Some('\u{02D8}'),
        0xC7 => Some('\u{02D9}'),
        0xC8 => Some('\u{00A8}'),
        0xCA => Some('\u{02DA}'),
        0xCB => Some('\u{00B8}'),
        0xCD => Some('\u{02DD}'),
        0xCE => Some('\u{02DB}'),
        0xCF => Some('\u{02C7}'),
        0xE1 => Some('\u{00C6}'),
        0xE3 => Some('\u{00AA}'),
        0xE8 => Some('\u{0141}'),
        0xE9 => Some('\u{00D8}'),
        0xEA => Some('\u{0152}'),
        0xEB => Some('\u{00BA}'),
        0xF1 => Some('\u{00E6}'),
        0xF5 => Some('\u{0131}'),
        0xF8 => Some('\u{0142}'),
        0xF9 => Some('\u{00F8}'),
        0xFA => Some('\u{0153}'),
        0xFB => Some('\u{00DF}'),
        _ => None,
    }
}

/// Decode a single byte under the PDF `MacRomanEncoding` table (PDF Annex D).
///
/// Codes `0x20..=0x7E` are plain ASCII. Codes `0x80..=0xFF` carry the Mac
/// Roman repertoire: accented Latin letters, math symbols, punctuation,
/// ligatures, and (at `0xF0`) the Apple logo mapped to the Corporate Use
/// Area `U+F8FF`. Undefined codes return `None`, which callers render as
/// `U+FFFD`.
fn mac_roman_byte_to_char(byte: u8) -> Option<char> {
    match byte {
        0x20..=0x7E => Some(byte as char),
        0x80 => Some('\u{00C4}'), // Ä
        0x81 => Some('\u{00C5}'), // Å
        0x82 => Some('\u{00C7}'), // Ç
        0x83 => Some('\u{00C9}'), // É
        0x84 => Some('\u{00D1}'), // Ñ
        0x85 => Some('\u{00D6}'), // Ö
        0x86 => Some('\u{00DC}'), // Ü
        0x87 => Some('\u{00E1}'), // á
        0x88 => Some('\u{00E0}'), // à
        0x89 => Some('\u{00E2}'), // â
        0x8A => Some('\u{00E4}'), // ä
        0x8B => Some('\u{00E3}'), // ã
        0x8C => Some('\u{00E5}'), // å
        0x8D => Some('\u{00E7}'), // ç
        0x8E => Some('\u{00E9}'), // é
        0x8F => Some('\u{00E8}'), // è
        0x90 => Some('\u{00EA}'), // ê
        0x91 => Some('\u{00EB}'), // ë
        0x92 => Some('\u{00ED}'), // í
        0x93 => Some('\u{00EC}'), // ì
        0x94 => Some('\u{00EE}'), // î
        0x95 => Some('\u{00EF}'), // ï
        0x96 => Some('\u{00F1}'), // ñ
        0x97 => Some('\u{00F3}'), // ó
        0x98 => Some('\u{00F2}'), // ò
        0x99 => Some('\u{00F4}'), // ô
        0x9A => Some('\u{00F6}'), // ö
        0x9B => Some('\u{00F5}'), // õ
        0x9C => Some('\u{00FA}'), // ú
        0x9D => Some('\u{00F9}'), // ù
        0x9E => Some('\u{00FB}'), // û
        0x9F => Some('\u{00FC}'), // ü
        0xA0 => Some('\u{2020}'), // †
        0xA1 => Some('\u{00B0}'), // °
        0xA2 => Some('\u{00A2}'), // ¢
        0xA3 => Some('\u{00A3}'), // £
        0xA4 => Some('\u{00A7}'), // §
        0xA5 => Some('\u{2022}'), // •
        0xA6 => Some('\u{00B6}'), // ¶
        0xA7 => Some('\u{00DF}'), // ß
        0xA8 => Some('\u{00AE}'), // ®
        0xA9 => Some('\u{00A9}'), // ©
        0xAA => Some('\u{2122}'), // ™
        0xAB => Some('\u{00B4}'), // ´
        0xAC => Some('\u{00A8}'), // ¨
        0xAD => Some('\u{2260}'), // ≠
        0xAE => Some('\u{00C6}'), // Æ
        0xAF => Some('\u{00D8}'), // Ø
        0xB0 => Some('\u{221E}'), // ∞
        0xB1 => Some('\u{00B1}'), // ±
        0xB2 => Some('\u{2264}'), // ≤
        0xB3 => Some('\u{2265}'), // ≥
        0xB4 => Some('\u{00A5}'), // ¥
        0xB5 => Some('\u{00B5}'), // µ
        0xB6 => Some('\u{2202}'), // ∂
        0xB7 => Some('\u{2211}'), // ∑
        0xB8 => Some('\u{220F}'), // ∏
        0xB9 => Some('\u{03C0}'), // π
        0xBA => Some('\u{222B}'), // ∫
        0xBB => Some('\u{00AA}'), // ª
        0xBC => Some('\u{00BA}'), // º
        0xBD => Some('\u{03A9}'), // Ω
        0xBE => Some('\u{00E6}'), // æ
        0xBF => Some('\u{00F8}'), // ø
        0xC0 => Some('\u{00BF}'), // ¿
        0xC1 => Some('\u{00A1}'), // ¡
        0xC2 => Some('\u{00AC}'), // ¬
        0xC3 => Some('\u{221A}'), // √
        0xC4 => Some('\u{0192}'), // ƒ
        0xC5 => Some('\u{2248}'), // ≈
        0xC6 => Some('\u{2206}'), // ∆
        0xC7 => Some('\u{00AB}'), // «
        0xC8 => Some('\u{00BB}'), // »
        0xC9 => Some('\u{2026}'), // …
        0xCA => Some('\u{00A0}'), // non-breaking space
        0xCB => Some('\u{00C0}'), // À
        0xCC => Some('\u{00C3}'), // Ã
        0xCD => Some('\u{00D5}'), // Õ
        0xCE => Some('\u{0152}'), // Œ
        0xCF => Some('\u{0153}'), // œ
        0xD0 => Some('\u{2013}'), // en dash
        0xD1 => Some('\u{2014}'), // em dash
        0xD2 => Some('\u{201C}'), // "
        0xD3 => Some('\u{201D}'), // "
        0xD4 => Some('\u{2018}'), // '
        0xD5 => Some('\u{2019}'), // '
        0xD6 => Some('\u{00F7}'), // ÷
        0xD7 => Some('\u{25CA}'), // ◊
        0xD8 => Some('\u{00FF}'), // ÿ
        0xD9 => Some('\u{0178}'), // Ÿ
        0xDA => Some('\u{2044}'), // ⁄
        0xDB => Some('\u{20AC}'), // €
        0xDC => Some('\u{2039}'), // ‹
        0xDD => Some('\u{203A}'), // ›
        0xDE => Some('\u{FB01}'), // ﬁ
        0xDF => Some('\u{FB02}'), // ﬂ
        0xE0 => Some('\u{2021}'), // ‡
        0xE1 => Some('\u{00B7}'), // ·
        0xE2 => Some('\u{201A}'), // ‚
        0xE3 => Some('\u{201E}'), // „
        0xE4 => Some('\u{2030}'), // ‰
        0xE5 => Some('\u{00C2}'), // Â
        0xE6 => Some('\u{00CA}'), // Ê
        0xE7 => Some('\u{00C1}'), // Á
        0xE8 => Some('\u{00CB}'), // Ë
        0xE9 => Some('\u{00C8}'), // È
        0xEA => Some('\u{00CD}'), // Í
        0xEB => Some('\u{00CE}'), // Î
        0xEC => Some('\u{00CF}'), // Ï
        0xED => Some('\u{00CC}'), // Ì
        0xEE => Some('\u{00D3}'), // Ó
        0xEF => Some('\u{00D4}'), // Ô
        0xF0 => Some('\u{F8FF}'), // Apple logo (Corporate Use Area)
        0xF1 => Some('\u{00D2}'), // Ò
        0xF2 => Some('\u{00DA}'), // Ú
        0xF3 => Some('\u{00DB}'), // Û
        0xF4 => Some('\u{00D9}'), // Ù
        0xF5 => Some('\u{0131}'), // ı
        0xF6 => Some('\u{02C6}'), // ˆ
        0xF7 => Some('\u{02DC}'), // ˜
        0xF8 => Some('\u{00AF}'), // ¯
        0xF9 => Some('\u{02D8}'), // ˘
        0xFA => Some('\u{02D9}'), // ˙
        0xFB => Some('\u{02DA}'), // ˚
        0xFC => Some('\u{00B8}'), // ¸
        0xFD => Some('\u{02DD}'), // ˝
        0xFE => Some('\u{02DB}'), // ˛
        0xFF => Some('\u{02C7}'), // ˇ
        _ => None,
    }
}

fn parse_simple_encoding(
    file: &PdfFile,
    font_dict: &crate::objects::PdfDictionary,
) -> SimpleEncoding {
    match font_dict.get("Encoding") {
        Some(PdfValue::Name(name)) => SimpleEncoding {
            base: simple_encoding_base_from_name(name),
            differences: BTreeMap::new(),
        },
        Some(value @ PdfValue::Reference(_)) | Some(value @ PdfValue::Dictionary(_)) => {
            match file.resolve_dict(value) {
                Ok(dict) => {
                    let base = dict
                        .get("BaseEncoding")
                        .and_then(PdfValue::as_name)
                        .map(simple_encoding_base_from_name)
                        .unwrap_or_default();
                    let differences = dict
                        .get("Differences")
                        .and_then(PdfValue::as_array)
                        .map(parse_differences_array)
                        .unwrap_or_default();
                    SimpleEncoding { base, differences }
                }
                Err(_) => SimpleEncoding::default(),
            }
        }
        _ => SimpleEncoding::default(),
    }
}

fn simple_encoding_base_from_name(name: &str) -> SimpleEncodingBase {
    match name {
        "WinAnsiEncoding" => SimpleEncodingBase::WinAnsi,
        "MacRomanEncoding" => SimpleEncodingBase::MacRoman,
        "StandardEncoding" => SimpleEncodingBase::Standard,
        _ => SimpleEncodingBase::Identity,
    }
}

fn parse_differences_array(entries: &[PdfValue]) -> BTreeMap<u8, String> {
    let mut output = BTreeMap::new();
    let mut code: u16 = 0;
    let mut have_code = false;
    for entry in entries {
        match entry {
            PdfValue::Integer(value) => {
                if *value >= 0 && *value <= 255 {
                    code = *value as u16;
                    have_code = true;
                } else {
                    have_code = false;
                }
            }
            PdfValue::Number(value) => {
                let rounded = value.round() as i64;
                if (0..=255).contains(&rounded) {
                    code = rounded as u16;
                    have_code = true;
                } else {
                    have_code = false;
                }
            }
            PdfValue::Name(name) if have_code && code <= 255 => {
                output.insert(code as u8, name.clone());
                code += 1;
                if code > 255 {
                    have_code = false;
                }
            }
            _ => {}
        }
    }
    output
}

pub(crate) fn decode_font_glyphs(font: &LoadedFont, bytes: &[u8]) -> Vec<DecodedGlyph> {
    match font {
        LoadedFont::Simple(font) => bytes
            .iter()
            .copied()
            .enumerate()
            .map(|(byte_index, byte)| {
                let text = decode_simple_byte(font, byte);
                let width_units = u16::from(byte)
                    .checked_sub(font.first_char)
                    .and_then(|index| font.widths.get(index as usize))
                    .map(|width| width * font.width_scale)
                    .or_else(|| {
                        (font.missing_width > 0.0 && !font.widths.is_empty())
                            .then_some(font.missing_width)
                    })
                    .unwrap_or_else(|| font.metrics.width(byte, &text));
                DecodedGlyph {
                    text,
                    code: u32::from(byte),
                    cid: None,
                    width_units,
                    byte_start: byte_index,
                    byte_end: byte_index + 1,
                    word_space: byte == b' ',
                    vertical: None,
                }
            })
            .collect(),
        LoadedFont::Composite(font) => decode_composite_glyphs(font, bytes),
        LoadedFont::Unreadable { code_len } => bytes
            .chunks(*code_len)
            .enumerate()
            .map(|(index, chunk)| DecodedGlyph {
                text: '\u{FFFD}'.to_string(),
                code: chunk.iter().fold(0, |code, b| code << 8 | u32::from(*b)),
                cid: None,
                width_units: 500.0,
                byte_start: index * code_len,
                byte_end: index * code_len + chunk.len(),
                word_space: chunk == b" ",
                vertical: None,
            })
            .collect(),
    }
}

fn load_composite_font(
    file: &PdfFile,
    font_dict: &crate::objects::PdfDictionary,
    draws: bool,
) -> PdfResult<CompositeFont> {
    let (encoding, mut unicode_codes) = match font_dict.get("Encoding") {
        Some(PdfValue::Name(name)) => (
            CMap::predefined(name).ok_or_else(|| {
                PdfError::Unsupported(format!("Type0 font encoding {name} is not supported"))
            })?,
            CMap::unicode_codes(name),
        ),
        Some(value) => {
            let stream = match value {
                PdfValue::Reference(reference) => get_stream(file, *reference)?,
                _ => {
                    return Err(PdfError::Corrupt(
                        "Type0 Encoding is not a CMap".to_string(),
                    ));
                }
            };
            let mut cmap = CMap::parse(&decode_stream(stream)?);
            cmap.bf_as_cids();
            let parent = stream
                .dict
                .get("UseCMap")
                .and_then(PdfValue::as_name)
                .map(str::to_string)
                .or_else(|| cmap.parent.clone());
            let mut unicode = false;
            if let Some(parent) = parent.as_deref().and_then(|name| {
                unicode = CMap::unicode_codes(name);
                CMap::predefined(name)
            }) {
                cmap.inherit(&parent);
            }
            (cmap, unicode)
        }
        None => (CMap::identity(false), false),
    };

    let descendant = font_dict
        .get("DescendantFonts")
        .and_then(|v| file.resolve(v).ok())
        .and_then(PdfValue::as_array)
        .and_then(|fonts| fonts.first())
        .ok_or_else(|| PdfError::Corrupt("Type0 font is missing DescendantFonts".to_string()))?;
    let descendant_dict = file.resolve_dict(descendant)?;
    let number = |key: &str| {
        descendant_dict
            .get(key)
            .and_then(|v| file.resolve(v).ok())
            .and_then(PdfValue::as_number)
    };
    let default_width = number("DW").unwrap_or(1000.0);
    let widths = descendant_dict
        .get("W")
        .and_then(|v| file.resolve(v).ok())
        .map(|w| parse_cid_widths(file, w))
        .unwrap_or_default();
    let default_vertical = descendant_dict
        .get("DW2")
        .and_then(|v| file.resolve(v).ok())
        .and_then(PdfValue::as_array)
        .and_then(|dw2| match dw2 {
            [vy, w1] => Some(VerticalMetrics {
                advance: w1.as_number()?,
                origin_x: None,
                origin_y: vy.as_number()?,
            }),
            _ => None,
        })
        .unwrap_or(DEFAULT_VERTICAL);
    let vertical_metrics = descendant_dict
        .get("W2")
        .and_then(|v| file.resolve(v).ok())
        .map(|w2| parse_cid_vertical_metrics(file, w2))
        .unwrap_or_default();
    let to_unicode = match load_to_unicode(file, font_dict) {
        Some(ToUnicode::Map(map)) => Some(map),
        Some(ToUnicode::Identity) => {
            unicode_codes = true;
            None
        }
        None => None,
    };
    let collection = descendant_dict
        .get("CIDSystemInfo")
        .and_then(|v| file.resolve_dict(v).ok())
        .and_then(|info| {
            let text = |key: &str| match file.resolve(info.get(key)?).ok()? {
                PdfValue::String(s) => Some(String::from_utf8_lossy(&s.0).into_owned()),
                PdfValue::Name(n) => Some(n.clone()),
                _ => None,
            };
            Some((text("Registry")?, text("Ordering")?))
        });
    let descriptor = descendant_dict
        .get("FontDescriptor")
        .and_then(|v| file.resolve_dict(v).ok());
    let glyph_text = descriptor
        .map(|descriptor| embedded::glyph_text(file, descriptor))
        .unwrap_or_default();
    let mac_order = descendant_dict.get("Subtype").and_then(PdfValue::as_name)
        == Some("CIDFontType2")
        && !descriptor.is_some_and(|d| d.contains_key("FontFile2"));
    let cid_to_gid = match descendant_dict.get("CIDToGIDMap") {
        Some(PdfValue::Reference(reference)) => get_stream(file, *reference)
            .ok()
            .and_then(|stream| decode_stream(stream).ok())
            .map(|map| {
                map.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect()
            }),
        _ => None,
    };
    let base_font = font_dict
        .get("BaseFont")
        .and_then(PdfValue::as_name)
        .unwrap_or("");
    let flags = descriptor
        .and_then(|d| d.get("Flags"))
        .and_then(PdfValue::as_integer)
        .unwrap_or(0);
    Ok(CompositeFont {
        encoding,
        face: Face::for_font(base_font, flags),
        unicode_codes,
        default_width,
        widths,
        default_vertical,
        vertical_metrics,
        to_unicode,
        collection,
        glyph_text,
        cid_to_gid,
        mac_order,
        program: descriptor
            .filter(|_| draws)
            .and_then(|descriptor| program::Program::load(file, descriptor)),
    })
}

/// The runs of a CID font's `/W` or `/W2` array, each CID with its values;
/// a malformed run ends the reading, the widths before it standing.
fn cid_runs(file: &PdfFile, value: &PdfValue, stride: usize) -> Vec<(u32, Vec<f64>)> {
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    let number = |v: &PdfValue| file.resolve(v).ok().and_then(PdfValue::as_number);
    let mut runs = Vec::new();
    let mut index = 0usize;
    while let Some(first) = array.get(index).and_then(number) {
        let first = first.max(0.0) as u32;
        match array.get(index + 1).map(|v| file.resolve(v).unwrap_or(v)) {
            Some(PdfValue::Array(values)) => {
                let values: Vec<f64> = values.iter().filter_map(number).collect();
                for (offset, chunk) in values.chunks_exact(stride).enumerate() {
                    runs.push((first.saturating_add(offset as u32), chunk.to_vec()));
                }
                index += 2;
            }
            Some(last) => {
                let Some(last) = number(last) else {
                    break;
                };
                let values: Option<Vec<f64>> = (0..stride)
                    .map(|k| array.get(index + 2 + k).and_then(number))
                    .collect();
                let Some(values) = values else {
                    break;
                };
                let last = (last.max(0.0) as u32).min(first.saturating_add(0xFFFF));
                for cid in first..=last {
                    runs.push((cid, values.clone()));
                }
                index += 2 + stride;
            }
            None => break,
        }
    }
    runs
}

fn parse_cid_widths(file: &PdfFile, value: &PdfValue) -> BTreeMap<u32, f64> {
    cid_runs(file, value, 1)
        .into_iter()
        .map(|(cid, values)| (cid, values[0]))
        .collect()
}

fn parse_cid_vertical_metrics(file: &PdfFile, value: &PdfValue) -> BTreeMap<u32, VerticalMetrics> {
    cid_runs(file, value, 3)
        .into_iter()
        .map(|(cid, v)| {
            let metrics = VerticalMetrics {
                advance: v[0],
                origin_x: Some(v[1]),
                origin_y: v[2],
            };
            (cid, metrics)
        })
        .collect()
}

enum ToUnicode {
    Map(std::sync::Arc<CMap>),
    /// `Identity-H` given as the ToUnicode: each code is its own Unicode value.
    Identity,
}

fn load_to_unicode(file: &PdfFile, font_dict: &crate::objects::PdfDictionary) -> Option<ToUnicode> {
    match font_dict.get("ToUnicode")? {
        PdfValue::Name(name) if name.starts_with("Identity") => Some(ToUnicode::Identity),
        PdfValue::Reference(reference) => {
            // Each form and annotation loads its fonts again, and a map of
            // many wide ranges takes tens of milliseconds to parse.
            if let Some(map) = file.font_programs.lock().unwrap().to_unicode.get(reference) {
                return Some(ToUnicode::Map(map.clone()));
            }
            let stream = get_stream(file, *reference).ok()?;
            let decoded = decode_stream(stream).ok()?;
            let map = std::sync::Arc::new(CMap::parse_to_unicode(&decoded));
            file.font_programs
                .lock()
                .unwrap()
                .to_unicode
                .insert(*reference, map.clone());
            Some(ToUnicode::Map(map))
        }
        _ => None,
    }
}

fn decode_composite_glyphs(font: &CompositeFont, bytes: &[u8]) -> Vec<DecodedGlyph> {
    let mut glyphs = Vec::new();
    let mut byte_index = 0usize;
    while byte_index < bytes.len() {
        let code = font.encoding.next_code(bytes, byte_index);
        let cid = font.encoding.cid(code);
        let text = font
            .to_unicode
            .as_ref()
            .and_then(|map| map.unicode(code))
            .filter(|text| !text.is_empty())
            .map(Cow::into_owned)
            .or_else(|| font.unicode_codes.then(|| unicode_of_code(code)))
            .or_else(|| cid.and_then(|cid| cid_text(font, cid)))
            .unwrap_or_else(|| decode_fallback_cid(cid.unwrap_or(u32::MAX)));
        let width_units = cid
            .and_then(|cid| font.widths.get(&cid))
            .copied()
            .unwrap_or(font.default_width);
        let vertical = font.encoding.vertical.then(|| {
            cid.and_then(|cid| font.vertical_metrics.get(&cid))
                .copied()
                .unwrap_or(font.default_vertical)
        });
        let end = byte_index + code.len as usize;
        glyphs.push(DecodedGlyph {
            text,
            code: code.value,
            cid,
            width_units,
            byte_start: byte_index,
            byte_end: end,
            word_space: code == Code { value: 32, len: 1 },
            vertical,
        });
        byte_index = end;
    }
    glyphs
}

fn cid_text(font: &CompositeFont, cid: u32) -> Option<String> {
    if let Some(text) = font
        .collection
        .as_ref()
        .and_then(|(registry, ordering)| cmap::collection_text(registry, ordering, cid))
    {
        return Some(text);
    }
    let gid = match &font.cid_to_gid {
        Some(map) => *map.get(usize::try_from(cid).ok()?)?,
        None => u16::try_from(cid).ok()?,
    };
    if let Some(character) = font.glyph_text.get(&gid) {
        return Some(character.to_string());
    }
    let name = read_fonts::tables::post::DEFAULT_GLYPH_NAMES.get(usize::from(gid))?;
    font.mac_order
        .then(|| glyph_name_to_char(name))
        .flatten()
        .map(|character| character.to_string())
}

fn unicode_of_code(code: Code) -> String {
    let scalar = match code.len {
        4 if (0xD800_DC00..=0xDBFF_DFFF).contains(&code.value) => {
            let (high, low) = (code.value >> 16, code.value & 0xFFFF);
            0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
        }
        _ => code.value,
    };
    char::from_u32(scalar).unwrap_or('\u{FFFD}').to_string()
}

fn decode_fallback_cid(cid: u32) -> String {
    match cid {
        0x20..=0x7E => char::from_u32(cid).unwrap_or('\u{FFFD}').to_string(),
        _ => '\u{FFFD}'.to_string(),
    }
}

pub(crate) fn matrix_from_operands(operands: &[PdfValue]) -> PdfResult<Matrix> {
    if operands.len() != 6 {
        return Err(PdfError::Corrupt(
            "matrix operator expects six operands".to_string(),
        ));
    }
    Ok(Matrix {
        a: operands[0]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        b: operands[1]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        c: operands[2]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        d: operands[3]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        e: operands[4]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        f: operands[5]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
    })
}

pub(crate) fn operand_name(operation: &Operation, index: usize) -> PdfResult<&str> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_name)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not a name")))
}

pub(crate) fn operand_number(operation: &Operation, index: usize) -> PdfResult<f64> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_number)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not numeric")))
}

pub(crate) fn operand_string(
    operation: &Operation,
    index: usize,
) -> PdfResult<&crate::objects::PdfString> {
    match operation.operands.get(index) {
        Some(PdfValue::String(string)) => Ok(string),
        _ => Err(PdfError::Corrupt(format!(
            "operand {index} is not a string"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CMap, CompositeFont, DEFAULT_VERTICAL, ExtractedPageText, Glyph, LoadedFont,
        SimpleEncoding, SimpleEncodingBase, SimpleFont, StandardMetrics, VerticalMetrics,
        build_search_index, coalesce_match_quads, decode_composite_glyphs, decode_font_glyphs,
        search_page_text,
    };
    use crate::graphics::{Point, Quad, Rect};
    use std::collections::BTreeMap;

    fn composite(encoding: CMap, to_unicode: Option<CMap>) -> CompositeFont {
        CompositeFont {
            encoding,
            face: Default::default(),
            unicode_codes: false,
            default_width: 1000.0,
            widths: BTreeMap::from([(0x41, 500.0)]),
            default_vertical: DEFAULT_VERTICAL,
            vertical_metrics: BTreeMap::new(),
            to_unicode: to_unicode.map(std::sync::Arc::new),
            collection: None,
            glyph_text: Default::default(),
            cid_to_gid: None,
            mac_order: false,
            program: None,
        }
    }

    #[test]
    fn composite_string_splits_on_its_cmap_and_reads_text_and_width_by_code_and_cid() {
        let encoding = CMap::parse(
            b"begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange \
              begincidrange <20> <7F> 1 endcidrange begincidchar <8140> 65 endcidchar",
        );
        let to_unicode = CMap::parse(b"beginbfchar <61> <0078> <8140> <00660069> endbfchar");
        let glyphs = decode_composite_glyphs(&composite(encoding, Some(to_unicode)), b"a\x81\x40 ");
        let read: Vec<_> = glyphs
            .iter()
            .map(|g| {
                (
                    g.text.as_str(),
                    g.width_units,
                    g.byte_end - g.byte_start,
                    g.word_space,
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                ("x", 1000.0, 1, false),
                ("fi", 500.0, 2, false),
                ("\u{FFFD}", 1000.0, 1, true)
            ]
        );
    }

    #[test]
    fn unicode_codes_decode_surrogate_pairs() {
        let mut font = composite(CMap::predefined("UniJIS-UTF16-H").unwrap(), None);
        font.unicode_codes = true;
        let glyphs = decode_composite_glyphs(&font, &[0xD8, 0x40, 0xDC, 0x00, 0x4E, 0x2D]);
        let text: Vec<_> = glyphs.iter().map(|g| g.text.as_str()).collect();
        assert_eq!(text, ["\u{20000}", "\u{4E2D}"]);
    }

    #[test]
    fn a_truetype_left_out_of_the_file_reads_its_cids_in_macintosh_order() {
        let mut font = composite(CMap::identity(false), None);
        font.mac_order = true;
        let glyphs = decode_composite_glyphs(&font, &[0, 57, 0, 3, 0, 36]);
        let text: Vec<_> = glyphs.iter().map(|g| g.text.as_str()).collect();
        assert_eq!(text, ["V", " ", "A"]);
    }

    #[test]
    fn vertical_font_carries_its_metrics_per_cid() {
        let mut font = composite(CMap::identity(true), None);
        font.vertical_metrics.insert(
            7,
            VerticalMetrics {
                advance: -500.0,
                origin_x: Some(250.0),
                origin_y: 800.0,
            },
        );
        let glyphs = decode_composite_glyphs(&font, &[0, 7, 0, 8]);
        assert_eq!(glyphs[0].vertical.map(|v| v.advance), Some(-500.0));
        assert_eq!(glyphs[1].vertical, Some(DEFAULT_VERTICAL));
    }

    #[test]
    fn type3_widths_go_through_the_font_matrix_and_undeclared_codes_read_standard() {
        let font = SimpleFont {
            widths: vec![5.0, 6.0],
            first_char: 97,
            metrics: StandardMetrics::Helvetica,
            face: Default::default(),
            missing_width: 0.0,
            width_scale: 0.1 * 1000.0,
            to_unicode: None,
            encoding: SimpleEncoding {
                base: SimpleEncodingBase::Standard,
                differences: BTreeMap::from([(97, "eacute".to_string())]),
            },
            builtin: Default::default(),
            recovered: BTreeMap::new(),
            glyphs: None,
        };
        let glyphs = decode_font_glyphs(&LoadedFont::Simple(font), b"ab");
        let read: Vec<_> = glyphs
            .iter()
            .map(|g| (g.text.as_str(), g.width_units))
            .collect();
        assert_eq!(read, [("\u{e9}", 500.0), ("b", 600.0)]);
        assert_eq!(
            super::base_char(SimpleEncodingBase::WinAnsi, 0x7F),
            Some('\u{2022}')
        );
        assert_eq!(
            super::glyph_name_to_text("f_f_i.sc").as_deref(),
            Some("ffi")
        );
        assert_eq!(
            super::glyph_name_to_text("uni00660069").as_deref(),
            Some("fi")
        );
    }

    #[test]
    fn generated_glyph_names_give_back_their_code() {
        let names = |pairs: &[(u8, &str)]| {
            pairs
                .iter()
                .map(|(code, name)| (*code, name.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let hex = names(&[
            (32, "C20"),
            (44, "C2c"),
            (65, "C41"),
            (49, "a49"),
            (71, "G47"),
            (72, "g0048"),
            (97, "eacute"),
            (98, "rect"),
        ]);
        let recovered: Vec<_> = super::recover_glyph_names(&hex).into_iter().collect();
        assert_eq!(
            recovered,
            [
                (32, ' '),
                (44, ','),
                (49, '1'),
                (65, 'A'),
                (71, 'G'),
                (72, 'H')
            ]
        );
        let decimal = names(&[(65, "C65"), (66, "c66")]);
        let recovered: Vec<_> = super::recover_glyph_names(&decimal).into_values().collect();
        assert_eq!(recovered, ['A', 'B']);
    }

    #[test]
    fn unreadable_font_keeps_one_unknown_glyph_per_code() {
        let glyphs = decode_font_glyphs(&LoadedFont::Unreadable { code_len: 2 }, &[1, 2, 3]);
        let spans: Vec<_> = glyphs.iter().map(|g| (g.byte_start, g.byte_end)).collect();
        assert_eq!(spans, [(0, 2), (2, 3)]);
    }

    fn make_glyph(text: char, x: f64, y: f64) -> Glyph {
        make_glyph_with_height(text, x, y, 12.0)
    }

    fn make_glyph_with_height(text: char, x: f64, y: f64, height: f64) -> Glyph {
        let rect = Rect {
            x,
            y,
            width: 8.0,
            height,
        };
        Glyph {
            text,
            bbox: rect,
            quad: rect.to_quad(),
            page_char_index: 0,
            operation_index: 0,
            location: super::GlyphLocation::Direct {
                operand_index: 0,
                byte_start: 0,
                byte_end: 1,
            },
            visible: true,
            width_units: 600.0,
            source_form: None,
        }
    }

    #[test]
    fn search_handles_multibyte_utf8_characters() {
        // Text: "café and tea", 'é' is 2 bytes in UTF-8, so byte and char
        // offsets part in normalized_to_display.
        let glyphs: Vec<Glyph> = "café and tea"
            .chars()
            .enumerate()
            .map(|(i, c)| make_glyph(c, i as f64 * 10.0, 100.0))
            .collect();
        let page = ExtractedPageText {
            page_index: 0,
            text: "café and tea".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let matches = search_page_text(&page, "and");
        assert_eq!(matches.len(), 1, "should find exactly one 'and'");
        assert_eq!(
            matches[0].text, "and",
            "match text should be 'and', not a shifted substring"
        );
    }

    #[test]
    fn search_index_byte_alignment() {
        // Verify that normalized_to_display has exactly one entry per byte
        let glyphs: Vec<Glyph> = "aé b"
            .chars()
            .enumerate()
            .map(|(i, c)| make_glyph(c, i as f64 * 10.0, 100.0))
            .collect();
        let page = ExtractedPageText {
            page_index: 0,
            text: "aé b".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let index = build_search_index(&page);
        assert_eq!(
            index.normalized_to_display.len(),
            index.normalized_text.len(),
            "normalized_to_display should have one entry per byte of normalized_text"
        );
    }

    #[test]
    fn coalesces_adjacent_glyph_quads_into_match_regions() {
        let quads = vec![
            Quad {
                points: [
                    Point { x: 10.0, y: 20.0 },
                    Point { x: 14.0, y: 20.0 },
                    Point { x: 14.0, y: 30.0 },
                    Point { x: 10.0, y: 30.0 },
                ],
            },
            Quad {
                points: [
                    Point { x: 14.3, y: 20.0 },
                    Point { x: 18.5, y: 20.0 },
                    Point { x: 18.5, y: 30.0 },
                    Point { x: 14.3, y: 30.0 },
                ],
            },
            Quad {
                points: [
                    Point { x: 50.0, y: 5.0 },
                    Point { x: 54.0, y: 5.0 },
                    Point { x: 54.0, y: 15.0 },
                    Point { x: 50.0, y: 15.0 },
                ],
            },
        ];

        let merged = coalesce_match_quads(&quads);
        assert_eq!(merged.len(), 2);
        let first = merged[0].bounding_rect();
        assert_eq!(
            first.x, 10.0,
            "without padding, rect should start at exact glyph boundary"
        );
        assert_eq!(first.max_x(), 18.5, "first two glyphs should be merged");
    }

    #[test]
    fn sub_pt_rows_are_not_merged_by_line_grouper() {
        // Three rows of 6pt Helvetica (bbox.height ≈ 4.8) sitting only
        // 0.5pt apart in y. The anchor-based check (height_ref * 0.10 =
        // 0.48pt for 6pt glyphs) keeps each row on its own line, so search
        // for "AB" finds zero matches (no adjacency) but each single-letter
        // search hits once.
        let h = 4.8_f64;
        // bbox.y = baseline - 6 * 0.12 = baseline - 0.72
        let row_a_y = 700.0 - 0.72;
        let row_b_y = 699.5 - 0.72;
        let row_c_y = 699.0 - 0.72;
        let glyphs = vec![
            make_glyph_with_height('A', 72.0, row_a_y, h),
            make_glyph_with_height('B', 72.0, row_b_y, h),
            make_glyph_with_height('C', 72.0, row_c_y, h),
        ];
        let page = ExtractedPageText {
            page_index: 0,
            text: "ABC".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let matches_ab = search_page_text(&page, "AB");
        assert!(
            matches_ab.is_empty(),
            "rows A and B must be on separate lines (no 'AB' adjacency); got {:?}",
            matches_ab
        );
        for needle in ["A", "B", "C"] {
            let hits = search_page_text(&page, needle);
            assert_eq!(
                hits.len(),
                1,
                "row {needle} must yield exactly one match; got {hits:?}"
            );
        }
    }

    #[test]
    fn mixed_font_same_baseline_glyphs_merge_into_one_line() {
        // 10pt 'H' and 8pt 'W' on the same baseline. Their y-centres
        // differ by 0.56pt (10pt centre = baseline + 2.8, 8pt centre =
        // baseline + 2.24). With height_ref = 8.0 (10pt bbox.height)
        // the tolerance is 0.80pt, so the 8pt glyph merges into the
        // 10pt-anchored line. The two letters appear consecutively in
        // the visual display string so "HW" matches once.
        let glyphs = vec![
            // 10pt: bbox.y = baseline - 1.2 = -1.2, height = 8.0,
            // centre = -1.2 + 4.0 = 2.8 (baseline = 0).
            make_glyph_with_height('H', 10.0, -1.2, 8.0),
            // 8pt: bbox.y = baseline - 0.96 = -0.96, height = 6.4,
            // centre = -0.96 + 3.2 = 2.24. Place W flush against H's
            // right edge (gap 0) so no synthetic space is injected
            // between them in the display string.
            make_glyph_with_height('W', 18.0, -0.96, 6.4),
        ];
        let page = ExtractedPageText {
            page_index: 0,
            text: "HW".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let matches = search_page_text(&page, "HW");
        assert_eq!(
            matches.len(),
            1,
            "10pt + 8pt same-baseline glyphs must share a visual line; got {matches:?}"
        );
    }
}
