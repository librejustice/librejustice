//! A Légimonaco decision into a [`Decision`] and its `source_fields`: the court from its
//! jurisdiction, the text its index holds without the editors' abstract and summary at its head,
//! the dockets its header states.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

pub const MONACO: &str = "geo:MC";

/// A court of Monaco the load takes, by its slug in the index.
#[derive(Debug)]
pub struct MonacoCourt {
    pub slug: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
}

const fn court(
    slug: &'static str,
    jurisdiction_type: &'static str,
    code: &'static str,
    label: &'static str,
) -> MonacoCourt {
    MonacoCourt {
        slug,
        jurisdiction_type,
        code,
        label,
    }
}

/// Every court the index serves decisions of, but the European Court of Human Rights.
pub const MONACO_COURTS: [MonacoCourt; 12] = [
    court(
        "tribunal-supreme",
        "COUR_CONSTITUTIONNELLE",
        "mc_cour_constitutionnelle",
        "Tribunal suprême (Monaco)",
    ),
    court(
        "cour-revision",
        "COUR_CASSATION",
        "mc_cour_cassation",
        "Cour de révision (Monaco)",
    ),
    court(
        "cour-superieure-arbitrage",
        "COUR_CASSATION",
        "mc_cour_cassation_arbitrage",
        "Cour supérieure d'arbitrage (Monaco)",
    ),
    court(
        "cour-appel",
        "COUR_APPEL",
        "mc_cour_appel",
        "Cour d'appel (Monaco)",
    ),
    court(
        "tribunal-criminel",
        "COUR_CRIMINELLE",
        "mc_cour_criminelle",
        "Tribunal criminel (Monaco)",
    ),
    court(
        "tribunal-premiere-instance",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance",
        "Tribunal de première instance (Monaco)",
    ),
    court(
        "tribunal-correctionnel",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_correctionnel",
        "Tribunal correctionnel (Monaco)",
    ),
    court(
        "tribunal-travail",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_travail",
        "Tribunal du travail (Monaco)",
    ),
    court(
        "juge-tutelaire",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_juge_tutelaire",
        "Juge tutélaire (Monaco)",
    ),
    court(
        "juge-paix",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_juge_paix",
        "Juge de paix (Monaco)",
    ),
    court(
        "commission-arbitrale-loyers-commerciaux",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_loyers_commerciaux",
        "Commission arbitrale des loyers commerciaux (Monaco)",
    ),
    court(
        "commission-arbitrale-loyers-habitation",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "mc_tribunal_premiere_instance_loyers_habitation",
        "Commission arbitrale des loyers d'habitation (Monaco)",
    ),
];

pub fn monaco_court(slug: &str) -> Option<&'static MonacoCourt> {
    MONACO_COURTS.iter().find(|c| c.slug == slug)
}

#[derive(Debug)]
pub struct LegimonacoParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static MonacoCourt,
}

/// The non-blank characters a decision text holds at least.
const DECISION_FLOOR: usize = 200;

/// The header lines a docket is read from, before the parties.
const HEADER_LINES: usize = 8;

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:pourvoi\s+)?n(?:°|os)\s*(?:rg\s*)?:?\s*([^(]*)").expect("valid regex")
});
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]?\d{1,4}(?:[-/]\d+)+\b").expect("valid regex"));
static SUPREME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^TS\s?\d{4}-\d+\b").expect("valid regex"));
static BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]?\d{4}/\d+$").expect("valid regex"));

/// The dockets the header states: `N° 2025/000555 (assignation du …)`, `Pourvoi N° 2026/000051`,
/// `Nos 88-2024/2025 et 40-2025/2026`, `N°RG : 2025/000179`, one `TS 2025-43` a line, a bare
/// `C2013/0000008`.
fn header_dockets(text: &str) -> Vec<String> {
    let mut dockets: Vec<String> = Vec::new();
    for line in text.lines().take(HEADER_LINES) {
        let line = line.trim();
        if ["En la cause", "Et en la cause", "Affaire"]
            .iter()
            .any(|p| line.starts_with(p))
        {
            break;
        }
        let found: Vec<&str> = if let Some(numbers) = NUMBERED.captures(line).and_then(|c| c.get(1))
        {
            NUMBER
                .find_iter(numbers.as_str())
                .map(|m| m.as_str())
                .collect()
        } else if let Some(supreme) = SUPREME.find(line) {
            vec![supreme.as_str()]
        } else if BARE.is_match(line) {
            vec![line]
        } else {
            Vec::new()
        };
        for docket in found {
            if !dockets.iter().any(|d| d == docket) {
                dockets.push(docket.to_string());
            }
        }
    }
    dockets
}

/// One paragraph a line, as the index holds them.
fn clean_text(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `body` without `head` at its start; `None` when `head` is not there.
fn without_head<'a>(body: &'a str, head: Option<&str>) -> Option<&'a str> {
    match head.map(str::trim).filter(|h| !h.is_empty()) {
        None => Some(body),
        Some(head) => body.strip_prefix(head).map(str::trim_start),
    }
}

fn text_field<'a>(doc: &'a Value, key: &str) -> Option<&'a str> {
    doc.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn decision_of(
    id: &str,
    court: &MonacoCourt,
    date: String,
    dockets: Vec<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid: format!("legimonaco/{id}"),
        member_name: id.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: dockets.first().cloned(),
        numero_dossiers: (!dockets.is_empty()).then_some(dockets),
        numero_role: None,
        date_lecture: Some(date),
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
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

fn dockets_of(source_fields: &Value) -> Vec<String> {
    source_fields
        .get("dockets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Parses a decision's `_source` and the bodies of its `Abstract` and `Résumé` sections, which
/// the editors write and its `enBody` opens with.
pub fn parse_legimonaco(
    raw: &str,
    abstract_body: Option<&str>,
    summary_body: Option<&str>,
) -> Result<LegimonacoParsed> {
    let fail = |id: &str, why: String| CoreError::Xml(format!("legimonaco {id}: {why}"));
    let doc: Value =
        serde_json::from_str(raw).map_err(|e| CoreError::Xml(format!("legimonaco: {e}")))?;
    let id = text_field(&doc, "contentId").ok_or_else(|| fail("?", "no contentId".into()))?;
    let slug =
        text_field(&doc, "jurisdiction").ok_or_else(|| fail(id, "no jurisdiction".into()))?;
    let court = monaco_court(slug).ok_or_else(|| fail(id, format!("court {slug} not taken")))?;
    let date = text_field(&doc, "date").ok_or_else(|| fail(id, "no date".into()))?;
    date.parse::<jiff::civil::Date>()
        .map_err(|e| fail(id, format!("date {date:?}: {e}")))?;
    let body = text_field(&doc, "enBody").unwrap_or_default();
    let body = without_head(body, abstract_body)
        .ok_or_else(|| fail(id, "the abstract does not open the text".into()))?;
    let body = without_head(body, summary_body)
        .ok_or_else(|| fail(id, "the summary does not open the text".into()))?;
    let text = clean_text(body);
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(fail(id, "no decision text".into()));
    }
    let dockets = header_dockets(&text);
    let source_fields = json!({
        "legimonaco_id": id,
        "idbd": text_field(&doc, "idbd"),
        "court": court.slug,
        "matter": text_field(&doc, "commonCaseArea"),
        "date": date,
        "title": text_field(&doc, "title"),
        "parties": text_field(&doc, "parties"),
        "source_url": text_field(&doc, "path").map(|p| format!("https://legimonaco.mc{p}")),
        "dockets": dockets,
        "abstract": abstract_body.map(clean_text),
        "summary": summary_body.map(clean_text),
        "thematic": doc.get("thematic").cloned().unwrap_or(json!([])),
    });
    let decision = decision_of(id, court, date.to_string(), dockets, text);
    Ok(LegimonacoParsed {
        decision,
        source_fields,
        court,
    })
}

/// Branch of [`Decision::from_source_fields`] for `legimonaco/`, on what the parse stored.
pub(crate) fn from_source_fields_legimonaco(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let id = source_uid.strip_prefix("legimonaco/").unwrap_or(source_uid);
    let court = field("court")
        .and_then(monaco_court)
        .unwrap_or_else(|| panic!("stored legimonaco court not taken ({source_uid})"));
    let date = field("date")
        .unwrap_or_else(|| panic!("source_fields legimonaco without a date ({source_uid})"));
    decision_of(
        id,
        court,
        date.to_string(),
        dockets_of(source_fields),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOTIFS: &str = "Attendu que la Cour d'appel, appréciant souverainement les éléments de \
        fait et de preuve qui lui étaient soumis, a constaté que le changement d'emploi ne \
        constituait pas une circonstance revêtant un caractère exceptionnel ; qu'elle en a \
        exactement déduit que cet avantage devait être maintenu ; que le moyen n'est pas fondé ;\n\
        PAR CES MOTIFS,\nRejette le pourvoi.";

    fn document(court: &str, body: &str) -> String {
        json!({
            "contentId": "pv0000A",
            "idbd": "31492",
            "jurisdiction": court,
            "commonCaseArea": "sociale",
            "date": "2026-11-05",
            "title": "Cour de révision, 5 novembre 2026, La SAM A c/ d B",
            "path": "/jurisprudence/cour-revision/2026/11-05-31492",
            "enBody": body,
        })
        .to_string()
    }

    #[test]
    fn the_text_starts_after_the_editors_abstract_and_summary() {
        let abstract_body = "Contrat de travail - Avenant - Dol";
        let summary = "L'avenant comportait les informations omises.\nLe dol est établi.";
        let body = format!(
            "{abstract_body}\n{summary}\nPourvoi N° 2026/000051 Hors session T. T\n\
             COUR DE RÉVISION\nARRÊT DU 5 NOVEMBRE 2026\nEn la cause de :\n{MOTIFS}"
        );
        let parsed = parse_legimonaco(
            &document("cour-revision", &body),
            Some(abstract_body),
            Some(summary),
        )
        .unwrap();
        let d = &parsed.decision;
        assert!(d.texte_integral_clean.starts_with("Pourvoi N° 2026/000051"));
        assert_eq!(d.source_uid, "legimonaco/pv0000A");
        assert_eq!(
            d.jurisdiction_source_code.as_deref(),
            Some("mc_cour_cassation")
        );
        assert_eq!(d.numero_dossiers, Some(vec!["2026/000051".to_string()]));
        assert_eq!(parsed.source_fields["summary"], summary);
        assert_eq!(
            parsed.source_fields["source_url"],
            "https://legimonaco.mc/jurisprudence/cour-revision/2026/11-05-31492"
        );
        let rebuilt = from_source_fields_legimonaco(
            &d.texte_integral_clean,
            &parsed.source_fields,
            &d.source_uid,
        );
        assert_eq!(rebuilt.numero_dossiers, d.numero_dossiers);
        assert_eq!(rebuilt.jurisdiction_type, d.jurisdiction_type);
    }

    #[test]
    fn a_summary_that_does_not_open_the_text_refuses_the_decision() {
        let body = format!("Contrat de travail\n{MOTIFS}");
        let raw = document("cour-revision", &body);
        assert!(parse_legimonaco(&raw, Some("Contrat de travail"), Some("Autre résumé")).is_err());
        assert!(parse_legimonaco(&raw, Some("Contrat de travail"), None).is_ok());
    }

    #[test]
    fn the_echr_is_refused() {
        let raw = document("cour-europeenne-droits-homme", MOTIFS);
        assert!(parse_legimonaco(&raw, None, None).is_err());
    }

    #[test]
    fn the_header_states_the_dockets_in_each_court_s_form() {
        let cases: [(&str, &[&str]); 8] = [
            (
                "TRIBUNAL DE PREMIÈRE INSTANCE\nN° 2025/000555 (assignation du 4 juin 2025)\nJUGEMENT DU 9 JUILLET 2026",
                &["2025/000555"],
            ),
            (
                "Pourvoi N° 2026/000012 en session civile\nCOUR DE RÉVISION",
                &["2026/000012"],
            ),
            (
                "TRIBUNAL DU TRAVAIL\nJUGEMENT DU 13 AOÛT 2026\nNos 88-2024/2025 et 40-2025/2026",
                &["88-2024/2025", "40-2025/2026"],
            ),
            (
                "ORDONNANCE DE RÉFÉRÉ\nrendue le 8 JUILLET 2026\nN°RG : 2025/000179\nAssignation du 29 juillet 2025",
                &["2025/000179"],
            ),
            (
                "TRIBUNAL SUPRÊME\nTS 2025-43\nTS 2025-44\nAffaires :\na A",
                &["TS 2025-43", "TS 2025-44"],
            ),
            (
                "COMMISSION ARBITRALE DES LOYERS COMMERCIAUX\nn° C2013/000001\nJUGEMENT DU 19 MARS 2014",
                &["C2013/000001"],
            ),
            (
                "COMMISSION ARBITRALE DES LOYERS COMMERCIAUX\n-------------------\nC2013/0000008\nJUGEMENT DU 15 AVRIL 2015",
                &["C2013/0000008"],
            ),
            (
                "COUR D'APPEL\nARRÊT DU 23 JUIN 2026\nEn la cause de :\n- La SAM A, immatriculée sous le numéro 2014/000123",
                &[],
            ),
        ];
        for (header, expected) in cases {
            assert_eq!(header_dockets(header), expected, "{header}");
        }
    }

    #[test]
    fn each_court_is_a_foreign_family_coded_from_its_place() {
        for court in &MONACO_COURTS {
            assert!(
                lj_vocab::courts::is_foreign(court.jurisdiction_type),
                "{court:?}"
            );
            let rest = court
                .code
                .strip_prefix(&lj_vocab::courts::foreign_court_code(
                    "mc",
                    court.jurisdiction_type,
                    &[],
                ))
                .unwrap_or_else(|| panic!("{court:?}"));
            assert!(rest.is_empty() || rest.starts_with('_'), "{court:?}");
        }
    }
}
