//! The Vlaamse Codex: one dated state of a Flemish act, each article version with its own dates.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// An act as the Codex dates it, and the article versions of one of its states.
#[derive(Debug)]
pub struct CodexState {
    pub numac: Option<String>,
    pub kind: String,
    pub title: String,
    pub adopted: Option<String>,
    /// The day the Moniteur published it.
    pub published: Option<String>,
    pub versions: Vec<CodexVersion>,
}

/// One version of a numbered article or annex, its last day included in `end`.
#[derive(Debug)]
pub struct CodexVersion {
    pub id: u64,
    pub num: String,
    pub title_path: Option<String>,
    pub texte: String,
    pub start: Option<String>,
    pub end: Option<String>,
}

static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</?p\b[^>]*>|</?div\b[^>]*>|</tr>").unwrap());
static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<span class="reference">.*?</span>"#).unwrap());
/// An amendment note the Codex writes inline: `(ing. decr. 18 juli 2003, art. 75, I: …)`.
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\((?:ing|verv|opgeh|gew|vernummerd|hernummerd)\.{1,2}\s[^()]*\)").unwrap()
});
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(a|b|em|i|span|strong|sub|sup|u)\b[^>]*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

fn plain(html: &str) -> String {
    let html = REFERENCE.replace_all(html, "");
    let html = BREAK.replace_all(&html, "\n");
    let html = INLINE_TAG.replace_all(&html, "");
    let bare = TAG.replace_all(&html, " ");
    let bare = html_escape::decode_html_entities(&bare);
    NOTE.replace_all(&bare, "")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A heading without the brackets of an inserted one nor its amendment note.
fn heading(title: &str) -> String {
    let title = NOTE.replace_all(title, "");
    let title = title
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(&[']', '}'][..])
        .trim();
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('>', "›")
}

/// The heading path of each article version the table of contents files under a heading.
fn paths(items: &[Value], path: &mut Vec<String>, out: &mut HashMap<u64, String>) {
    for item in items {
        let children = item
            .get("Items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if item.get("ItemType").and_then(Value::as_str) == Some("H") {
            let label = text(item, "Titel").map(heading).unwrap_or_default();
            let pushed = !label.is_empty();
            if pushed {
                path.push(label);
            }
            paths(children, path, out);
            if pushed {
                path.pop();
            }
        } else {
            if let (Some(id), false) = (item.get("Id").and_then(Value::as_u64), path.is_empty()) {
                out.insert(id, path.join(" > "));
            }
            paths(children, path, out);
        }
    }
}

fn text<'a>(node: &'a Value, field: &str) -> Option<&'a str> {
    node.get(field).and_then(Value::as_str)
}

fn day(node: &Value, field: &str) -> Option<String> {
    text(node, field).map(|s| s.chars().take(10).collect())
}

/// The number a version is keyed by (`10.1.1`, `Bijlage III`), none for a preamble.
fn number(kind: Option<&str>, num: Option<&str>) -> Option<String> {
    let num = num?.trim().trim_end_matches('.').trim();
    if num.is_empty() {
        return None;
    }
    Some(match kind.map(str::trim) {
        Some(k) if k.to_uppercase().starts_with("BIJLAGE") => format!("Bijlage {num}"),
        _ => num.to_string(),
    })
}

/// The act and its numbered article versions in one state, those it holds for later included.
pub fn codex_state(payload: &str) -> Result<CodexState, String> {
    let answer: Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    let document = answer
        .get("Document")
        .ok_or("the answer carries no document")?;
    let mut by_version = HashMap::new();
    if let Some(items) = answer
        .pointer("/Inhoudstafel/Items")
        .and_then(Value::as_array)
    {
        paths(items, &mut Vec::new(), &mut by_version);
    }
    let list = |pointer: &str| {
        answer
            .pointer(pointer)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let versions = list("/Inhoud/ArtikelVersies")
        .iter()
        .filter_map(|w| w.get("ArtikelVersie"))
        .chain(list("/Inhoud/ToekomstigeArtikelVersies"))
        .filter_map(|v| {
            let num = number(text(v, "ArtikelType"), text(v, "ArtikelNummer"))?;
            let id = v.get("Id").and_then(Value::as_u64)?;
            Some(CodexVersion {
                id,
                title_path: by_version.get(&id).cloned(),
                texte: plain(text(v, "Tekst").unwrap_or_default()),
                start: day(v, "StartDatum"),
                end: day(v, "EindDatum"),
                num,
            })
        })
        .collect();
    let numac = match document.get("Numac") {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };
    Ok(CodexState {
        numac,
        kind: text(document, "WetgevingDocumentType")
            .ok_or("the document names no type")?
            .to_string(),
        title: plain(text(document, "Opschrift").unwrap_or_default()),
        adopted: day(document, "Datum"),
        published: day(document, "BSDatum"),
        versions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_reads_its_number_heading_and_text_without_amendment_notes() {
        let payload = serde_json::json!({
            "Document": {
                "Numac": 1995035716,
                "WetgevingDocumentType": "Decreet",
                "Opschrift": "houdende algemene bepalingen inzake milieubeleid",
                "Datum": "1995-04-05T00:00:00",
            },
            "Inhoudstafel": {"Items": [{
                "ItemType": "H",
                "Titel": "[TITEL III BEDRIJFSINTERNE MILIEUZORG (ing. decr. 19 april 1995, art. 2)}]",
                "Items": [{"ItemType": "A", "Titel": "3.1.1.", "Id": 7, "Items": [
                    {"ItemType": "AV", "Titel": "3.1.1. (01/01/2019 - ...)", "Id": 8}
                ]}],
            }]},
            "Inhoud": {
                "ArtikelVersies": [
                    {"ArtikelVersie": {
                        "Id": 8,
                        "Tekst": "  <P>§ 1. Het [sediment (ing. decr. 18 juli 2003, art. 75, I: 24 november 2003) ] of de bodem;  <span class=\"reference\">(ing. decr. 7 mei 2004, art. 5)</span> ]<BR><BR>2° ...;</P>",
                        "StartDatum": "2019-01-01T00:00:00",
                        "EindDatum": null,
                        "ArtikelType": "ART.",
                        "ArtikelNummer": "3.1.1.",
                    }},
                    {"ArtikelVersie": {
                        "Id": 9, "Tekst": "De Vlaamse Regering,<br />Gelet op", "StartDatum": null,
                        "EindDatum": null, "ArtikelType": "", "ArtikelNummer": "",
                    }},
                ],
                "ToekomstigeArtikelVersies": [{
                    "Id": 10, "Tekst": "Lijst", "StartDatum": "2027-01-01T00:00:00",
                    "EindDatum": null, "ArtikelType": "BIJLAGE", "ArtikelNummer": "III.",
                }],
            },
        })
        .to_string();
        let state = codex_state(&payload).unwrap();
        assert_eq!(state.numac.as_deref(), Some("1995035716"));
        assert_eq!(state.adopted.as_deref(), Some("1995-04-05"));
        assert_eq!(state.versions.len(), 2);
        let v = &state.versions[0];
        assert_eq!(v.num, "3.1.1");
        assert_eq!(
            v.title_path.as_deref(),
            Some("TITEL III BEDRIJFSINTERNE MILIEUZORG")
        );
        assert_eq!(v.texte, "§ 1. Het [sediment ] of de bodem; ]\n2° ...;");
        assert_eq!(v.start.as_deref(), Some("2019-01-01"));
        assert_eq!(state.versions[1].num, "Bijlage III");
        assert_eq!(state.versions[1].start.as_deref(), Some("2027-01-01"));
    }
}
