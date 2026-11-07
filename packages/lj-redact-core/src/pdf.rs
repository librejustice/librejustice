//! PDF backend: glyph quads in, glyph-precise removal out.

use anyhow::{anyhow, Result};
use lj_pdf::objects::parse_pdf;
use lj_pdf::text::analyze_page_text;
use lj_pdf::{PdfDocument as Engine, RedactionPlan, RedactionTarget};

use crate::index::Anchor;
use crate::{layout, locate, survivors, Outcome};

/// Remove every occurrence of `needles` from a PDF, along with whatever the
/// `rects` cover, then verify on the bytes produced that no needle survives.
pub fn redact(bytes: &[u8], needles: &[String], rects: &[PageRect]) -> Result<Outcome> {
    let source = glyphs(bytes)?;
    let source_chars = source.len();
    let occurrences = locate(&source, needles);
    let found = occurrences.len();
    let anchors: Vec<Anchor> = occurrences.into_iter().flat_map(|o| o.anchors).collect();

    let mut document = PdfDocument::open(bytes)?;
    let mut removed = document.redact(targets_of(&anchors))?;
    // One pass per rectangle. Given several in the same plan, the engine blacks
    // out their bounding box on a page drawn as an image, and with it the page
    // between them.
    for rect in rects {
        removed += document.redact(rect_targets(std::slice::from_ref(rect)))?;
    }
    if !needles.is_empty() {
        document.clear_strings(needles);
    }
    let produced = document.save()?;

    let mut leaks = survivors(&glyphs(&produced)?, needles);
    for text in Engine::open(&produced)
        .map_err(|e| anyhow!("parsing the PDF: {e}"))?
        .strings()
    {
        leaks.extend(survivors(&string_glyphs(&text), needles));
    }
    leaks.sort_unstable();
    leaks.dedup();
    Ok(Outcome {
        bytes: produced,
        found,
        removed,
        leaks,
        source_chars,
    })
}

/// A PDF held in memory, opened once and redacted in place.
pub struct PdfDocument {
    engine: Engine,
}

impl PdfDocument {
    /// Parse `bytes`, refusing rather than guessing when the structure is not
    /// one the engine handles.
    pub fn open(bytes: &[u8]) -> Result<Self> {
        let engine = Engine::open(bytes).map_err(|e| anyhow!("parsing the PDF: {e}"))?;
        Ok(Self { engine })
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.engine.page_count()
    }

    /// Apply a plan built from [`targets_of`] and clean metadata, annotations
    /// and attachments along the way, even when the plan names nothing.
    pub fn redact(&mut self, targets: Vec<RedactionTarget>) -> Result<usize> {
        let plan = RedactionPlan {
            targets,
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(true),
            strip_metadata: Some(true),
            strip_attachments: Some(true),
            sanitize_hidden_ocgs: None,
        };
        let report = self
            .engine
            .apply_redactions(plan)
            .map_err(|e| anyhow!("applying the redaction: {e}"))?;
        Ok(report.text_glyphs_removed)
    }

    /// Empty every string of the document's dictionaries that still carries
    /// one of `needles`, as an outline title or the note of an annotation the
    /// removed glyphs did not touch.
    pub fn clear_strings(&mut self, needles: &[String]) {
        self.engine
            .clear_strings(|text| !survivors(&string_glyphs(text), needles).is_empty());
    }

    /// Serialise the document as it now stands, as a full rewrite.
    pub fn save(&self) -> Result<Vec<u8>> {
        self.engine
            .save()
            .map_err(|e| anyhow!("writing the PDF: {e}"))
    }
}

/// Every glyph of every page, in reading order, each with its anchor.
///
/// Invisible glyphs are kept: a scan carrying an OCR layer drawn in `Tr 3` is
/// exactly the document where a name hides under a picture of itself. Pages are
/// separated by a newline carrying the previous glyph's anchor, so no match
/// straddles two pages, and so are lines and words the producer set apart
/// without writing the space.
pub fn glyphs(bytes: &[u8]) -> Result<Vec<(char, Anchor)>> {
    let parsed = parse_pdf(bytes).map_err(|e| anyhow!("parsing the PDF: {e}"))?;
    let mut out: Vec<(char, Anchor)> = Vec::new();
    for (page, info) in parsed.pages.iter().enumerate() {
        let extracted = analyze_page_text(&parsed.file, page, info)
            .map_err(|e| anyhow!("extracting page {page}: {e}"))?;
        if let Some(last) = out.last().map(|(_, a)| *a) {
            out.push(('\n', last));
        }
        let mut previous: Option<(char, Anchor)> = None;
        for glyph in extracted.glyphs {
            let quad = glyph.quad.points.map(|p| [p.x, p.y]);
            let anchor = Anchor::PdfGlyph { page, quad };
            if let Some((c, before @ Anchor::PdfGlyph { quad: last, .. })) = previous {
                if let Some(gap) = gap(c, &last, glyph.text, &quad) {
                    out.push((gap, before));
                }
            }
            out.push((glyph.text, anchor));
            previous = Some((glyph.text, anchor));
        }
    }
    Ok(out)
}

/// A string of a dictionary as glyphs set side by side on no page.
fn string_glyphs(text: &str) -> Vec<(char, Anchor)> {
    text.chars()
        .enumerate()
        .map(|(at, c)| {
            let x = at as f64;
            let quad = [[x, 0.0], [x + 1.0, 0.0], [x + 1.0, 1.0], [x, 1.0]];
            (c, Anchor::PdfGlyph { page: 0, quad })
        })
        .collect()
}

/// The break the producer drew by moving the pen without writing it: a line
/// ends on its last glyph and the next one starts on its first. A jump back on
/// the same baseline is no break: a font without widths, drawn in pieces
/// placed apart, runs each piece past the start of the next.
fn gap(c: char, last: &[[f64; 2]; 4], next: char, quad: &[[f64; 2]; 4]) -> Option<char> {
    if c.is_whitespace() || next.is_whitespace() {
        return None;
    }
    let (last, quad) = (layout::bounds(last), layout::bounds(quad));
    let inline = layout::same_baseline(&last, &quad) || layout::raised(&last, &quad);
    match (inline, layout::apart(&last, &quad)) {
        (false, _) => Some('\n'),
        (true, true) => Some(' '),
        (true, false) => None,
    }
}

/// A rectangle of one page, in PDF user space, as a viewer hands it over.
///
/// The engine rewrites the pixels of an image the rectangle covers, so this is
/// the only honest way to hide something a scan draws rather than writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRect {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One target per rectangle, left whole rather than merged: the engine reads
/// each against the images it covers.
pub fn rect_targets(rects: &[PageRect]) -> Vec<RedactionTarget> {
    rects
        .iter()
        .map(|r| RedactionTarget::Rect {
            page_index: r.page,
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        })
        .collect()
}

/// Turn the anchors of the located occurrences into one redaction target per
/// page, carrying the quad of every glyph to remove.
pub fn targets_of(anchors: &[Anchor]) -> Vec<RedactionTarget> {
    let mut targets: Vec<RedactionTarget> = Vec::new();
    let mut of_page: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for anchor in anchors {
        let Some((page, quad)) = anchor.pdf() else {
            continue;
        };
        let points = quad.map(|p| lj_pdf::graphics::Point { x: p[0], y: p[1] });
        let index = *of_page.entry(page).or_insert_with(|| {
            targets.push(RedactionTarget::QuadGroup {
                page_index: page,
                quads: Vec::new(),
            });
            targets.len() - 1
        });
        if let RedactionTarget::QuadGroup { quads, .. } = &mut targets[index] {
            quads.push(points);
        }
    }
    targets
}

/// The picture a scanned page is made of, as the file stores it: the reader
/// reads its own pixels, and a box found there reaches the page through
/// `transform`, which stretches the picture's unit square, top left corner at
/// the origin, over the page in points with y down.
pub struct ScanPicture {
    pub mime: &'static str,
    pub data: std::sync::Arc<[u8]>,
    pub transform: [f64; 6],
}

/// Share of the page a picture must cover for the page to be read on that
/// picture rather than on its drawing.
const SCAN_COVER: f64 = 0.5;

/// Every picture drawn, with the matrix that places it on the page.
fn pictures(
    items: &[lj_draw::Item],
    parent: lj_draw::Matrix,
    out: &mut Vec<(lj_draw::Image, lj_draw::Matrix)>,
) {
    for item in items {
        match item {
            lj_draw::Item::Image(image) => out.push((image.clone(), image.transform.then(parent))),
            lj_draw::Item::Group(group) => {
                pictures(&group.items, group.transform.then(parent), out)
            }
            _ => {}
        }
    }
}

/// The picture page `index` is made of: the page's only picture, when it
/// covers at least half of the page, upright and in its own colours. Any other
/// page is read on its drawing: a scan a copier split into a background and
/// tiles of text, or a picture turned on the page, reads as noise pixel for
/// pixel.
pub fn scan_picture(engine: &Engine, index: usize) -> Result<Option<ScanPicture>> {
    let page = engine
        .draw_page(index)
        .map_err(|e| anyhow!("drawing page {index}: {e}"))?;
    let mut found = Vec::new();
    pictures(&page.items, lj_draw::Matrix::IDENTITY, &mut found);
    let [(image, m)] = &found[..] else {
        return Ok(None);
    };
    let upright = m.b == 0.0 && m.c == 0.0 && m.a > 0.0 && m.d > 0.0;
    let corners = [[0.0, 0.0], [1.0, 1.0]].map(|p| m.apply(p));
    let span = |axis: usize, limit: f64| {
        (corners[1][axis].min(limit) - corners[0][axis].max(0.0)).max(0.0)
    };
    let share = span(0, page.width) * span(1, page.height) / (page.width * page.height);
    Ok(
        (upright && image.recolor.is_none() && share >= SCAN_COVER).then(|| ScanPicture {
            mime: image.mime,
            data: image.data.clone(),
            transform: [m.a, m.b, m.c, m.d, m.e, m.f],
        }),
    )
}

/// Whether page `index` names a picture, whose pixels may carry text the file
/// does not. Read from its resources, without drawing the page.
pub fn names_picture(engine: &Engine, index: usize) -> Result<bool> {
    engine
        .page_names_picture(index)
        .map_err(|e| anyhow!("reading page {index}: {e}"))
}

/// One word a text reader found on a rasterised page, in that page's own
/// coordinates. Words sharing a `line` are one run of text; a new line, or a new
/// page, breaks the run.
#[derive(Clone)]
pub struct ReadWord {
    pub page: usize,
    pub line: usize,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Share of a word's box given to one of its characters. Punctuation is about
/// half a letter wide: an even split left the end of a name before its comma
/// outside the rectangle.
fn advance(character: char) -> f64 {
    match character {
        '.' | ',' | ';' | ':' | '\'' | '’' | '!' | '(' | ')' => 0.5,
        _ => 1.0,
    }
}

/// What a text reader found, as the characters of a document: one quad per
/// character, cut from its word's box, a space between the words of a line and
/// a newline between lines and pages.
pub fn word_glyphs(words: &[ReadWord]) -> Vec<(char, Anchor)> {
    let mut characters: Vec<(char, Anchor)> = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for word in words {
        let quad = |x0: f64, x1: f64| {
            [
                [x0, word.y],
                [x1, word.y],
                [x1, word.y + word.height],
                [x0, word.y + word.height],
            ]
        };
        if let Some(previous) = run {
            let separator = match previous == (word.page, word.line) {
                true => ' ',
                false => '\n',
            };
            characters.push((
                separator,
                Anchor::PdfGlyph {
                    page: word.page,
                    quad: quad(word.x, word.x),
                },
            ));
        }
        let total: f64 = word.text.chars().map(advance).sum();
        let mut at = 0.0;
        for character in word.text.chars() {
            let x0 = word.x + word.width * at / total;
            at += advance(character);
            let x1 = word.x + word.width * at / total;
            characters.push((
                character,
                Anchor::PdfGlyph {
                    page: word.page,
                    quad: quad(x0, x1),
                },
            ));
        }
        run = Some((word.page, word.line));
    }
    characters
}

/// What a text reader found, added to the characters the file yields on its
/// own. A word read over a glyph the file draws is that glyph read a second
/// time, and is dropped. Each page keeps the file's characters, then the words
/// read on its pictures.
pub fn merge_read(native: &[(char, Anchor)], words: &[ReadWord]) -> Vec<(char, Anchor)> {
    let page_of = |(_, anchor): &(char, Anchor)| anchor.pdf().map(|(page, _)| page);
    let centre = |(_, anchor): &(char, Anchor)| {
        anchor.pdf().map(|(page, quad)| {
            let x = quad.iter().map(|p| p[0]).sum::<f64>() / 4.0;
            let y = quad.iter().map(|p| p[1]).sum::<f64>() / 4.0;
            (page, x, y)
        })
    };
    let centres: Vec<(usize, f64, f64)> = native.iter().filter_map(centre).collect();
    let over_a_glyph = |word: &ReadWord| {
        centres.iter().any(|&(page, x, y)| {
            page == word.page
                && (word.x..=word.x + word.width).contains(&x)
                && (word.y..=word.y + word.height).contains(&y)
        })
    };
    let kept: Vec<ReadWord> = words.iter().filter(|w| !over_a_glyph(w)).cloned().collect();

    let mut pages: Vec<usize> = native
        .iter()
        .filter_map(page_of)
        .chain(kept.iter().map(|w| w.page))
        .collect();
    pages.sort_unstable();
    pages.dedup();
    let mut out: Vec<(char, Anchor)> = Vec::new();
    for page in pages {
        out.extend(native.iter().filter(|g| page_of(g) == Some(page)).cloned());
        let read: Vec<ReadWord> = kept.iter().filter(|w| w.page == page).cloned().collect();
        let Some(first) = read.first() else {
            continue;
        };
        if out.last().is_some_and(|(c, _)| !c.is_whitespace()) {
            let edge = [
                [first.x, first.y],
                [first.x, first.y],
                [first.x, first.y + first.height],
                [first.x, first.y + first.height],
            ];
            out.push(('\n', Anchor::PdfGlyph { page, quad: edge }));
        }
        out.extend(word_glyphs(&read));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(page: usize, x: f64) -> Anchor {
        Anchor::PdfGlyph {
            page,
            quad: [[x, 0.0], [x + 5.0, 0.0], [x + 5.0, 10.0], [x, 10.0]],
        }
    }

    fn quad_count(target: &RedactionTarget) -> usize {
        match target {
            RedactionTarget::QuadGroup { quads, .. } => quads.len(),
            _ => 0,
        }
    }

    fn at(x: f64, y: f64) -> [[f64; 2]; 4] {
        [[x, y], [x + 5.0, y], [x + 5.0, y + 10.0], [x, y + 10.0]]
    }

    #[test]
    fn a_read_word_joins_the_file_s_text_unless_the_file_already_draws_it() {
        let native: Vec<(char, Anchor)> = "Dupont"
            .chars()
            .enumerate()
            .map(|(i, c)| (c, glyph(0, 72.0 + 5.0 * i as f64)))
            .collect();
        let word = |page, text: &str, x| ReadWord {
            page,
            line: 0,
            text: text.into(),
            x,
            y: 0.0,
            width: 30.0,
            height: 10.0,
        };
        let merged = merge_read(
            &native,
            &[
                word(0, "Dupond", 72.0),
                word(0, "Forbach", 300.0),
                word(1, "Metz", 72.0),
            ],
        );
        let text: String = merged.iter().map(|(c, _)| c).collect();
        assert_eq!(text, "Dupont\nForbach\nMetz");
        let pages: Vec<usize> = merged
            .iter()
            .filter_map(|(_, a)| a.pdf())
            .map(|(p, _)| p)
            .collect();
        assert!(pages.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_line_or_a_gap_the_producer_left_unwritten_becomes_a_separator() {
        assert_eq!(
            gap('s', &at(300.0, 700.0), 'e', &at(72.0, 680.0)),
            Some('\n')
        );
        assert_eq!(
            gap('s', &at(100.0, 700.0), 'e', &at(110.0, 700.0)),
            Some(' ')
        );
        assert_eq!(gap('s', &at(100.0, 700.0), 'e', &at(105.0, 700.0)), None);
        assert_eq!(gap(' ', &at(300.0, 700.0), 'e', &at(72.0, 680.0)), None);
        assert_eq!(gap('m', &at(495.0, 700.0), 'o', &at(392.0, 700.0)), None);
        assert_eq!(gap('M', &at(100.0, 700.0), 'm', &at(105.0, 706.0)), None);
    }

    fn pdf(objects: &[&str]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (at, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", at + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        let trailer = format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        );
        out.extend(trailer.bytes());
        out
    }

    #[test]
    fn a_name_leaves_the_strings_no_glyph_draws_and_the_metadata_and_thumbnail_go_even_when_no_page_shows_them(
    ) {
        let content = "BT /F1 12 Tf 72 700 Td (Monsieur Dupontel est venu) Tj ET";
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /PageLabels << /Nums [0 << /P (Dupontel-) >>] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 8 0 R >> >> /Annots [9 0 R] /Thumb 10 0 R >>",
            &format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Outlines /First 6 0 R /Last 6 0 R /Count 1 >>",
            "<< /Title <FEFF0043006800610070002E0020004400750070006F006E00740065006C> /Parent 5 0 R >>",
            "<< /Author (Zorglub) >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Annot /Subtype /Link /Rect [500 50 520 70] /A << /S /URI /URI (http://exemple.fr/Dupontel) >> /Contents (Note sur Dupontel) >>",
            "<< /Width 3 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 3 >>\nstream\nDup\nendstream",
        ]);
        let needles = ["Dupontel".to_string(), "Zorglub".to_string()];
        let outcome = redact(&bytes, &needles, &[]).unwrap();
        assert!(outcome.leaks.is_empty());
        let strings = Engine::open(&outcome.bytes).unwrap().strings();
        assert!(!strings.is_empty());
        assert!(strings
            .iter()
            .all(|s| !s.contains("Dupontel") && !s.contains("Zorglub")));
        assert!(!outcome.bytes.windows(6).any(|w| w == b"Thumb "));
        assert!(glyphs(&outcome.bytes)
            .unwrap()
            .iter()
            .any(|(c, _)| *c == 'M'));
    }

    #[test]
    fn glyphs_of_one_page_gather_into_a_single_target() {
        let targets = targets_of(&[glyph(0, 0.0), glyph(0, 5.0), glyph(0, 10.0)]);
        assert_eq!(targets.len(), 1);
        assert_eq!(quad_count(&targets[0]), 3);
    }

    #[test]
    fn glyphs_on_different_pages_never_share_a_target() {
        let targets = targets_of(&[glyph(0, 0.0), glyph(1, 5.0), glyph(0, 10.0)]);
        assert_eq!(targets.len(), 2);
        assert_eq!(quad_count(&targets[0]), 2);
        assert_eq!(quad_count(&targets[1]), 1);
    }
}
