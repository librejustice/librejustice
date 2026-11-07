//! A contribution keeps its court, numbers, date and formation in its source fields.

use lj_core::decision::{Decision, DecisionSection};
use serde_json::{json, Value};

use crate::european::extract_sections_europe;
use crate::{extract_sections_xml, unzoned_judicial_sections};

pub(crate) fn source_uid_is_contribution(source_uid: &str) -> bool {
    source_uid.starts_with("contribution/")
}

/// What [`Decision::from_source_fields`](crate::FromSourceFields) reads back of a contribution.
pub fn contribution_source_fields(d: &Decision) -> Value {
    json!({
        "jurisdiction_type": d.jurisdiction_type,
        "jurisdiction_name": d.jurisdiction_name,
        "jurisdiction_location": d.jurisdiction_location,
        "jurisdiction_source_code": d.jurisdiction_source_code,
        "numbers": d.numero_dossiers,
        "decision_date": d.date_lecture,
        "formation": d.formation,
    })
}

pub(crate) fn from_source_fields_contribution(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let get = |key: &str| {
        source_fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let numbers: Option<Vec<String>> =
        source_fields
            .get("numbers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            });
    let numero_dossier = numbers.as_ref().and_then(|n| n.first().cloned());
    Decision {
        source_uid: source_uid.to_string(),
        member_name: numero_dossier.clone().unwrap_or_default(),
        ecli: None,
        jurisdiction_source_code: get("jurisdiction_source_code"),
        chamber: None,
        nac: None,
        jurisdiction_name: get("jurisdiction_name"),
        jurisdiction_type: get("jurisdiction_type"),
        jurisdiction_location: get("jurisdiction_location"),
        numero_dossier,
        numero_dossiers: numbers,
        numero_role: None,
        date_lecture: get("decision_date"),
        date_audience: None,
        date_mise_jour: None,
        formation: get("formation"),
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
        sections: sections(full_text, get("jurisdiction_type").as_deref()),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

/// A contribution has no zones: its sections are cut as its court drafts.
fn sections(full_text: &str, jurisdiction_type: Option<&str>) -> Vec<DecisionSection> {
    match jurisdiction_type {
        Some("CEDH" | "CJUE") => extract_sections_europe(full_text),
        Some("CA" | "TJ" | "TCOM" | "CPH" | "CC") => {
            let sections = unzoned_judicial_sections(full_text, jurisdiction_type == Some("CC"));
            if sections.iter().any(|s| s.kind == "dispositif") {
                return sections;
            }
            let markers = extract_sections_xml(full_text);
            if markers.is_empty() {
                sections
            } else {
                markers
            }
        }
        _ => extract_sections_xml(full_text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;

    #[test]
    fn a_contribution_reads_back_its_court_numbers_date_and_formation() {
        let mut written =
            from_source_fields_contribution("Le tribunal.", &json!({}), "contribution/1");
        written.jurisdiction_type = Some("TJ".into());
        written.jurisdiction_name = Some("Tribunal judiciaire de Bobigny".into());
        written.jurisdiction_location = Some("tj93008".into());
        written.numero_dossiers = Some(vec!["11-24-000123".into(), "34/2024".into()]);
        written.date_lecture = Some("2024-01-15".into());
        written.formation = Some("juge des contentieux de la protection".into());

        let read = Decision::from_source_fields(
            "Le tribunal.",
            &contribution_source_fields(&written),
            "contribution/1",
        );
        assert_eq!(read.jurisdiction_type.as_deref(), Some("TJ"));
        assert_eq!(read.jurisdiction_location.as_deref(), Some("tj93008"));
        assert_eq!(
            read.jurisdiction_name.as_deref(),
            Some("Tribunal judiciaire de Bobigny")
        );
        assert_eq!(read.numero_dossier.as_deref(), Some("11-24-000123"));
        assert_eq!(read.numero_dossiers, written.numero_dossiers);
        assert_eq!(read.date_lecture.as_deref(), Some("2024-01-15"));
        assert_eq!(read.formation, written.formation);
    }

    #[test]
    fn a_contribution_is_cut_as_its_court_drafts() {
        let kinds = |text: &str, court: &str| -> Vec<String> {
            from_source_fields_contribution(
                text,
                &json!({ "jurisdiction_type": court }),
                "contribution/1",
            )
            .sections
            .into_iter()
            .map(|s| s.kind)
            .collect()
        };
        let administrative = "TRIBUNAL ADMINISTRATIF DE LYON\nVu la procédure suivante :\nPar une requête, M. A demande l'annulation de l'arrêté.\nConsidérant ce qui suit :\n1. La requête est fondée.\nD E C I D E :\nArticle 1er : L'arrêté est annulé.";
        assert!(kinds(administrative, "TA").contains(&"dispositif".to_string()));
        let judicial = "COUR D'APPEL DE METZ\nARRÊT du 28 août 1991\nLA COUR, après en avoir délibéré :\nAttendu que le prévenu conteste.\nPAR CES MOTIFS\nLA COUR, statuant publiquement, confirme le jugement.";
        assert!(kinds(judicial, "CA").contains(&"dispositif".to_string()));
    }
}
