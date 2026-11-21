//! A decision page of conseil-constitutionnel.fr, read into the fields its DILA CONSTIT copy carries.

use std::sync::OnceLock;

use lj_core::dates::parse_date_fr;
use regex::Regex;
use serde_json::{json, Value};

/// What a decision page shows, its text one paragraph a line.
#[derive(Debug, Clone, PartialEq)]
pub struct CcSitePage {
    pub url: String,
    /// `Décision n° 2026-1228 QPC du 22 octobre 2026 | Conseil constitutionnel`.
    pub title: String,
    /// The parties and the subject, `M. Mohcine S. [Consultation …]`.
    pub parties: String,
    pub solution: Option<String>,
    pub paragraphs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CcSiteDecision {
    pub source_uid: String,
    pub text: String,
    pub source_fields: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcSiteRefused {
    NoNumber,
    NoDate,
    NoText,
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// The number and kind come from the page's address, which names every number of a joined
/// decision where the page's ECLI names the first; the date from the title.
pub fn cc_site_decision(page: &CcSitePage) -> Result<CcSiteDecision, CcSiteRefused> {
    static ADDRESS: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    let address = regex(
        &ADDRESS,
        r"/decision/(\d{4})/(\d{4})([0-9_]+)([A-Z]+)\.htm$",
    )
    .captures(&page.url)
    .ok_or(CcSiteRefused::NoNumber)?;
    let (year, series, kind) = (&address[1], &address[2], &address[4]);
    let numbers: Vec<&str> = address[3].split('_').collect();
    let numero = format!("{series}-{}", numbers.join("/"));
    let ecli = format!("{series}.{}.{kind}", numbers.join("."));
    let date = regex(&DATE, r"\bdu (\d{1,2}(?:er)? \S+ \d{4})")
        .captures(&page.title)
        .and_then(|c| parse_date_fr(&c[1]))
        .ok_or(CcSiteRefused::NoDate)?
        .to_string();
    let end = page
        .paragraphs
        .iter()
        .position(|p| p.starts_with("JORF") || p.starts_with("ECLI"))
        .unwrap_or(page.paragraphs.len());
    if end == 0 {
        return Err(CcSiteRefused::NoText);
    }
    let journal = page.paragraphs[end..]
        .iter()
        .find(|p| p.starts_with("JORF"))
        .map(|p| p.trim_end_matches([',', ' ']).to_string());
    let source_uid = format!("cc-site/{year}/{ecli}");
    let source_fields = json!({
        "META_COMMUN": {"ID": source_uid},
        "META_JURI": {
            "TITRE": page.parties,
            "NUMERO": numero,
            "DATE_DEC": date,
            "SOLUTION": page.solution,
            "JURIDICTION": "Conseil constitutionnel",
        },
        "META_JURI_CONSTIT": {
            "ECLI": format!("ECLI:FR:CC:{year}:{ecli}"),
            "URL_CC": page.url,
            "TITRE_JO": journal,
            "NATURE_QUALIFIEE": kind,
        },
    });
    Ok(CcSiteDecision {
        source_uid,
        text: page.paragraphs[..end].join("\n"),
        source_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;
    use lj_core::decision::Decision;

    fn page(url: &str, title: &str) -> CcSitePage {
        CcSitePage {
            url: format!("https://www.conseil-constitutionnel.fr/decision/2026/{url}"),
            title: title.into(),
            parties: "Société A et autre [Objet]".into(),
            solution: Some("Conformité".into()),
            paragraphs: vec![
                "LE CONSEIL CONSTITUTIONNEL A ÉTÉ SAISI le 1er août 2026.".into(),
                "LE CONSEIL CONSTITUTIONNEL DÉCIDE :".into(),
                "Rendu public le 23 octobre 2026.".into(),
                "JORF n°0249 du 24 octobre 2026, texte n° 80,".into(),
                "ECLI : FR : CC : 2026 : 2026.1230.QPC".into(),
            ],
        }
    }

    #[test]
    fn a_joined_decision_takes_the_dila_numbering_and_builds_a_constit_decision() {
        let read = cc_site_decision(&page(
            "20261230_1231QPC.htm",
            "Décision n° 2026-1230/1231 QPC du 23 octobre 2026 | Conseil constitutionnel",
        ))
        .unwrap();
        assert_eq!(read.source_uid, "cc-site/2026/2026.1230.1231.QPC");
        assert_eq!(read.source_fields["META_JURI"]["NUMERO"], "2026-1230/1231");
        assert_eq!(read.source_fields["META_JURI"]["DATE_DEC"], "2026-10-23");
        assert_eq!(
            read.source_fields["META_JURI_CONSTIT"]["ECLI"],
            "ECLI:FR:CC:2026:2026.1230.1231.QPC"
        );
        assert_eq!(
            read.source_fields["META_JURI_CONSTIT"]["TITRE_JO"],
            "JORF n°0249 du 24 octobre 2026, texte n° 80"
        );
        assert!(read.text.ends_with("Rendu public le 23 octobre 2026."));

        let decision =
            Decision::from_source_fields(&read.text, &read.source_fields, &read.source_uid);
        assert_eq!(decision.jurisdiction_type.as_deref(), Some("CONSTIT"));
        assert_eq!(
            decision.numero_dossiers,
            Some(vec!["2026-1230".to_string(), "2026-1231".to_string()])
        );
        assert_eq!(decision.date_lecture.as_deref(), Some("2026-10-23"));
    }

    #[test]
    fn the_number_is_the_address_s_when_the_title_mistypes_it() {
        let read = cc_site_decision(&page(
            "2026169ORGA.htm",
            "Décision n° 2019-169 ORGA du 8 octobre 2026 | Conseil constitutionnel",
        ))
        .unwrap();
        assert_eq!(read.source_fields["META_JURI"]["NUMERO"], "2026-169");
        assert_eq!(read.source_fields["META_JURI"]["DATE_DEC"], "2026-10-08");
    }
}
