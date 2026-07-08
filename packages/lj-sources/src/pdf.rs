//! Extraction texte PDF born-digital au bord I/O (ADR 0095, frontière #1/#12).
//!
//! Les conclusions du rapporteur public (CRP) ArianeWeb sont des PDF
//! born-digital (Word → Aspose, polices embarquées, aucune image → pas d'OCR).
//! Le décodage de format vit dans `lj-sources` : ce module rend le texte plat
//! pour que le parser pur de `lj-core` reçoive du texte, pas des octets PDF.
//!
//! Crate PUR Rust [`pdf_extract`] (pas de dép native pdfium/poppler, non
//! garantie en CI/env). Si la qualité word-spacing/apostrophes s'avère
//! insuffisante sur PDF réels, swap pdfium-render/poppler (follow-up ADR 0095).

use crate::error::{Result, SourceError};
use std::io::Write;
use std::process::{Command, Stdio};

/// Extrait le texte d'un PDF born-digital. Erreur franche [`SourceError`] si le
/// PDF est illisible (corrompu, chiffré, structure invalide).
pub fn extract_pdf_text(bytes: &[u8]) -> Result<String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| SourceError::Invalid(format!("extraction PDF échouée: {e}")))
}

/// Extrait le texte d'un PDF via `pdftotext` (poppler, sous-process), ordre de
/// lecture préservé, encodage UTF-8 (ADR 0124). Choisi pour la CNDA car son
/// word-spacing est fidèle (≠ `pdf_extract`, qui coupe intra-mot) : le recollage
/// par règles aval (`lj_core::parsing::reflow_cnda_pdf_text`) n'a plus qu'à
/// rejoindre les retours de ligne visuels.
///
/// PDF passé sur stdin, texte lu sur stdout (pas de fichier temporaire). Le PDF
/// peut peser plusieurs Mo : on écrit stdin depuis un thread dédié pour ne pas
/// inter-bloquer le pipe stdout. Erreur franche si `pdftotext` est absent
/// (`poppler-utils` requis) ou sort en échec (PDF chiffré/corrompu). Un PDF
/// **scanné** (sans couche texte) réussit mais rend un texte quasi vide — c'est
/// à l'appelant de basculer en repli OCR (cf. seuil pipeline).
pub fn pdftotext_extract(bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("pdftotext")
        .args(["-q", "-enc", "UTF-8", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            SourceError::Invalid(format!("pdftotext introuvable (poppler-utils requis): {e}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .expect("stdin piped above is always present");
    let payload = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&payload));
    let output = child
        .wait_with_output()
        .map_err(|e| SourceError::Invalid(format!("pdftotext: {e}")))?;
    writer
        .join()
        .map_err(|_| SourceError::Invalid("pdftotext: thread stdin paniqué".into()))?
        .map_err(|e| SourceError::Invalid(format!("pdftotext stdin: {e}")))?;
    if !output.status.success() {
        return Err(SourceError::Invalid(format!(
            "pdftotext code {:?}",
            output.status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PDF born-digital valide (police base-14 Helvetica encodée par printpdf) :
    /// un PDF cousu à la main n'embarque pas de mapping glyphe→caractère, donc
    /// l'extracteur rend du vide. On en génère un vrai à la volée.
    fn born_digital_pdf(text: &str) -> Vec<u8> {
        use printpdf::{
            BuiltinFont, Mm, Op, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, Point, Pt,
            TextItem,
        };
        let ops = vec![
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point {
                    x: Pt(20.0),
                    y: Pt(700.0),
                },
            },
            Op::SetFont {
                font: PdfFontHandle::Builtin(BuiltinFont::Helvetica),
                size: Pt(12.0),
            },
            Op::ShowText {
                items: vec![TextItem::Text(text.to_string())],
            },
            Op::EndTextSection,
        ];
        let page = PdfPage::new(Mm(210.0), Mm(297.0), ops);
        PdfDocument::new("fixture")
            .with_pages(vec![page])
            .save(&PdfSaveOptions::default(), &mut Vec::new())
    }

    #[test]
    fn extracts_text_from_born_digital_pdf() {
        let pdf = born_digital_pdf("Conclusions du rapporteur public");
        let text = extract_pdf_text(&pdf).expect("PDF born-digital extractible");
        assert!(
            text.contains("Conclusions du rapporteur public"),
            "texte extrait: {text:?}"
        );
    }

    /// Déterminisme run-à-run (ADR 0095/0096) : `pdf-extract` est pur Rust, sans
    /// dép native ni source d'entropie ; extraire deux fois le **même** PDF
    /// born-digital doit rendre un texte **identique**. C'est ce qui garantit
    /// qu'un re-extract futur (re-fetch du même PDF) reproduit le même
    /// `texte_integral_clean`, donc les mêmes chunks (pas de re-embed).
    #[test]
    fn extraction_is_deterministic_run_to_run() {
        let pdf = born_digital_pdf("Cour nationale du droit d'asile N 26006334 DECIDE");
        let a = extract_pdf_text(&pdf).expect("extraction 1");
        let b = extract_pdf_text(&pdf).expect("extraction 2");
        assert_eq!(a, b, "extraction PDF non déterministe : {a:?} != {b:?}");
    }

    #[test]
    fn invalid_pdf_is_franc_error() {
        let err = extract_pdf_text(b"pas un pdf du tout").unwrap_err();
        assert!(matches!(err, SourceError::Invalid(_)));
    }
}
