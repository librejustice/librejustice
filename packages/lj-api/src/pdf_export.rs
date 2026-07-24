//! Génération PDF à partir d'un [`DecisionDetail`] — port de `pdf_export.py`.
//!
//! Le Python s'appuie sur ReportLab (`SimpleDocTemplate` + `Paragraph`). Côté
//! Rust on porte la **logique de composition** (titre / sous-titre / méta /
//! corps, échappement XML ReportLab, conversion `\n` → `<br/>`) dans
//! [`build_decision_story`], puis on rend les octets PDF avec la crate `printpdf`
//! en embarquant **Liberation Sans** (Regular + Bold), libre et métric-compatible
//! Helvetica/Arial.
//!
//! Le word-wrap, le centrage et la pagination restent calculés ici (métriques AFM
//! Helvetica — valables car Liberation Sans en est métric-compatible, positions en
//! points) pour conserver le découpage en lignes ; `printpdf` ne sert qu'à émettre
//! le flux d'opérations texte positionnées.

use std::collections::{BTreeMap, BTreeSet};

use lj_dtos::DecisionDetail;
use printpdf::{
    Color, FontId, FontMetrics, Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage,
    PdfSaveOptions, Pt, Rgb, TextItem, TextMatrix,
};

use crate::titles::decision_jurisdiction;

/// Style logique d'un bloc de contenu — calque des `ParagraphStyle` ReportLab.
///
/// Les valeurs numériques (police, taille, interligne, couleurs, alignement)
/// sont reprises telles quelles de `pdf_export.py` pour que le rendu final soit
/// pixel-fidèle une fois branché sur un moteur PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfBlockStyle {
    /// Titre centré, Helvetica-Bold 18 / leading 22, `#1f2a44`.
    Title,
    /// Sous-titre centré (numéro de rôle), Helvetica-Bold 13 / leading 16.
    Subtitle,
    /// Ligne de méta, Helvetica 9.5 / leading 12, `#4a5568`.
    Meta,
    /// Paragraphe de corps, Helvetica 10.5 / leading 15, `#1f2937`.
    Body,
    /// Espaceur vertical (hauteur en points ≈ `Spacer`).
    Spacer,
}

/// Un bloc de la story PDF : style + contenu déjà échappé (XML ReportLab) prêt
/// à rendre. Pour [`PdfBlockStyle::Spacer`], `content` est vide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfBlock {
    pub style: PdfBlockStyle,
    pub content: String,
}

/// Échappe `&`, `<`, `>` façon `xml.sax.saxutils.escape` (ReportLab markup).
///
/// `escape` n'échappe par défaut **que** ces trois caractères (pas les
/// guillemets) — parité stricte avec le Python.
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

/// Construit la story (séquence de blocs) d'un PDF de décision — port fidèle de
/// `build_decision_pdf` (partie composition).
///
/// Ordre : titre (juridiction) → sous-titre (1er numéro de rôle ou `id`) →
/// lignes de méta (date de lecture, formation) → espaceur si méta → corps.
pub fn build_decision_story(detail: &DecisionDetail) -> Vec<PdfBlock> {
    let jurisdiction =
        decision_jurisdiction(jur_type_code(detail), detail.jurisdiction_name.as_deref());
    let docket = detail
        .docket_numbers
        .as_ref()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_else(|| detail.id.clone());

    let mut story: Vec<PdfBlock> = vec![
        PdfBlock {
            style: PdfBlockStyle::Title,
            content: xml_escape(&jurisdiction),
        },
        PdfBlock {
            style: PdfBlockStyle::Subtitle,
            content: xml_escape(&docket),
        },
    ];

    let mut meta_lines: Vec<String> = Vec::new();
    if let Some(date) = detail.date_lecture.as_deref().filter(|s| !s.is_empty()) {
        meta_lines.push(format!("Date de lecture : {date}"));
    }
    if let Some(seat) = detail.seat.as_deref().filter(|s| !s.is_empty()) {
        meta_lines.push(format!("Formation : {seat}"));
    }

    for line in &meta_lines {
        story.push(PdfBlock {
            style: PdfBlockStyle::Meta,
            content: xml_escape(line),
        });
    }

    if !meta_lines.is_empty() {
        story.push(PdfBlock {
            style: PdfBlockStyle::Spacer,
            content: String::new(),
        });
    }

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
        for (label, value) in provenance {
            story.push(PdfBlock {
                style: PdfBlockStyle::Meta,
                content: xml_escape(&format!("{label} : {value}")),
            });
        }
    }

    story
}

/// Génère le PDF d'une décision (octets) — port de `build_decision_pdf`.
///
/// La composition (titre, méta, corps, échappement) est portée et figée par
/// [`build_decision_story`] / les tests ; le rendu binaire passe par `printpdf`.
pub fn build_decision_pdf(detail: &DecisionDetail) -> Vec<u8> {
    let story = build_decision_story(detail);
    render_pdf(&story)
}

// --- Rendu PDF (printpdf, Liberation Sans embarquée) -----------------------

/// Géométrie A4 (points PostScript, 1 mm = 72/25.4 pt) — marges de
/// `SimpleDocTemplate` dans `pdf_export.py`. Les positions de texte sont
/// calculées en points (origine bas-gauche) puis posées telles quelles.
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

/// Attributs typographiques d'un style — calque 1-pour-1 des `ParagraphStyle`
/// ReportLab (taille, interligne, alignement, couleur RGB 0-1). `font` est le code
/// de graisse logique : `"F1"` = Regular, `"F2"` = Bold.
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
        PdfBlockStyle::Title => StyleAttrs {
            font: "F2", // gras
            size: 18.0,
            leading: 22.0,
            centered: true,
            color: (
                0x1f as f64 / 255.0,
                0x2a as f64 / 255.0,
                0x44 as f64 / 255.0,
            ),
            space_after: 8.0,
        },
        PdfBlockStyle::Subtitle => StyleAttrs {
            font: "F2",
            size: 13.0,
            leading: 16.0,
            centered: true,
            color: (
                0x1f as f64 / 255.0,
                0x2a as f64 / 255.0,
                0x44 as f64 / 255.0,
            ),
            space_after: 12.0,
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
        // Spacer : pas de glyphe, hauteur fixe (ReportLab `Spacer(1, 6)`).
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

        // Le contenu est en markup ReportLab (déjà XML-échappé + `<br/>`).
        // ReportLab le *rend* : on rétablit les glyphes visibles avant de poser.
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
/// dés-échappement XML (`&lt;`/`&gt;`/`&amp;`). C'est ce que ReportLab affiche.
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
// (`text_width`) restent valides pour le wrap/centrage. On l'embarque pour deux
// raisons : les polices base-14 builtin de printpdf 0.9 + lopdf 0.38 écrivent les
// octets **UTF-8 bruts** sous une police déclarée `/WinAnsiEncoding` (branche
// `SimpleEncoding(_)` non gérée) → tout accent finit en mojibake (`è` → `Ã¨`).
// Une police embarquée passe par l'encodage glyph-id (`Identity-H`), correct pour
// tout l'Unicode. printpdf en `default-features=false` n'a pas de parseur de
// police : on alimente ses maps glyphes (cmap + hmtx) via ttf-parser.

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
/// PDF `Tm`, **absolu**) par ligne — surtout pas `SetTextCursor` (`Td`, relatif :
/// il cumulerait les positions et jetterait le texte hors-page). `printpdf`
/// sérialise le document (catalog / pages / fonts embarquées / xref).
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

/// Code `jurisdiction_type` (forme DB « TA »/« CC »…) du DTO, pour
/// [`decision_jurisdiction`] qui attend le code brut.
fn jur_type_code(detail: &DecisionDetail) -> &'static str {
    use lj_dtos::JurisdictionType::*;
    match detail.jurisdiction_type {
        Ta => "TA",
        Caa => "CAA",
        Ce => "CE",
        Constit => "CONSTIT",
        Tc => "TC",
        Cc => "CC",
        Ca => "CA",
        Tj => "TJ",
        Tcom => "TCOM",
        Cedh => "CEDH",
        Cjue => "CJUE",
        Cnda => "CNDA",
        Cnil => "CNIL",
    }
}

// --- Métriques AFM base-14 (largeurs en 1/1000 d'em, indexées WinAnsi) ------
//
// Valeurs des fichiers AFM Adobe officiels (Helvetica.afm / Helvetica-Bold.afm),
// réindexées par code WinAnsiEncoding. Servent au word-wrap et au centrage.
// Les codes non assignés portent la largeur de l'espace (approximation neutre).

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
            id: "abc123".to_string(),
            jurisdiction_type: JurisdictionType::Cc,
            title: "ignored".to_string(),
            paragraphs: vec!["Premier <para> & suite".to_string(), "Second".to_string()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: Some("Cour de cassation".to_string()),
            date_lecture: Some("2026-05-29".to_string()),
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication: None,
            publication_codes: Vec::new(),
            date_audience: None,
            docket_numbers: Some(vec!["24-17.384".to_string()]),
            seat: Some("Chambre sociale".to_string()),
            chamber: None,
            formation: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: Vec::new(),
            commentaires: vec![],
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
    fn story_structure_matches_python() {
        let story = build_decision_story(&detail());
        // titre, sous-titre, 2 lignes méta, spacer, 2 paragraphes, spacer + pied
        // de provenance (permalien seul : fixture sans source ni ECLI).
        assert_eq!(story.len(), 9);
        assert_eq!(story[0].style, PdfBlockStyle::Title);
        assert_eq!(story[0].content, "Cour de cassation");
        assert_eq!(story[1].style, PdfBlockStyle::Subtitle);
        assert_eq!(story[1].content, "24-17.384");
        assert_eq!(story[2].style, PdfBlockStyle::Meta);
        assert_eq!(story[2].content, "Date de lecture : 2026-05-29");
        assert_eq!(story[3].content, "Formation : Chambre sociale");
        assert_eq!(story[4].style, PdfBlockStyle::Spacer);
        assert_eq!(story[5].style, PdfBlockStyle::Body);
        assert_eq!(story[5].content, "Premier &lt;para&gt; &amp; suite");
        assert_eq!(story[7].style, PdfBlockStyle::Spacer);
        assert_eq!(story[8].style, PdfBlockStyle::Meta);
        assert_eq!(
            story[8].content,
            "Permalien : https://librejustice.fr/decision/abc123"
        );
    }

    #[test]
    fn subtitle_falls_back_to_id_without_docket() {
        let mut d = detail();
        d.docket_numbers = None;
        let story = build_decision_story(&d);
        assert_eq!(story[1].content, "abc123");
    }

    #[test]
    fn no_meta_means_no_spacer() {
        let mut d = detail();
        d.date_lecture = None;
        d.seat = None;
        let story = build_decision_story(&d);
        // titre, sous-titre, puis directement le corps (pas de spacer).
        assert_eq!(story[2].style, PdfBlockStyle::Body);
    }

    #[test]
    fn pdf_is_well_formed() {
        let bytes = build_decision_pdf(&detail());
        // En-tête PDF (`%PDF-1.x`) + marqueur de fin produits par printpdf.
        assert!(bytes.starts_with(b"%PDF-"), "préfixe %PDF- manquant");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("%%EOF"), "marqueur %%EOF manquant");
        assert!(text.contains("startxref"), "table xref manquante");
        assert!(text.contains("/Catalog"), "catalog racine manquant");
        // Police Unicode embarquée (Type0/Identity-H + flux TrueType FontFile2),
        // pas une base-14 builtin — c'est ce qui rend les accents corrects.
        assert!(text.contains("/Type0"), "police Type0 (composite) attendue");
        assert!(text.contains("Identity-H"), "encodage Identity-H attendu");
        assert!(text.contains("FontFile2"), "flux police TrueType embarqué");
        assert!(text.contains("LiberationSans"), "BaseFont Liberation Sans");
    }
}
