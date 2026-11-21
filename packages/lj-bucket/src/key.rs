//! Where each fonds keeps its files in the bucket.

pub fn ddd(explnum: u64) -> String {
    format!("ddd/{explnum}.pdf")
}

/// A CNDA decision's file, now and then a DOCX despite the extension.
pub fn cnda(numero: &str) -> String {
    format!("cnda/{numero}.pdf")
}

/// A recueil as the prefecture served it, one object per version of the file at its URL.
pub fn raa(id: i64, checksum: i64) -> String {
    format!("raa/{:016x}/{:016x}.pdf", id as u64, checksum as u64)
}

/// A Conseil du contentieux des étrangers arrêt's PDF, by its number.
pub fn cce(number: u32) -> String {
    format!("cce/{number}.pdf")
}

/// A ministry bulletin's text, its own document as the site served it.
pub fn bulletin(site: &str, native_id: &str) -> String {
    format!("bo/{site}/{native_id}.pdf")
}

/// A bulletin issue its texts were cut from, by the xxh3-64 of its URL.
pub fn bulletin_issue(site: &str, url_hash: u64) -> String {
    format!("bo/{site}/issues/{url_hash:016x}.pdf")
}

/// An archived copy of what a JuriCAF page links: `url`, `html`, `pdf`, or the markdown `ocr.md`
/// of a scan read before its OCR answer was kept.
pub fn juricaf(slug: &str, ext: &str) -> String {
    format!("juricaf/{slug}.{ext}")
}

/// A file an administrative court's site links, by the id the site gives it and its format.
pub fn juradm_site(court: &str, id: &str, format: &str) -> String {
    format!("juradm/{court}/{id}.{format}")
}

/// A file a court's site links, by the site's source and the id it gives the decision.
pub fn court_site(source: &str, id: &str, ext: &str) -> String {
    format!("court-sites/{source}/{id}.{ext}")
}

/// A deed (`acte`) or annual accounts (`bilan`) a company filed at the INPI.
pub fn inpi(siren: &str, kind: &str, id: &str) -> String {
    format!("inpi/{siren}/{kind}/{id}.pdf")
}

/// Mistral's OCR answer for the file at `file`, kept beside it.
pub fn ocr(file: &str) -> String {
    format!("{file}.ocr.json.zst")
}

/// Mistral's OCR answer for page `page` of the file at `file`, counted from 1.
pub fn ocr_page(file: &str, page: u32) -> String {
    format!("{file}.ocr/{page}.json.zst")
}

/// Mistral's OCR answer for a circulaire's scan, whose PDF stays in DILA's stock.
pub fn circulaire_ocr(id: &str) -> String {
    format!("circulaires/{id}.ocr.json.zst")
}

/// Mistral's OCR answer for a ConsiliaWeb opinion's scan, whose PDF stays on the site.
pub fn consiliaweb_ocr(number: &str) -> String {
    format!("consiliaweb/{number}.ocr.json.zst")
}

/// Mistral's OCR answer for an entscheidsuche document's scan, whose PDF stays at the index.
pub fn entscheidsuche_ocr(id: &str) -> String {
    format!("entscheidsuche/{id}.ocr.json.zst")
}

/// An AMF Commission des sanctions decision's file, the version its page serves last.
pub fn amf(number: &str) -> String {
    format!("amf/{number}.pdf")
}

/// The text of conclusions in one language, zstd-compressed: `uid` is the CELEX at
/// EUR-Lex, `<case number>@<reading date>` at ArianeWeb.
pub fn conclusions_text(source: &str, uid: &str, lang: &str) -> String {
    format!("conclusions/{source}/{uid}/{lang}.txt.zst")
}

/// Conclusions as their source served them, a PDF at ArianeWeb.
pub fn conclusions_pdf(source: &str, uid: &str) -> String {
    format!("conclusions/{source}/{uid}.pdf")
}
