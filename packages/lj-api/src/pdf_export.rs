//! Génération PDF à partir d'un [`DecisionDetail`].
//!
//! La **logique de composition** (citation / corps / provenance,
//! échappement XML du markup, conversion `\n` → `<br/>`) vit dans
//! [`build_decision_story`], puis on rend les octets PDF avec la crate `printpdf`
//! en embarquant **Liberation Sans** (Regular + Bold), libre et métric-compatible
//! Helvetica/Arial.
//!
//! Le word-wrap, le centrage et la pagination sont calculés ici (métriques AFM
//! Helvetica, valables car Liberation Sans en est métric-compatible, positions en
//! points) ; `printpdf` ne sert qu'à émettre le flux d'opérations texte
//! positionnées.

use std::collections::{BTreeMap, BTreeSet};

use lj_dtos::DecisionDetail;
use printpdf::{
    Color, FontId, FontMetrics, Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage,
    PdfSaveOptions, Pt, Rgb, TextItem, TextMatrix,
};

/// Style logique d'un bloc de contenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfBlockStyle {
    /// Titre centré.
    Title,
    /// Ligne de méta.
    Meta,
    /// Paragraphe de corps.
    Body,
    /// Espaceur vertical (hauteur en points).
    Spacer,
}

/// Un bloc de la story PDF : style + contenu déjà échappé (XML ReportLab) prêt
/// à rendre. Pour [`PdfBlockStyle::Spacer`], `content` est vide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfBlock {
    pub style: PdfBlockStyle,
    pub content: String,
}

/// Échappe `&`, `<`, `>` — ces trois caractères seulement.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// Blocks of a decision PDF: the canonical citation, the one the print sheet
/// heads its page with, then the body, then the provenance footer.
pub fn build_decision_story(detail: &DecisionDetail) -> Vec<PdfBlock> {
    let mut story: Vec<PdfBlock> = vec![
        PdfBlock {
            style: PdfBlockStyle::Title,
            content: xml_escape(&detail.title),
        },
        PdfBlock {
            style: PdfBlockStyle::Spacer,
            content: String::new(),
        },
    ];

    for text in &detail.paragraphs {
        let cleaned = xml_escape(text).replace('\n', "<br/>");
        story.push(PdfBlock {
            style: PdfBlockStyle::Body,
            content: cleaned,
        });
    }

    // Pied de provenance/audit (source, ECLI, permalien) — même contenu que le
    // bloc « Source » du web et l'export DOCX (`lj_dtos::provenance_rows`).
    let provenance = lj_dtos::provenance_rows(detail);
    if !provenance.is_empty() {
        story.push(PdfBlock {
            style: PdfBlockStyle::Spacer,
            content: String::new(),
        });
        for (field, value) in provenance {
            story.push(PdfBlock {
                style: PdfBlockStyle::Meta,
                content: xml_escape(&format!("{} : {value}", field.label())),
            });
        }
    }

    story
}

/// Decision PDF as bytes: composition in [`build_decision_story`], binary
/// rendering by `printpdf`.
pub fn build_decision_pdf(detail: &DecisionDetail) -> Vec<u8> {
    let story = build_decision_story(detail);
    render_pdf(&story)
}

// --- Rendu PDF (printpdf, Liberation Sans embarquée) -----------------------

/// Géométrie A4 (points PostScript, 1 mm = 72/25.4 pt). Les positions de texte
/// sont calculées en points (origine bas-gauche) puis posées telles quelles.
const PAGE_W: f64 = 595.275_59; // 210 mm
const PAGE_H: f64 = 841.889_76; // 297 mm
const MARGIN_L: f64 = 56.692_91; // 20 mm
const MARGIN_R: f64 = 56.692_91; // 20 mm
const MARGIN_T: f64 = 51.023_62; // 18 mm
const MARGIN_B: f64 = 51.023_62; // 18 mm

/// Format de page A4 en millimètres, pour `PdfPage::new` (printpdf attend des
/// `Mm` ; le `MediaBox` qui en résulte vaut le `PAGE_W`/`PAGE_H` en points).
const PAGE_W_MM: f32 = 210.0;
const PAGE_H_MM: f32 = 297.0;

/// Attributs typographiques d'un style (taille, interligne, alignement, couleur
/// RGB 0-1). `font` est le code de graisse logique : `"F1"` = Regular,
/// `"F2"` = Bold.
struct StyleAttrs {
    font: &'static str,
    size: f64,
    leading: f64,
    centered: bool,
    color: (f64, f64, f64),
    space_after: f64,
}

fn style_attrs(style: PdfBlockStyle) -> StyleAttrs {
    match style {
        // #1f2a44
        // A whole citation, so the print sheet's 1.3rem rather than a display size:
        // at 18 pt a Cour de cassation citation already spilled onto a second line.
        PdfBlockStyle::Title => StyleAttrs {
            font: "F2", // gras
            size: 15.5,
            leading: 19.0,
            centered: true,
            color: (
                0x1f as f64 / 255.0,
                0x2a as f64 / 255.0,
                0x44 as f64 / 255.0,
            ),
            space_after: 8.0,
        },
        // #4a5568
        PdfBlockStyle::Meta => StyleAttrs {
            font: "F1", // regular
            size: 9.5,
            leading: 12.0,
            centered: false,
            color: (
                0x4a as f64 / 255.0,
                0x55 as f64 / 255.0,
                0x68 as f64 / 255.0,
            ),
            space_after: 4.0,
        },
        // #1f2937
        PdfBlockStyle::Body => StyleAttrs {
            font: "F1",
            size: 10.5,
            leading: 15.0,
            centered: false,
            color: (
                0x1f as f64 / 255.0,
                0x29 as f64 / 255.0,
                0x37 as f64 / 255.0,
            ),
            space_after: 8.0,
        },
        // Spacer : pas de glyphe, hauteur fixe.
        PdfBlockStyle::Spacer => StyleAttrs {
            font: "F1",
            size: 0.0,
            leading: 6.0,
            centered: false,
            color: (0.0, 0.0, 0.0),
            space_after: 0.0,
        },
    }
}

/// Une ligne déjà composée, prête à poser : texte rendu (glyphes visibles) +
/// style + position en points (origine bas-gauche, façon PDF/printpdf).
struct LineCmd {
    text: String,
    font: &'static str,
    size: f64,
    color: (f64, f64, f64),
    x: f64,
    y: f64,
}

/// Une page = ses commandes de texte.
type Page = Vec<LineCmd>;

/// Rend la story en octets PDF (pagination simple, wrapping par largeur).
fn render_pdf(story: &[PdfBlock]) -> Vec<u8> {
    let content_w = PAGE_W - MARGIN_L - MARGIN_R;
    let top = PAGE_H - MARGIN_T;
    let bottom = MARGIN_B;

    let mut pages: Vec<Page> = vec![Vec::new()];
    let mut y = top;

    let newline_break = |pages: &mut Vec<Page>, y: &mut f64| {
        // Saut de page : repart en haut d'une nouvelle page.
        pages.push(Vec::new());
        *y = top;
    };

    for block in story {
        let attrs = style_attrs(block.style);

        if block.style == PdfBlockStyle::Spacer {
            y -= attrs.leading;
            if y < bottom {
                newline_break(&mut pages, &mut y);
            }
            continue;
        }

        // Le contenu est en markup ReportLab (déjà XML-échappé + `<br/>`) :
        // on rétablit les glyphes visibles avant de poser.
        let rendered = unescape_reportlab(&block.content);

        for logical_line in rendered.split('\n') {
            let wrapped = wrap_line(logical_line, attrs.font, attrs.size, content_w);
            for piece in wrapped {
                if y - attrs.leading < bottom {
                    newline_break(&mut pages, &mut y);
                }
                y -= attrs.leading;
                let piece_w = text_width(&piece, attrs.font, attrs.size);
                let x = if attrs.centered {
                    MARGIN_L + (content_w - piece_w) / 2.0
                } else {
                    MARGIN_L
                };
                let page = pages.last_mut().expect("au moins une page");
                page.push(LineCmd {
                    text: piece,
                    font: attrs.font,
                    size: attrs.size,
                    color: attrs.color,
                    x,
                    y,
                });
            }
        }
        y -= attrs.space_after;
        if y < bottom {
            newline_break(&mut pages, &mut y);
        }
    }

    assemble_pdf(&pages)
}

/// Découpe une ligne logique en lignes physiques tenant dans `max_w` (greedy
/// word-wrap sur les espaces ; un mot trop long n'est pas coupé).
fn wrap_line(line: &str, font: &str, size: f64, max_w: f64) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if text_width(&candidate, font, size) <= max_w || current.is_empty() {
            current = candidate;
        } else {
            out.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

/// Largeur d'une chaîne en points pour une police base-14 (métriques AFM
/// Helvetica / Helvetica-Bold, en 1/1000 d'em).
fn text_width(text: &str, font: &str, size: f64) -> f64 {
    let widths = if font == "F2" {
        &HELVETICA_BOLD_WIDTHS
    } else {
        &HELVETICA_WIDTHS
    };
    let mut total = 0u32;
    for ch in text.chars() {
        let code = winansi_code(ch) as usize;
        total += widths[code] as u32;
    }
    total as f64 / 1000.0 * size
}

/// Rétablit les glyphes visibles du markup ReportLab : `<br/>` → `\n`, puis
/// dés-échappement XML (`&lt;`/`&gt;`/`&amp;`).
fn unescape_reportlab(markup: &str) -> String {
    let with_breaks = markup.replace("<br/>", "\n");
    with_breaks
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Code WinAnsiEncoding (Windows-1252) d'un caractère Unicode ; `?` (0x3F) en
/// repli. Couvre l'ASCII imprimable + les diacritiques du français.
fn winansi_code(ch: char) -> u8 {
    let c = ch as u32;
    if (0x20..=0x7E).contains(&c) {
        return c as u8;
    }
    match ch {
        '€' => 0x80,
        '\u{2018}' => 0x91, // ‘
        '\u{2019}' => 0x92, // ’
        '\u{201C}' => 0x93, // “
        '\u{201D}' => 0x94, // ”
        '\u{2022}' => 0x95, // •
        '\u{2013}' => 0x96, // – en dash
        '\u{2014}' => 0x97, // — em dash
        '\u{2026}' => 0x85, // …
        '\u{00A0}' => 0x20, // espace insécable → espace
        '\u{0152}' => 0x8C, // Œ
        '\u{0153}' => 0x9C, // œ
        '\u{0178}' => 0x9F, // Ÿ
        // Latin-1 (0xA0-0xFF) : identité WinAnsi pour la plage des accents FR.
        _ if (0xA0..=0xFF).contains(&c) => c as u8,
        _ => b'?',
    }
}

// --- Police embarquée (Liberation Sans, OFL) -------------------------------
//
// Liberation Sans est métric-compatible Helvetica/Arial : nos tables AFM
// (`text_width`) restent valides pour le wrap/centrage. Une police embarquée
// passe par l'encodage glyph-id (`Identity-H`), correct pour tout l'Unicode.
// printpdf en `default-features=false` n'a pas de parseur de police : on
// alimente ses maps glyphes (cmap + hmtx) via ttf-parser.

static LIBERATION_SANS: &[u8] = include_bytes!("../data/fonts/LiberationSans-Regular.ttf");
static LIBERATION_SANS_BOLD: &[u8] = include_bytes!("../data/fonts/LiberationSans-Bold.ttf");

/// Construit le `ParsedFont` printpdf d'une police TTF, restreint aux caractères
/// réellement posés (`used`) : map codepoint→glyphe (cmap) + largeurs glyphe
/// (hmtx). printpdf embarque la police + une table ToUnicode/W dérivée de ces maps.
fn load_font(ttf: &'static [u8], used: &BTreeSet<char>) -> ParsedFont {
    let face = ttf_parser::Face::parse(ttf, 0).expect("police Liberation Sans valide");
    let mut codepoint_to_glyph = BTreeMap::new();
    let mut glyph_widths = BTreeMap::new();
    for &ch in used {
        if let Some(gid) = face.glyph_index(ch) {
            codepoint_to_glyph.insert(ch as u32, gid.0);
            glyph_widths.insert(gid.0, face.glyph_hor_advance(gid).unwrap_or(0));
        }
    }
    ParsedFont::with_glyph_data(
        ttf.to_vec(),
        0,
        Some("LiberationSans".to_string()),
        codepoint_to_glyph,
        glyph_widths,
        face.units_per_em(),
        FontMetrics {
            ascent: face.ascender(),
            descent: face.descender(),
        },
    )
}

/// Assemble les pages (lignes pré-positionnées) en octets PDF via `printpdf`.
///
/// Chaque [`LineCmd`] porte une position absolue en points (origine bas-gauche,
/// déjà calculée par [`render_pdf`]) : on émet un `Op::SetTextMatrix` (opérateur
/// PDF `Tm`, **absolu**) par ligne. `printpdf` sérialise le document (catalog /
/// pages / fonts embarquées / xref).
fn assemble_pdf(pages: &[Page]) -> Vec<u8> {
    let mut doc = PdfDocument::new("Decision");

    // Caractères posés par graisse (`F2` = gras) → maps glyphes minimales.
    let mut used_regular = BTreeSet::new();
    let mut used_bold = BTreeSet::new();
    for page in pages {
        for cmd in page {
            let set = if cmd.font == "F2" {
                &mut used_bold
            } else {
                &mut used_regular
            };
            set.extend(cmd.text.chars());
        }
    }
    let regular_id = doc.add_font(&load_font(LIBERATION_SANS, &used_regular));
    let bold_id = doc.add_font(&load_font(LIBERATION_SANS_BOLD, &used_bold));
    let font_id = |code: &str| -> &FontId {
        if code == "F2" {
            &bold_id
        } else {
            &regular_id
        }
    };

    let pdf_pages: Vec<PdfPage> = pages
        .iter()
        .map(|page| {
            let mut ops: Vec<Op> = Vec::with_capacity(page.len() * 4 + 2);
            ops.push(Op::StartTextSection);
            for cmd in page {
                let (r, g, b) = cmd.color;
                ops.push(Op::SetTextMatrix {
                    matrix: TextMatrix::Translate(Pt(cmd.x as f32), Pt(cmd.y as f32)),
                });
                ops.push(Op::SetFont {
                    font: PdfFontHandle::External(font_id(cmd.font).clone()),
                    size: Pt(cmd.size as f32),
                });
                ops.push(Op::SetFillColor {
                    col: Color::Rgb(Rgb {
                        r: r as f32,
                        g: g as f32,
                        b: b as f32,
                        icc_profile: None,
                    }),
                });
                ops.push(Op::ShowText {
                    items: vec![TextItem::Text(cmd.text.clone())],
                });
            }
            ops.push(Op::EndTextSection);
            PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), ops)
        })
        .collect();

    doc.with_pages(pdf_pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

// --- Métriques AFM base-14 (largeurs en 1/1000 d'em, indexées WinAnsi) ------
//
// Valeurs des fichiers AFM Adobe officiels (Helvetica.afm / Helvetica-Bold.afm),
// réindexées par code WinAnsiEncoding. Servent au word-wrap et au centrage.
// Les codes non assignés portent la largeur de l'espace.

/// Largeurs Helvetica (regular).
static HELVETICA_WIDTHS: [u16; 256] = build_helvetica_widths();
/// Largeurs Helvetica-Bold.
static HELVETICA_BOLD_WIDTHS: [u16; 256] = build_helvetica_bold_widths();

const fn build_helvetica_widths() -> [u16; 256] {
    let mut w = [278u16; 256];
    // ASCII imprimable (0x20-0x7E).
    let ascii: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    let mut i = 0;
    while i < 95 {
        w[0x20 + i] = ascii[i];
        i += 1;
    }
    // Latin-1 / WinAnsi diacritiques utiles (français).
    w[0x80] = 556; // euro
    w[0x85] = 1000; // ellipsis
    w[0x91] = 222; // ‘
    w[0x92] = 222; // ’
    w[0x93] = 333; // “
    w[0x94] = 333; // ”
    w[0x95] = 350; // bullet
    w[0x96] = 556; // en dash
    w[0x97] = 1000; // em dash
    w[0x8C] = 1000; // OE
    w[0x9C] = 944; // oe
    w[0x9F] = 667; // Ydieresis
    w[0xA0] = 278; // nbsp
    w[0xC0] = 667; // Agrave
    w[0xC1] = 667; // Aacute
    w[0xC2] = 667; // Acirc
    w[0xC3] = 667;
    w[0xC4] = 667;
    w[0xC5] = 667;
    w[0xC6] = 1000; // AE
    w[0xC7] = 722; // Ccedilla
    w[0xC8] = 667; // Egrave
    w[0xC9] = 667; // Eacute
    w[0xCA] = 667; // Ecirc
    w[0xCB] = 667; // Edieresis
    w[0xCC] = 278; // Igrave
    w[0xCD] = 278;
    w[0xCE] = 278;
    w[0xCF] = 278;
    w[0xD1] = 722; // Ntilde
    w[0xD2] = 778; // Ograve
    w[0xD3] = 778;
    w[0xD4] = 778;
    w[0xD5] = 778;
    w[0xD6] = 778;
    w[0xD9] = 722; // Ugrave
    w[0xDA] = 722;
    w[0xDB] = 722;
    w[0xDC] = 722;
    w[0xDF] = 611; // germandbls
    w[0xE0] = 556; // agrave
    w[0xE1] = 556; // aacute
    w[0xE2] = 556; // acirc
    w[0xE3] = 556;
    w[0xE4] = 556;
    w[0xE5] = 556;
    w[0xE6] = 889; // ae
    w[0xE7] = 500; // ccedilla
    w[0xE8] = 556; // egrave
    w[0xE9] = 556; // eacute
    w[0xEA] = 556; // ecirc
    w[0xEB] = 556; // edieresis
    w[0xEC] = 222; // igrave
    w[0xED] = 222;
    w[0xEE] = 222;
    w[0xEF] = 222;
    w[0xF1] = 556; // ntilde
    w[0xF2] = 556; // ograve
    w[0xF3] = 556;
    w[0xF4] = 556;
    w[0xF5] = 556;
    w[0xF6] = 556;
    w[0xF9] = 556; // ugrave
    w[0xFA] = 556;
    w[0xFB] = 556;
    w[0xFC] = 556;
    w
}

const fn build_helvetica_bold_widths() -> [u16; 256] {
    let mut w = [278u16; 256];
    let ascii: [u16; 95] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722,
        722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611,
        611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556,
        500, 389, 280, 389, 584,
    ];
    let mut i = 0;
    while i < 95 {
        w[0x20 + i] = ascii[i];
        i += 1;
    }
    w[0x80] = 556;
    w[0x85] = 1000;
    w[0x91] = 278;
    w[0x92] = 278;
    w[0x93] = 500;
    w[0x94] = 500;
    w[0x95] = 350;
    w[0x96] = 556;
    w[0x97] = 1000;
    w[0x8C] = 1000;
    w[0x9C] = 944;
    w[0x9F] = 667;
    w[0xA0] = 278;
    w[0xC0] = 722;
    w[0xC1] = 722;
    w[0xC2] = 722;
    w[0xC3] = 722;
    w[0xC4] = 722;
    w[0xC5] = 722;
    w[0xC6] = 1000;
    w[0xC7] = 722;
    w[0xC8] = 667;
    w[0xC9] = 667;
    w[0xCA] = 667;
    w[0xCB] = 667;
    w[0xCC] = 278;
    w[0xCD] = 278;
    w[0xCE] = 278;
    w[0xCF] = 278;
    w[0xD1] = 722;
    w[0xD2] = 778;
    w[0xD3] = 778;
    w[0xD4] = 778;
    w[0xD5] = 778;
    w[0xD6] = 778;
    w[0xD9] = 722;
    w[0xDA] = 722;
    w[0xDB] = 722;
    w[0xDC] = 722;
    w[0xDF] = 611;
    w[0xE0] = 556;
    w[0xE1] = 556;
    w[0xE2] = 556;
    w[0xE3] = 556;
    w[0xE4] = 556;
    w[0xE5] = 556;
    w[0xE6] = 889;
    w[0xE7] = 556;
    w[0xE8] = 556;
    w[0xE9] = 556;
    w[0xEA] = 556;
    w[0xEB] = 556;
    w[0xEC] = 278;
    w[0xED] = 278;
    w[0xEE] = 278;
    w[0xEF] = 278;
    w[0xF1] = 611;
    w[0xF2] = 611;
    w[0xF3] = 611;
    w[0xF4] = 611;
    w[0xF5] = 611;
    w[0xF6] = 611;
    w[0xF9] = 611;
    w[0xFA] = 611;
    w[0xFB] = 611;
    w[0xFC] = 611;
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::JurisdictionType;

    fn detail() -> DecisionDetail {
        DecisionDetail {
            court_names: Default::default(),
            id: "abc123".to_string(),
            jurisdiction_type: JurisdictionType::Cc,
            title: "Cour de cassation, chambre sociale, 29 mai 2026, 24-17.384".to_string(),
            case_name: None,
            paragraphs: vec!["Premier <para> & suite".to_string(), "Second".to_string()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            summary_quote: None,
            summary_body: None,
            summary_lang: None,
            jurisdiction_code: None,
            jurisdiction_name: Some("Cour de cassation".to_string()),
            court_place: None,
            date_lecture: Some("2026-05-29".to_string()),
            solution: None,
            formation: Vec::new(),
            legal_domain: None,
            publication_codes: Vec::new(),
            date_audience: None,
            docket_numbers: Some(vec!["24-17.384".to_string()]),
            seat: Some("Chambre sociale".to_string()),
            seat_parts: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: Vec::new(),
            commentaires: vec![],
            ruling_url: None,
            citantes: vec![],
            citantes_total: None,
            lang: None,
            texts: Vec::new(),
        }
    }

    #[test]
    fn xml_escape_only_amp_lt_gt() {
        assert_eq!(
            xml_escape("a & b < c > d \" '"),
            "a &amp; b &lt; c &gt; d \" '"
        );
    }

    #[test]
    fn story_opens_on_the_citation() {
        let story = build_decision_story(&detail());
        assert_eq!(story.len(), 6);
        assert_eq!(story[0].style, PdfBlockStyle::Title);
        assert_eq!(
            story[0].content,
            "Cour de cassation, chambre sociale, 29 mai 2026, 24-17.384"
        );
        assert_eq!(story[1].style, PdfBlockStyle::Spacer);
        assert_eq!(story[2].style, PdfBlockStyle::Body);
        assert_eq!(story[2].content, "Premier &lt;para&gt; &amp; suite");
        assert_eq!(story[4].style, PdfBlockStyle::Spacer);
        assert_eq!(story[5].style, PdfBlockStyle::Meta);
        assert_eq!(
            story[5].content,
            "Permalien : https://librejustice.fr/decision/fr/abc123"
        );
    }

    #[test]
    fn pdf_is_well_formed() {
        let bytes = build_decision_pdf(&detail());
        assert!(bytes.starts_with(b"%PDF-"), "préfixe %PDF- manquant");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("%%EOF"), "marqueur %%EOF manquant");
        assert!(text.contains("startxref"), "table xref manquante");
        assert!(text.contains("/Catalog"), "catalog racine manquant");
        assert!(text.contains("/Type0"), "police Type0 (composite) attendue");
        assert!(text.contains("Identity-H"), "encodage Identity-H attendu");
        assert!(text.contains("FontFile2"), "flux police TrueType embarqué");
        assert!(text.contains("LiberationSans"), "BaseFont Liberation Sans");
    }
}
