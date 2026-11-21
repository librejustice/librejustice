//! A consultative opinion of the Conseil d'État, from its ConsiliaWeb census hit and the text of its file.

use std::sync::OnceLock;

use jiff::civil::Date;
use lj_core::decision::Decision;
use regex::Regex;
use serde_json::{json, Value};

use crate::extract_sections_xml;

pub(crate) fn source_uid_is_consiliaweb(source_uid: &str) -> bool {
    source_uid.starts_with("consiliaweb/")
}

/// The census hit whole, and the address its file is served at.
pub fn consiliaweb_source_fields(hit: Value, url: &str) -> Value {
    json!({ "url_site": url, "hit": hit })
}

/// The opinion's number, `SourceCsv1`.
pub fn consiliaweb_number(hit: &Value) -> Option<&str> {
    hit.get("SourceCsv1")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
}

/// The day of the opinion: its file name's (`408492_20240701.doc` is dated 2027 by its hit).
pub fn consiliaweb_date(hit: &Value) -> Option<Date> {
    static FILE: OnceLock<Regex> = OnceLock::new();
    let named = hit
        .get("FileName")
        .and_then(Value::as_str)
        .and_then(|f| {
            FILE.get_or_init(|| Regex::new(r"^\d+_(\d{4})(\d{2})(\d{2})").unwrap())
                .captures(f)
        })
        .and_then(|c| Date::new(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?).ok());
    named.or_else(|| {
        hit.get("SourceDateTime1")
            .and_then(Value::as_str)
            .and_then(|d| d.get(..10)?.parse().ok())
    })
}

pub(crate) fn from_source_fields_consiliaweb(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let hit = &source_fields["hit"];
    let number = consiliaweb_number(hit).map(str::to_string);
    Decision {
        source_uid: source_uid.to_string(),
        member_name: number.clone().unwrap_or_default(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Conseil d'État".to_string()),
        jurisdiction_type: Some("CE".to_string()),
        jurisdiction_location: None,
        numero_dossiers: number.clone().map(|n| vec![n]),
        numero_dossier: number,
        numero_role: None,
        date_lecture: consiliaweb_date(hit).map(|d| d.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: Some("Formation consultative".to_string()),
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections: extract_sections_xml(full_text),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;

    #[test]
    fn an_opinion_is_a_ce_decision_of_its_consultative_formation_dated_by_its_file_name() {
        let hit = json!({
            "Id": "/Consilia_Web/CW/|105267",
            "FileName": "408492_20240701.doc",
            "SourceCsv1": "408492",
            "SourceStr7": "Section des travaux publics",
            "SourceStr45": "section des travaux publics",
            "SourceDateTime1": "2027-07-01 02:00:00",
        });
        let fields = consiliaweb_source_fields(hit, "u");
        let d = Decision::from_source_fields(
            "Le Conseil d'Etat a été saisi.",
            &fields,
            "consiliaweb/408492",
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CE"));
        assert_eq!(d.date_lecture.as_deref(), Some("2024-07-01"));
        assert_eq!(d.numero_dossiers, Some(vec!["408492".to_string()]));
        assert_eq!(d.formation.as_deref(), Some("Formation consultative"));
    }
}
