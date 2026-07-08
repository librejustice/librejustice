//! Génération DOCX à partir d'un [`DecisionDetail`] — port de `docx_export.py`.
//!
//! Le Python utilise `python-docx` (`Document` + `add_heading` /
//! `add_paragraph`). Côté Rust on s'appuie sur la crate `docx-rs` : la
//! **structure du document** (heading niveau 1 = juridiction, niveau 2 =
//! numéro, méta en paragraphes 10 pt, corps) est figée par
//! [`build_decision_docx_blocks`], puis [`render_docx`] la sérialise en paquet
//! OPC. La parité est une parité de **contenu/structure**, pas d'octets avec
//! python-docx.

use docx_rs::{Docx, Paragraph, Run};
use lj_dtos::DecisionDetail;

use crate::titles::decision_jurisdiction;

/// Bloc logique d'un document DOCX — calque des appels `python-docx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocxBlock {
    /// `add_heading(text, level=1)` — juridiction.
    Heading1(String),
    /// `add_heading(text, level=2)` — numéro de rôle (ou `id`).
    Heading2(String),
    /// `add_paragraph(text)` avec `runs[0].font.size = Pt(10)` — ligne de méta.
    MetaParagraph(String),
    /// `add_paragraph("")` — paragraphe vide séparateur après la méta.
    EmptyParagraph,
    /// `add_paragraph(text)` — paragraphe de corps (style par défaut).
    BodyParagraph(String),
}

/// Construit la séquence de blocs d'un DOCX de décision — port fidèle de
/// `build_decision_docx` (partie structure).
///
/// Ordre : heading 1 (juridiction) → heading 2 (1er numéro de rôle ou `id`) →
/// lignes de méta (10 pt) + paragraphe vide si méta → corps. Contrairement au
/// PDF, **aucun échappement** : `python-docx` écrit le texte tel quel.
pub fn build_decision_docx_blocks(detail: &DecisionDetail) -> Vec<DocxBlock> {
    let juridiction =
        decision_jurisdiction(jur_type_code(detail), detail.jurisdiction_name.as_deref());
    let docket = detail
        .docket_numbers
        .as_ref()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_else(|| detail.id.clone());

    let mut blocks: Vec<DocxBlock> = vec![
        DocxBlock::Heading1(juridiction),
        DocxBlock::Heading2(docket),
    ];

    let mut meta_lines: Vec<String> = Vec::new();
    if let Some(date) = detail.date_lecture.as_deref().filter(|s| !s.is_empty()) {
        meta_lines.push(format!("Date de lecture : {date}"));
    }
    if let Some(formation) = detail
        .formation_or_chamber
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        meta_lines.push(format!("Formation : {formation}"));
    }

    if !meta_lines.is_empty() {
        for line in meta_lines {
            blocks.push(DocxBlock::MetaParagraph(line));
        }
        blocks.push(DocxBlock::EmptyParagraph);
    }

    for text in &detail.paragraphs {
        blocks.push(DocxBlock::BodyParagraph(text.clone()));
    }

    // Pied de provenance/audit (source, ECLI, permalien) — même contenu que le
    // bloc « Source » du web et l'export PDF (`lj_dtos::provenance_rows`).
    let provenance = lj_dtos::provenance_rows(detail);
    if !provenance.is_empty() {
        blocks.push(DocxBlock::EmptyParagraph);
        for (label, value) in provenance {
            blocks.push(DocxBlock::MetaParagraph(format!("{label} : {value}")));
        }
    }

    blocks
}

/// Génère le DOCX d'une décision (octets) — port de `build_decision_docx`.
///
/// La structure (headings, méta, corps) est portée et figée par
/// [`build_decision_docx_blocks`] ; [`render_docx`] la sérialise via `docx-rs`.
pub fn build_decision_docx(detail: &DecisionDetail) -> Vec<u8> {
    let blocks = build_decision_docx_blocks(detail);
    render_docx(&blocks)
}

/// Sérialise les blocs en paquet OPC `.docx` via `docx-rs`.
///
/// Mapping fidèle à python-docx : Heading1/Heading2 via `pStyle` built-in, méta
/// en run 10 pt (`size(20)` = 20 demi-points), paragraphe vide séparateur, corps
/// en style par défaut. `docx-rs` échappe le texte à la sérialisation, comme
/// python-docx.
fn render_docx(blocks: &[DocxBlock]) -> Vec<u8> {
    let mut docx = Docx::new();
    for block in blocks {
        let paragraph = match block {
            DocxBlock::Heading1(t) => Paragraph::new()
                .add_run(Run::new().add_text(t))
                .style("Heading1"),
            DocxBlock::Heading2(t) => Paragraph::new()
                .add_run(Run::new().add_text(t))
                .style("Heading2"),
            // `runs[0].font.size = Pt(10)` → size en demi-points = 20.
            DocxBlock::MetaParagraph(t) => {
                Paragraph::new().add_run(Run::new().add_text(t).size(20))
            }
            DocxBlock::EmptyParagraph => Paragraph::new(),
            DocxBlock::BodyParagraph(t) => Paragraph::new().add_run(Run::new().add_text(t)),
        };
        docx = docx.add_paragraph(paragraph);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    docx.build()
        .pack(&mut buf)
        .expect("docx-rs pack into in-memory buffer");
    buf.into_inner()
}

/// Code `juridiction_type` (forme DB « TA »/« CC »…) du DTO, pour
/// [`decision_jurisdiction`].
fn jur_type_code(detail: &DecisionDetail) -> &'static str {
    use lj_dtos::JuridictionType::*;
    match detail.juridiction_type {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::JuridictionType;

    fn detail() -> DecisionDetail {
        DecisionDetail {
            id: "abc123".to_string(),
            juridiction_type: JuridictionType::Ca,
            title: "ignored".to_string(),
            paragraphs: vec!["Corps & <texte>".to_string()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_name: Some("Cour d'appel de Paris".to_string()),
            date_lecture: Some("2024-02-13".to_string()),
            solution: None,
            voie: None,
            office: None,
            legal_domain: None,
            publication_codes: Vec::new(),
            date_audience: None,
            docket_numbers: Some(vec!["RG 21/12345".to_string()]),
            formation_or_chamber: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            chronology: Vec::new(),
        }
    }

    /// Extrait le texte UTF-8 d'une partie OPC du paquet `.docx` (ZIP).
    fn read_part(bytes: &[u8], name: &str) -> String {
        use std::io::Read as _;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
            .expect("paquet .docx = ZIP valide");
        let mut part = archive.by_name(name).expect("partie OPC présente");
        let mut out = String::new();
        part.read_to_string(&mut out).expect("partie OPC en UTF-8");
        out
    }

    #[test]
    fn blocks_structure_matches_python() {
        let blocks = build_decision_docx_blocks(&detail());
        // heading1, heading2, 1 méta (date seule), empty, 1 corps, puis pied de
        // provenance : empty + permalien (fixture sans source ni ECLI).
        assert_eq!(blocks.len(), 7);
        assert_eq!(
            blocks[0],
            DocxBlock::Heading1("Cour d'appel de Paris".to_string())
        );
        assert_eq!(blocks[1], DocxBlock::Heading2("RG 21/12345".to_string()));
        assert_eq!(
            blocks[2],
            DocxBlock::MetaParagraph("Date de lecture : 2024-02-13".to_string())
        );
        assert_eq!(blocks[3], DocxBlock::EmptyParagraph);
        // Corps : pas d'échappement (python-docx écrit tel quel).
        assert_eq!(
            blocks[4],
            DocxBlock::BodyParagraph("Corps & <texte>".to_string())
        );
        assert_eq!(blocks[5], DocxBlock::EmptyParagraph);
        assert_eq!(
            blocks[6],
            DocxBlock::MetaParagraph(
                "Permalien : https://librejustice.fr/decision/abc123".to_string()
            )
        );
    }

    #[test]
    fn no_meta_means_no_empty_paragraph() {
        let mut d = detail();
        d.date_lecture = None;
        d.formation_or_chamber = None;
        let blocks = build_decision_docx_blocks(&d);
        // heading1, heading2, corps (pas de séparateur méta), puis pied de
        // provenance : empty + permalien.
        assert_eq!(blocks.len(), 5);
        assert!(matches!(blocks[2], DocxBlock::BodyParagraph(_)));
        assert_eq!(blocks[3], DocxBlock::EmptyParagraph);
        assert!(matches!(blocks[4], DocxBlock::MetaParagraph(_)));
    }

    #[test]
    fn heading2_falls_back_to_id() {
        let mut d = detail();
        d.docket_numbers = None;
        let blocks = build_decision_docx_blocks(&d);
        assert_eq!(blocks[1], DocxBlock::Heading2("abc123".to_string()));
    }

    #[test]
    fn docx_is_a_zip_with_expected_parts() {
        let bytes = build_decision_docx(&detail());
        // Signature ZIP local file header.
        assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
        // Fin de central directory présente.
        assert!(bytes.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
        // Les parties OPC attendues sont dans l'archive.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
            .expect("paquet .docx = ZIP valide");
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "[Content_Types].xml"));
        assert!(names.iter().any(|n| n == "word/document.xml"));
        assert!(names.iter().any(|n| n == "_rels/.rels"));
    }

    #[test]
    fn document_xml_carries_structure_and_styles() {
        let bytes = build_decision_docx(&detail());
        let xml = read_part(&bytes, "word/document.xml");
        // Headings via pStyle built-in Heading1/Heading2.
        assert!(xml.contains("<w:pStyle w:val=\"Heading1\" />"));
        // docx-rs échappe l'apostrophe en `&apos;` (texte rendu identique).
        assert!(xml.contains("Cour d&apos;appel de Paris"));
        assert!(xml.contains("<w:pStyle w:val=\"Heading2\" />"));
        assert!(xml.contains("RG 21/12345"));
        // Méta en 10 pt (sz = demi-points = 20).
        assert!(xml.contains("<w:sz w:val=\"20\" />"));
        assert!(xml.contains("Date de lecture : 2024-02-13"));
        // Corps : XML échappé par docx-rs (`&` et `<`/`>`).
        assert!(xml.contains("Corps &amp; &lt;texte&gt;"));
    }
}
