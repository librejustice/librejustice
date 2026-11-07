//! A decision of the Autorité de la concurrence, or of the Conseil de la concurrence, from its data.gouv record.

use lj_core::decision::Decision;
use serde_json::{Map, Value};

use crate::extract_sections_xml;

const TEXT_FIELD: &str = "texte_complet_decision";
/// The Autorité de la concurrence took over from the Conseil de la concurrence on this day.
const AUTORITE_FROM: &str = "2009-03-02";

pub(crate) fn source_uid_is_adlc(source_uid: &str) -> bool {
    source_uid.starts_with("adlc/")
}

/// A contentious decision, an interim measure or a merger decision, never an opinion.
pub fn adlc_is_decision(record: &Value) -> bool {
    let is = |t: &Value| matches!(t.as_str(), Some("Décision" | "DCC"));
    match &record["type_decision"] {
        Value::Array(types) => types.iter().any(is),
        t => is(t),
    }
}

pub fn adlc_number(record: &Value) -> Option<&str> {
    record["id_decision"].as_str().map(str::trim)
}

pub fn adlc_text(record: &Value) -> Option<&str> {
    record[TEXT_FIELD].as_str()
}

/// Every field of the record but its text.
pub fn adlc_source_fields(record: &Value) -> Value {
    let mut fields: Map<String, Value> = record.as_object().cloned().unwrap_or_default();
    fields.remove(TEXT_FIELD);
    Value::Object(fields)
}

pub(crate) fn from_source_fields_adlc(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let numero = adlc_number(source_fields).map(str::to_string);
    let date = source_fields["date_decision_datetime"]
        .as_str()
        .map(str::to_string);
    let authority = match date.as_deref() {
        Some(d) if d < AUTORITE_FROM => "Conseil de la concurrence",
        _ => "Autorité de la concurrence",
    };
    Decision {
        source_uid: source_uid.to_string(),
        member_name: numero.clone().unwrap_or_default(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some(authority.to_string()),
        jurisdiction_type: Some("ADLC".to_string()),
        jurisdiction_location: None,
        numero_dossiers: numero.clone().map(|n| vec![n]),
        numero_dossier: numero,
        numero_role: None,
        date_lecture: date,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;
    use serde_json::json;

    #[test]
    fn a_record_reads_as_a_decision_of_the_authority_of_its_day() {
        let record = json!({
            "id_decision": "09-D-10 ",
            "date_decision_datetime": "2009-02-27",
            "type_decision": "Décision",
            "url_site": "https://www.autoritedelaconcurrence.fr/fr/decision/x",
            "texte_complet_decision": "Le Conseil de la concurrence (section IV),",
        });
        let fields = adlc_source_fields(&record);
        assert!(fields.get(TEXT_FIELD).is_none());
        let d = Decision::from_source_fields(adlc_text(&record).unwrap(), &fields, "adlc/09-D-10");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("ADLC"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Conseil de la concurrence")
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("09-D-10"));
        assert_eq!(d.date_lecture.as_deref(), Some("2009-02-27"));

        let later = json!({"id_decision": "09-D-12", "date_decision_datetime": "2009-03-18"});
        let d = Decision::from_source_fields("", &later, "adlc/09-D-12");
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Autorité de la concurrence")
        );
    }

    #[test]
    fn the_opinions_are_not_decisions() {
        for (t, expected) in [
            (json!("DCC"), true),
            (json!("Décision"), true),
            (json!(["Décision", "MC"]), true),
            (json!(["DCC", "DEX"]), true),
            (json!("Avis"), false),
            (json!(["Avis", "SOA"]), false),
            (json!("Lettre du ministre de l'économie"), false),
        ] {
            assert_eq!(
                adlc_is_decision(&json!({ "type_decision": t })),
                expected,
                "{t}"
            );
        }
    }
}
