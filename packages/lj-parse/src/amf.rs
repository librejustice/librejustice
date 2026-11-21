//! A decision of the AMF's Commission des sanctions, from its page and the text of its file.

use std::sync::OnceLock;

use jiff::civil::Date;
use jiff::ToSpan;
use lj_core::dates::parse_date_fr;
use lj_core::decision::Decision;
use regex::Regex;
use serde_json::{json, Value};

use crate::extract_sections_xml;

const AUTHORITY: &str = "Autorité des marchés financiers";
/// The AMF puts the anonymised file online up to a month before the date it set.
const EARLY_MONTHS: i8 = 2;

pub(crate) fn source_uid_is_amf(source_uid: &str) -> bool {
    source_uid.starts_with("amf/")
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// The page as read and its URL, the file's key in the bucket, and the day the file stops naming.
pub fn amf_source_fields(page: Value, file: &str, named_until: Option<Date>) -> Value {
    let mut fields = json!({ "url_site": page["url"], "page": page, "file": file });
    if let Some(until) = named_until {
        fields["named_until"] = json!(until.to_string());
    }
    fields
}

/// The date in « Décision … du 30 avril 2021 à l'égard de … ».
pub fn amf_decided(title: &str) -> Option<Date> {
    static DATE: OnceLock<Regex> = OnceLock::new();
    let c = regex(&DATE, r"\bdu (\d{1,2}(?:er)? \p{L}+ \d{4})").captures(title)?;
    parse_date_fr(&c[1])
}

/// The day the file at `file` stops naming the persons the decision sanctions, from the clause of
/// its operative part; `None` when it names them for no set time or is the version for after.
pub fn amf_named_until(title: &str, text: &str, file: &str) -> Option<Date> {
    static CLAUSE: OnceLock<Regex> = OnceLock::new();
    static FOLDER: OnceLock<Regex> = OnceLock::new();
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('’', "'");
    let years = regex(
        &CLAUSE,
        r"\bfixer? à (\w+) ans,? à compter de (?:la date de )?la présente décision,? la durée de son maintien en ligne de manière non anonyme",
    )
    .captures(&text)?;
    let years: i16 = match years[1].to_lowercase().as_str() {
        "un" => 1,
        "deux" => 2,
        "trois" => 3,
        "quatre" => 4,
        "cinq" => 5,
        "six" => 6,
        "sept" => 7,
        "huit" => 8,
        "neuf" => 9,
        "dix" => 10,
        n => n.parse().ok()?,
    };
    let until = amf_decided(title)?.checked_add(years.years()).ok()?;
    let folder = regex(&FOLDER, r"/private/(\d{4})-(\d{2})/")
        .captures(file)
        .and_then(|c| Date::new(c[1].parse().ok()?, c[2].parse().ok()?, 1).ok());
    let after = until
        .first_of_month()
        .checked_sub(EARLY_MONTHS.months())
        .ok()?;
    match folder {
        Some(folder) if folder >= after => None,
        _ => Some(until),
    }
}

pub(crate) fn from_source_fields_amf(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let page = |key: &str| {
        source_fields
            .get("page")
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let number = page("number").to_string();
    Decision {
        source_uid: source_uid.to_string(),
        member_name: number.clone(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some(AUTHORITY.to_string()),
        jurisdiction_type: Some("AMF".to_string()),
        jurisdiction_location: None,
        numero_dossiers: Some(vec![number.clone()]),
        numero_dossier: Some(number),
        numero_role: None,
        date_lecture: amf_decided(page("title")).map(|d| d.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: Some("Commission des sanctions".to_string()),
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

    const TITLE: &str = "SAN-2021-08 - Décision de la Commission des sanctions du 30 avril 2021 à l'égard de la société X et de M. A";
    const DISPOSITIF: &str = "- ordonne la publication de la présente décision sur le site Internet de l’Autorité des marchés financiers et fixe à cinq ans à compter de la date de la présente décision la durée de son\nmaintien en ligne de manière non anonyme.";

    #[test]
    fn the_operative_part_sets_the_day_the_decision_stops_naming() {
        let d = |s: &str| s.parse::<Date>().unwrap();
        assert_eq!(
            amf_named_until(
                TITLE,
                DISPOSITIF,
                "/sites/institutionnel/files/private/2021-05/san-2021-08.pdf"
            ),
            Some(d("2026-04-30"))
        );
        assert_eq!(
            amf_named_until(
                TITLE,
                &DISPOSITIF.replace("cinq", "3"),
                "/sites/institutionnel/files/sanction/decision.pdf"
            ),
            Some(d("2024-04-30"))
        );
        assert_eq!(
            amf_named_until(
                TITLE,
                DISPOSITIF,
                "/sites/institutionnel/files/private/2026-03/san-2021-08.pdf"
            ),
            None
        );
        assert_eq!(
            amf_named_until(TITLE, "ordonne la publication de la présente décision.", ""),
            None
        );
    }

    #[test]
    fn a_page_reads_as_a_decision_of_the_amf() {
        let fields = amf_source_fields(
            json!({ "number": "SAN-2021-08", "title": TITLE }),
            "amf/SAN-2021-08.pdf",
            None,
        );
        let d = Decision::from_source_fields(DISPOSITIF, &fields, "amf/SAN-2021-08");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("AMF"));
        assert_eq!(d.numero_dossier.as_deref(), Some("SAN-2021-08"));
        assert_eq!(d.date_lecture.as_deref(), Some("2021-04-30"));
    }
}
