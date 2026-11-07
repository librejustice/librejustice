//! A competition decision of the European Commission, from its case search record.

use lj_core::decision::Decision;
use serde_json::Value;

use crate::extract_sections_xml;

pub const COMEU_COURTS: [(&str, &str, &str); 5] = [
    (
        "AT",
        "comeu_at",
        "Commission européenne, ententes et abus de position dominante",
    ),
    ("M", "comeu_m", "Commission européenne, concentrations"),
    ("SA", "comeu_sa", "Commission européenne, aides d'État"),
    (
        "InstrumentDMA",
        "comeu_dma",
        "Commission européenne, marchés numériques",
    ),
    (
        "InstrumentFS",
        "comeu_fs",
        "Commission européenne, subventions étrangères",
    ),
];

/// The adoption date as Brussels reads it: the search stores its midnight in UTC, the day before.
pub const COMEU_ADOPTION_DATE: &str = "adoption_date";

pub(crate) fn source_uid_is_comeu(source_uid: &str) -> bool {
    source_uid.starts_with("comeu/")
}

pub fn comeu_court_code(label: &str) -> Option<&'static str> {
    COMEU_COURTS
        .iter()
        .find(|(_, _, l)| *l == label)
        .map(|(_, code, _)| *code)
}

/// The first value of a field of the case search, which holds every value in an array.
pub fn comeu_first<'a>(fields: &'a Value, key: &str) -> Option<&'a str> {
    fields[key].as_array()?.first()?.as_str()
}

pub(crate) fn from_source_fields_comeu(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let number = comeu_first(source_fields, "caseNumber").map(str::to_string);
    let court = comeu_first(source_fields, "caseInstrument")
        .and_then(|i| {
            COMEU_COURTS
                .iter()
                .find(|(instrument, _, _)| *instrument == i)
        })
        .map(|(_, _, label)| label.to_string());
    let reference = source_uid
        .strip_prefix("comeu/")
        .and_then(|rest| rest.rsplit_once('/'))
        .map_or(source_uid, |(reference, _)| reference);
    Decision {
        source_uid: source_uid.to_string(),
        member_name: reference.to_string(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: court,
        jurisdiction_type: Some("COMEU".to_string()),
        jurisdiction_location: None,
        numero_dossiers: number.clone().map(|n| vec![n]),
        numero_dossier: number,
        numero_role: None,
        date_lecture: source_fields[COMEU_ADOPTION_DATE]
            .as_str()
            .map(str::to_string),
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
    use crate::FromSourceFields;
    use lj_core::decision::Decision;
    use serde_json::json;

    #[test]
    fn a_record_reads_as_a_decision_of_its_instrument() {
        let fields = json!({
            "caseNumber": ["SA.113145"],
            "caseInstrument": ["SA"],
            "caseTitle": ["Prolongation of the Danish scheme"],
            "adoption_date": "2024-10-04",
        });
        let uid = "comeu/SA.113145-DEC5D7C8841-FF72-49A2-8AB5-87F2966ECCB1/da";
        let d = Decision::from_source_fields("Kommissionen har besluttet", &fields, uid);
        assert_eq!(d.jurisdiction_type.as_deref(), Some("COMEU"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Commission européenne, aides d'État")
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("SA.113145"));
        assert_eq!(d.date_lecture.as_deref(), Some("2024-10-04"));
        assert_eq!(
            d.member_name,
            "SA.113145-DEC5D7C8841-FF72-49A2-8AB5-87F2966ECCB1"
        );
        assert_eq!(
            super::comeu_court_code("Commission européenne, aides d'État"),
            Some("comeu_sa")
        );
    }
}
