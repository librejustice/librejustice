//! Extraction texte DOCX born-digital au bord I/O (ADR 0110, frontière #1/#12).
//!
//! Certaines décisions CNDA sont publiées en **Word (.docx)** et non en PDF (le
//! lien « Voir la décision » sert alors un OOXML, pas un PDF). Mistral OCR ne
//! traite que PDF/images ⇒ rejette le DOCX (HTTP 400). Mais un DOCX est
//! born-digital : son texte vit dans `word/document.xml` (zip OOXML), extractible
//! **sans OCR**, déterministe. Ce module rend le texte plat (un paragraphe `<w:p>`
//! par ligne) pour que le parser pur de `lj-core` reçoive du texte, pas des octets.

use crate::error::{Result, SourceError};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Extrait le texte d'un DOCX (OOXML). Le corps est `word/document.xml` ; le texte
/// vit dans les runs `<w:t>`, les paragraphes sont délimités par `<w:p>`. On rend
/// un paragraphe logique par ligne (aligné sur le rendu OCR, `clean_texte` en
/// aval). Erreur franche [`SourceError`] si le zip est invalide, sans
/// `word/document.xml`, ou sans texte extractible.
pub fn extract_docx_text(bytes: &[u8]) -> Result<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| SourceError::Invalid(format!("docx sans word/document.xml: {e}")))?
        .read_to_string(&mut xml)?;

    let mut reader = Reader::from_str(&xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"t" => in_text = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"t" => in_text = false,
            Ok(Event::Text(t)) if in_text => {
                let raw = t
                    .decode()
                    .map_err(|e| SourceError::Invalid(format!("docx texte non-UTF8: {e}")))?;
                out.push_str(&raw);
            }
            // quick-xml 0.40 émet les entités `&amp;`/`&#233;` comme événements
            // séparés (nom seul, sans `&;`) ; on les résout via `escape::unescape`
            // (prédéfinies XML + numériques). Hors `<w:t>` ⇒ ignorées.
            Ok(Event::GeneralRef(r)) if in_text => {
                let name = std::str::from_utf8(r.as_ref()).unwrap_or("");
                if let Ok(u) = quick_xml::escape::unescape(&format!("&{name};")) {
                    out.push_str(&u);
                }
            }
            // Fin de paragraphe Word → saut de ligne (un paragraphe logique/ligne).
            Ok(Event::End(e)) if e.local_name().as_ref() == b"p" => out.push('\n'),
            Ok(Event::Eof) => break,
            Err(e) => return Err(SourceError::Invalid(format!("docx xml illisible: {e}"))),
            _ => {}
        }
    }

    if out.trim().is_empty() {
        return Err(SourceError::Invalid(
            "docx sans texte extractible (word/document.xml vide)".to_string(),
        ));
    }
    Ok(out)
}

/// Vrai si les octets ressemblent à un conteneur OOXML/zip (signature `PK\x03\x04`).
/// Sert au pipeline à router PDF (OCR) vs DOCX (extraction directe) sur le payload
/// « PDF » réel — certaines fiches CNDA servent un Word sous le lien décision.
pub fn is_zip_container(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// DOCX minimal valide (zip avec `word/document.xml` à deux paragraphes).
    fn tiny_docx(paras: &[&str]) -> Vec<u8> {
        let body: String = paras
            .iter()
            .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
            .collect();
        let xml = format!(
            r#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body>{body}</w:body></w:document>"#
        );
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file("word/document.xml", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extracts_paragraphs_one_per_line() {
        let docx = tiny_docx(&["Vu la procédure suivante :", "DECIDE : Article 1er."]);
        assert!(is_zip_container(&docx));
        let text = extract_docx_text(&docx).expect("docx extractible");
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines,
            vec!["Vu la procédure suivante :", "DECIDE : Article 1er."]
        );
    }

    #[test]
    fn decodes_xml_entities() {
        let docx = tiny_docx(&["Médecine &amp; Hygiène &lt;2002&gt;"]);
        let text = extract_docx_text(&docx).expect("docx");
        assert!(text.contains("Médecine & Hygiène <2002>"), "{text:?}");
    }

    #[test]
    fn franc_error_on_non_zip() {
        assert!(!is_zip_container(b"%PDF-1.7"));
        assert!(extract_docx_text(b"pas un zip du tout").is_err());
    }
}
