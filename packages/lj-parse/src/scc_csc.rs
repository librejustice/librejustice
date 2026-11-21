//! A judgment page of the Supreme Court of Canada's site (`decisions.scc-csc.ca`) into a
//! [`Decision`] and its `source_fields`: the date, the citations and the docket from the page's
//! information table, the style of cause from its title, the text of its French version.

use serde_json::{json, Value};

use lj_core::decision::Decision;

pub const SCC_CODE: &str = "ca_cour_supreme";
pub const SCC_TYPE: &str = "COUR_SUPREME";
pub const SCC_LABEL: &str = "Cour suprême du Canada";
pub const SCC_PLACE: &str = "geo:CA";

/// What a judgment page of the French site shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SccPage {
    /// The item's number on the site, which its French and English versions share.
    pub id: u64,
    /// The style of cause, `Québec (Procureur général) c. SGS Canada inc.`.
    pub title: String,
    /// The rows of the information table, by label, a row's lines joined by a newline.
    pub fields: Vec<(String, String)>,
    /// The judgment, one paragraph a line.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SccRefused {
    NoDate,
    NoText,
    /// The French page serves the English text: the judgment has no French version.
    English,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SccDecision {
    pub decision: Decision,
    pub source_fields: Value,
}

/// The non-blank characters a judgment holds at least.
pub(crate) const DECISION_FLOOR: usize = 200;

pub fn scc_source_uid(id: u64) -> String {
    format!("scc-csc/{id}/fr")
}

pub fn scc_page_url(id: u64) -> String {
    format!("https://decisions.scc-csc.ca/scc-csc/scc-csc/fr/item/{id}/index.do")
}

/// Whether English function words outnumber French ones over the text's first 5,000 words.
pub(crate) fn english(text: &str) -> bool {
    let (fr, en) = function_words(text);
    en > fr
}

/// Whether French function words outnumber English ones over the text's first 5,000 words.
pub(crate) fn french(text: &str) -> bool {
    let (fr, en) = function_words(text);
    fr > en
}

/// The French and the English function words among the text's first 5,000 words.
fn function_words(text: &str) -> (usize, usize) {
    const FRENCH: [&str; 12] = [
        "le", "la", "les", "des", "du", "et", "que", "qui", "est", "dans", "pour", "une",
    ];
    const ENGLISH: [&str; 12] = [
        "the", "of", "and", "to", "that", "is", "for", "by", "this", "with", "was", "which",
    ];
    let (mut fr, mut en) = (0usize, 0usize);
    for word in text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .take(5_000)
    {
        let word = word.to_lowercase();
        fr += usize::from(FRENCH.contains(&word.as_str()));
        en += usize::from(ENGLISH.contains(&word.as_str()));
    }
    (fr, en)
}

fn decision_of(id: u64, date: String, dockets: Vec<String>, text: String) -> Decision {
    Decision {
        source_uid: scc_source_uid(id),
        member_name: id.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(SCC_CODE.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(SCC_LABEL.to_string()),
        jurisdiction_type: Some(SCC_TYPE.to_string()),
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

pub(crate) fn strings(source_fields: &Value, key: &str) -> Vec<String> {
    source_fields
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// A row's values: its lines, or its `;`-separated items, each once.
pub(crate) fn items(value: Option<&str>, separators: &[char]) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for item in value
        .into_iter()
        .flat_map(|v| v.split(|c| c == '\n' || separators.contains(&c)))
        .map(str::trim)
        .filter(|i| !i.is_empty())
    {
        if !items.iter().any(|i| i == item) {
            items.push(item.to_string());
        }
    }
    items
}

pub fn scc_decision(page: &SccPage) -> Result<SccDecision, SccRefused> {
    let field = |label: &str| {
        page.fields
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
    };
    let date = field("Date")
        .filter(|d| d.parse::<jiff::civil::Date>().is_ok())
        .ok_or(SccRefused::NoDate)?;
    if page.text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(SccRefused::NoText);
    }
    if english(&page.text) {
        return Err(SccRefused::English);
    }
    let dockets = items(field("Numéro de dossier"), &[',', ';']);
    let source_fields = json!({
        "scc_id": page.id,
        "title": page.title,
        "date": date,
        "neutral_citation": field("Référence neutre"),
        "report_citation": field("Recueil"),
        "dockets": dockets,
        "judges": items(field("Juges"), &[';']),
        "appeal_from": field("En appel de"),
        "subjects": items(field("Sujets"), &[]),
        "source_url": scc_page_url(page.id),
        "lang": "fra",
    });
    Ok(SccDecision {
        decision: decision_of(page.id, date.to_string(), dockets, page.text.clone()),
        source_fields,
    })
}

/// Branch of [`Decision::from_source_fields`] for `scc-csc/`, on what the parse stored.
pub(crate) fn from_source_fields_scc(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let id = source_fields
        .get("scc_id")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("source_fields scc-csc without an id ({source_uid})"));
    let date = source_fields
        .get("date")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("source_fields scc-csc without a date ({source_uid})"));
    decision_of(
        id,
        date.to_string(),
        strings(source_fields, "dockets"),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRENCH: &str = "Le pourvoi est rejeté. La Cour d'appel a conclu que les relations de \
        travail de la succursale relèvent de la compétence fédérale dérivée, et que le tribunal \
        n'avait pas compétence sur la requête en accréditation présentée par le syndicat pour \
        représenter les inspecteurs de grains qui travaillent dans les silos du fleuve.";

    const ENGLISH: &str = "The appeal is dismissed. The Court of Appeal held that the labour \
        relations of the branch fall under derivative federal jurisdiction, and that the tribunal \
        had no jurisdiction over the certification application which was filed by the union to \
        represent the grain inspectors working for the elevators along the river.";

    fn page(fields: &[(&str, &str)], text: &str) -> SccPage {
        SccPage {
            id: 21749,
            title: "Québec (Procureur général) c. SGS Canada inc.".into(),
            fields: fields
                .iter()
                .map(|(l, v)| (l.to_string(), v.to_string()))
                .collect(),
            text: text.into(),
        }
    }

    #[test]
    fn a_translated_judgment_keeps_french_as_its_language_and_its_citations() {
        let text = format!("COUR SUPRÊME DU CANADA\nTraduction française officielle\n{FRENCH}");
        let parsed = scc_decision(&page(
            &[
                ("Date", "2026-11-06"),
                ("Référence neutre", "2026 CSC 37"),
                ("Numéro de dossier", "41334, 41335"),
                ("Juges", "Wagner, Richard; Karakatsanis, Andromache"),
                ("Sujets", "Droit constitutionnel\nRelations de travail"),
            ],
            &text,
        ))
        .unwrap();
        let sf = &parsed.source_fields;
        assert_eq!(sf.get("original_lang"), None);
        assert_eq!(sf["lang"], "fra");
        assert_eq!(sf["neutral_citation"], "2026 CSC 37");
        assert_eq!(sf["report_citation"], Value::Null);
        assert_eq!(sf["judges"][1], "Karakatsanis, Andromache");
        assert_eq!(sf["subjects"][1], "Relations de travail");
        let d = &parsed.decision;
        assert_eq!(d.source_uid, "scc-csc/21749/fr");
        assert_eq!(
            d.numero_dossiers,
            Some(vec!["41334".to_string(), "41335".to_string()])
        );
        let rebuilt = from_source_fields_scc(&d.texte_integral_clean, sf, &d.source_uid);
        assert_eq!(&rebuilt, d);
    }

    #[test]
    fn a_report_era_judgment_has_its_report_citation_and_no_docket() {
        let fields = [("Date", "1970-12-21"), ("Recueil", "[1971] RCS 321")];
        let parsed = scc_decision(&page(&fields, FRENCH)).unwrap();
        assert_eq!(parsed.source_fields["report_citation"], "[1971] RCS 321");
        assert_eq!(parsed.decision.numero_dossiers, None);
    }

    #[test]
    fn a_page_serving_the_english_text_is_refused() {
        assert_eq!(
            scc_decision(&page(&[("Date", "1985-03-01")], ENGLISH)),
            Err(SccRefused::English)
        );
        assert_eq!(scc_decision(&page(&[], FRENCH)), Err(SccRefused::NoDate));
    }
}
