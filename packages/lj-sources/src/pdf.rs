//! Extraction texte PDF born-digital au bord I/O.
//!
//! Le décodage de format vit dans `lj-sources` : ce module rend le texte plat
//! pour que le parser pur de `lj-core` reçoive du texte, pas des octets PDF.

use crate::error::{Result, SourceError};
use std::io::Write;
use std::process::{Command, Stdio};

/// A scanned PDF succeeds and yields an all but empty text: falling back to OCR
/// is the caller's call.
pub fn pdftotext_extract(bytes: &[u8]) -> Result<String> {
    poppler("pdftotext", &["-q", "-enc", "UTF-8", "-", "-"], bytes)
}

/// The columns kept side by side, for a table of contents whose page numbers
/// sit at the end of each line.
pub fn pdftotext_layout_extract(bytes: &[u8]) -> Result<String> {
    poppler(
        "pdftotext",
        &["-q", "-layout", "-enc", "UTF-8", "-", "-"],
        bytes,
    )
}

/// The text of every page, through `lj-pdf` rather than poppler. A page whose
/// content the reader does not support is left out.
pub fn lj_pdf_extract(bytes: &[u8]) -> Result<String> {
    let doc = lj_pdf::PdfDocument::open(bytes)
        .map_err(|e| SourceError::Invalid(format!("lj-pdf: {e}")))?;
    Ok((0..doc.page_count())
        .filter_map(|i| doc.extract_text(i).ok())
        .map(|p| p.text)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Renders the PDF to HTML, for the one thing flat text loses: the bold that
/// tells a form's label from its value. `-stdout` demands an output name all
/// the same, which gives rise to no file.
pub fn pdftohtml_extract(bytes: &[u8]) -> Result<String> {
    poppler(
        "pdftohtml",
        &["-q", "-i", "-noframes", "-stdout", "-", "sortie"],
        bytes,
    )
}

/// Every run of text with its page, place and font size, as `pdftohtml -xml` renders them.
pub fn pdftohtml_xml_extract(bytes: &[u8]) -> Result<String> {
    poppler(
        "pdftohtml",
        &["-q", "-xml", "-i", "-stdout", "-", "sortie"],
        bytes,
    )
}

/// A PDF of the given pages only, numbered from 1. `qpdf` keeps each page's own
/// resources, where `pdfseparate` gave each page the resources of the whole file.
pub fn pages_extract(bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>> {
    if pages.is_empty() {
        return Err(SourceError::Invalid("pages_extract: no page".into()));
    }
    in_scratch(|dir| qpdf_pages(dir, bytes, pages))
}

pub fn page_count(bytes: &[u8]) -> Result<u32> {
    in_scratch(|dir| {
        let input = dir.join("in.pdf");
        std::fs::write(&input, bytes)?;
        let out = Command::new("qpdf")
            .arg("--show-npages")
            .arg(&input)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| std::io::Error::new(e.kind(), format!("qpdf: {e}")))?;
        let shown = String::from_utf8_lossy(&out.stdout);
        shown
            .trim()
            .parse()
            .map_err(|_| SourceError::Invalid(format!("qpdf --show-npages: {:?}", shown.trim())))
    })
}

fn in_scratch<T>(f: impl FnOnce(&std::path::Path) -> Result<T>) -> Result<T> {
    static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lj-pages-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)?;
    let out = f(&dir);
    std::fs::remove_dir_all(&dir)?;
    out
}

fn qpdf_pages(dir: &std::path::Path, bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>> {
    let (input, output) = (dir.join("in.pdf"), dir.join("out.pdf"));
    std::fs::write(&input, bytes)?;
    let range = pages
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let status = Command::new("qpdf")
        .arg("--empty")
        .arg("--pages")
        .arg(&input)
        .arg(&range)
        .arg("--")
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| SourceError::Invalid(format!("qpdf: {e}")))?;
    // qpdf exits 3 when it wrote the file with warnings.
    if !matches!(status.code(), Some(0 | 3)) {
        return Err(SourceError::Invalid(format!(
            "qpdf code {:?}",
            status.code()
        )));
    }
    Ok(std::fs::read(output)?)
}

/// PDF on stdin, output read on stdout. A PDF weighing several MB deadlocks
/// against the stdout pipe unless stdin is written from its own thread.
fn poppler(outil: &str, args: &[&str], bytes: &[u8]) -> Result<String> {
    let mut child = Command::new(outil)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            SourceError::Invalid(format!("{outil} introuvable (poppler-utils requis): {e}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .expect("stdin piped above is always present");
    let payload = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&payload));
    let output = child
        .wait_with_output()
        .map_err(|e| SourceError::Invalid(format!("{outil}: {e}")))?;
    writer
        .join()
        .map_err(|_| SourceError::Invalid(format!("{outil}: thread stdin paniqué")))?
        .map_err(|e| SourceError::Invalid(format!("{outil} stdin: {e}")))?;
    if !output.status.success() {
        return Err(SourceError::Invalid(format!(
            "{outil} code {:?}",
            output.status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
