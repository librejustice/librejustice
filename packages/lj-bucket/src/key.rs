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

/// A deed (`acte`) or annual accounts (`bilan`) a company filed at the INPI.
pub fn inpi(siren: &str, kind: &str, id: &str) -> String {
    format!("inpi/{siren}/{kind}/{id}.pdf")
}
