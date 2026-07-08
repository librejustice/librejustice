//! Extracteur administratif (TA/CAA/CE) — port fidèle de `extract/opendata.py`.
//!
//! PUR : pas d'I/O. Les regexes Python à lookaround sont réécrites (cf. note de
//! module dans `extract.rs`). Les valeurs de sortie sont les chaînes des enums
//! `schema` (SCREAMING_CASE), retournées en `String` pour préserver le `None`.

use super::common::{clean_date_iso, clean_docket_numbers, extract_textual_audience_date};
use lj_core::decision::Decision;
use regex::Regex;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Regexes opendata — survivantes classées (audit ADR 0157) : petits spans
// positionnés (`DocScan::docket_context_windows`).
// ---------------------------------------------------------------------------

fn re_docket_context() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)sous\s+les?\s+(?:n[°ºo]s?|numéros?)\s+([^.;:\n]{1,160})").unwrap()
    })
}
fn re_docket_context_alt() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:requête|requêtes|pourvoi|pourvois|appel|appels)\b[^.;:\n]{0,100}?\bsous\s+les?\s+(?:n[°ºo]s?|numéros?)\s+([^.;:\n]{1,160})",
        )
        .unwrap()
    })
}
fn re_docket_citation_prefix() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:pourvu|pourvoi\w*)\s+en\s+cassation\s*$").unwrap())
}

include!("opendata_fields.rs");

#[cfg(test)]
mod tests;
