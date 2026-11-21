//! Benin's laws as the National Assembly's Omeka S collection files them, one item and one PDF each.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// A law as the collection's item describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnbeninLaw {
    pub item: u64,
    /// `LOI`, `LOI_ORGANIQUE` or `LOI_CONSTITUTIONNELLE`.
    pub nature: &'static str,
    /// As the title writes it, `99-001`, `91-010 bis`.
    pub number: String,
    pub day: Option<String>,
    /// The full title, `Loi n° 99-001 du 13 janvier 1999 portant loi de Finances pour la gestion 1999`.
    pub title: String,
    pub modified: Option<String>,
    pub media: Option<u64>,
}

/// `Loi n° 99-001 du 13 janvier 1999`, `Loi organique n° 2018-02 du …`, `N°61-1 du …`, `Loi n° 2016-06`.
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(loi(?:\s+organique|\s+constitutionnelle)?\s*)?n\s*[°º]\s*(.+?)(?:\s+du\s+(.+?))?\s*$")
        .unwrap()
});

fn value<'a>(item: &'a Value, term: &str) -> Option<&'a str> {
    item.get(term)?
        .as_array()?
        .iter()
        .find_map(|v| v.get("@value").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn day(s: &str) -> Option<String> {
    lj_core::dates::parse_date_fr(s).map(|d| d.to_string())
}

/// The laws of a page of items, the items that name no law left out.
pub fn anbenin_laws(json: &str) -> Result<Vec<AnbeninLaw>, String> {
    let items: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut laws = Vec::new();
    for item in &items {
        let id = item
            .get("o:id")
            .and_then(Value::as_u64)
            .ok_or("an item without an id")?;
        let short = item
            .get("o:title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let short = short.split_whitespace().collect::<Vec<_>>().join(" ");
        let Some(head) = HEAD.captures(&short) else {
            continue;
        };
        let nature = match head.get(1).map(|k| lj_fold::fold(k.as_str().trim())) {
            Some(k) if k.ends_with("organique") => "LOI_ORGANIQUE",
            Some(k) if k.ends_with("constitutionnelle") => "LOI_CONSTITUTIONNELLE",
            _ => "LOI",
        };
        let title = value(item, "dcterms:alternative")
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| t.len() > short.len())
            .unwrap_or_else(|| short.clone());
        laws.push(AnbeninLaw {
            item: id,
            nature,
            number: head[2].to_string(),
            day: head
                .get(3)
                .and_then(|d| day(d.as_str()))
                .or_else(|| value(item, "dcterms:issued").and_then(day))
                .or_else(|| value(item, "dcterms:date").and_then(day)),
            title,
            modified: item
                .pointer("/o:modified/@value")
                .and_then(Value::as_str)
                .map(str::to_string),
            media: item
                .pointer("/o:primary_media/o:id")
                .and_then(Value::as_u64),
        });
    }
    Ok(laws)
}

/// The PDF of each media of a page of media, by its id.
pub fn anbenin_pdfs(json: &str) -> Result<Vec<(u64, String)>, String> {
    let media: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(media
        .iter()
        .filter(|m| m.get("o:media_type").and_then(Value::as_str) == Some("application/pdf"))
        .filter_map(|m| {
            let id = m.get("o:id").and_then(Value::as_u64)?;
            let url = m.get("o:original_url").and_then(Value::as_str)?;
            Some((id, url.to_string()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_reads_as_a_law_with_its_number_its_day_and_its_full_title() {
        let json = r#"[
            {"o:id":2,"o:title":"Loi n° 99-001 du 13 janvier 1999","o:modified":{"@value":"2020-04-20T14:14:44+00:00"},
             "o:primary_media":{"o:id":31},
             "dcterms:alternative":[{"@value":"Loi n° 99-001 du 13 janvier 1999 portant loi de Finances pour la gestion 1999"}],
             "dcterms:date":[{"@value":"13/01/99"}]},
            {"o:id":2232,"o:title":"N°61-1 du 26 Janvier 1961","o:primary_media":{"o:id":2300}},
            {"o:id":430,"o:title":"Loi n° 2016-06","dcterms:date":[{"@value":"26 mai 2016"}]},
            {"o:id":2529,"o:title":"Loi N°84-001 Bis\tdu 13 août 1984"},
            {"o:id":9,"o:title":"Loi organique n° 2018-02 du 4 janvier 2019"},
            {"o:id":2192,"o:title":"Aperçu de l'Assemblée nationale du Bénin"}
        ]"#;
        let laws = anbenin_laws(json).unwrap();
        let read: Vec<(u64, &str, &str, Option<&str>)> = laws
            .iter()
            .map(|l| (l.item, l.nature, l.number.as_str(), l.day.as_deref()))
            .collect();
        assert_eq!(
            read,
            [
                (2, "LOI", "99-001", Some("1999-01-13")),
                (2232, "LOI", "61-1", Some("1961-01-26")),
                (430, "LOI", "2016-06", Some("2016-05-26")),
                (2529, "LOI", "84-001 Bis", Some("1984-08-13")),
                (9, "LOI_ORGANIQUE", "2018-02", Some("2019-01-04")),
            ]
        );
        assert_eq!(
            laws[0].title,
            "Loi n° 99-001 du 13 janvier 1999 portant loi de Finances pour la gestion 1999"
        );
        assert_eq!(laws[0].media, Some(31));
        assert_eq!(laws[1].title, "N°61-1 du 26 Janvier 1961");
    }
}
