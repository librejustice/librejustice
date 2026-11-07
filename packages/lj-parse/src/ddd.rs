//! A decision of the Défenseur des droits, or of an authority it succeeded, from its catalogue
//! notice and the text of its PDF.

use std::collections::BTreeMap;

use lj_core::decision::Decision;
use lj_core::official_url::DDD_NOTICE_URL;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::extract_sections_xml;

const DEFENSEUR: &str = "Défenseur des droits";

/// The authorities the Défenseur des droits succeeded, by the start of the first author the
/// notice names, with the name a decision carries.
const PREDECESSORS: &[(&str, &str)] = &[
    (
        "Haute autorité de lutte contre les discriminations",
        "Haute autorité de lutte contre les discriminations et pour l'égalité",
    ),
    (
        "Commission nationale de déontologie de la sécurité",
        "Commission nationale de déontologie de la sécurité",
    ),
    ("Défenseur des Enfants", "Défenseur des enfants"),
    ("Médiateur de la République", "Médiateur de la République"),
];

pub(crate) fn source_uid_is_ddd(source_uid: &str) -> bool {
    source_uid.starts_with("ddd/")
}

/// What the decision of notice `notice_id` keeps of the catalogue: every field of the notice, as
/// its label reads, and the ids of its files.
pub fn ddd_source_fields(
    notice_id: u64,
    fields: &BTreeMap<String, String>,
    files: &[u64],
) -> Value {
    json!({
        "notice_id": notice_id,
        "notice_url": format!("{DDD_NOTICE_URL}{notice_id}"),
        "files": files,
        "fields": fields,
    })
}

/// The name a decision of the notice carries: its first author when it is an authority the
/// Défenseur des droits succeeded, the Défenseur des droits otherwise, its colleges included.
pub(crate) fn ddd_authority(authors: &str) -> &'static str {
    let first = authors.trim_start();
    PREDECESSORS
        .iter()
        .find(|(start, _)| first.starts_with(start))
        .map_or(DEFENSEUR, |(_, name)| name)
}

pub(crate) fn from_source_fields_ddd(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |label: &str| {
        source_fields
            .get("fields")
            .and_then(|f| f.get(label))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let numero = field("Numéro de décision ou d'affaire").map(str::to_string);
    Decision {
        source_uid: source_uid.to_string(),
        member_name: numero.clone().unwrap_or_default(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some(ddd_authority(field("Auteurs").unwrap_or_default()).to_string()),
        jurisdiction_type: Some("DDD".to_string()),
        jurisdiction_location: None,
        numero_dossiers: numero.clone().map(|n| vec![n]),
        numero_dossier: numero,
        numero_role: None,
        date_lecture: field("Année de publication").and_then(iso_date),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
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

/// `16/04/2026` as `2026-04-16`; a year alone is no date.
fn iso_date(value: &str) -> Option<String> {
    static DATE: OnceLock<Regex> = OnceLock::new();
    let c = DATE
        .get_or_init(|| Regex::new(r"^(\d{2})/(\d{2})/(\d{4})$").unwrap())
        .captures(value)?;
    Some(format!("{}-{}-{}", &c[3], &c[2], &c[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;

    #[test]
    fn a_notice_reads_as_a_decision_of_its_authority() {
        let fields: BTreeMap<String, String> = [
            ("Titre", "Délibération n°2010-12 du 18 janvier 2010"),
            ("Auteurs", "Haute autorité de lutte contre les discriminations et pour l'égalité (2005-2011) , Auteur"),
            ("Type de document", "Décisions"),
            ("Année de publication", "18/01/2010"),
            ("Numéro de décision ou d'affaire", "2010-12"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let text = "Vu la loi n° 2004-1486 du 30 décembre 2004 ;\nLe Collège recommande au maire de réexaminer la demande.";
        let d = Decision::from_source_fields(text, &ddd_source_fields(42, &fields, &[7]), "ddd/42");
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Haute autorité de lutte contre les discriminations et pour l'égalité")
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("DDD"));
        assert_eq!(d.numero_dossier.as_deref(), Some("2010-12"));
        assert_eq!(d.date_lecture.as_deref(), Some("2010-01-18"));
    }

    #[test]
    fn a_college_of_the_defenseur_is_the_defenseur() {
        assert_eq!(
            ddd_authority("Justice et libertés , Auteur"),
            "Défenseur des droits"
        );
        assert_eq!(
            ddd_authority("Défenseur des droits , Auteur ; Défense des enfants , Auteur"),
            "Défenseur des droits"
        );
        assert_eq!(
            ddd_authority("Médiateur de la République (1973-2011) , Auteur"),
            "Médiateur de la République"
        );
        assert_eq!(iso_date("2026"), None);
    }
}
