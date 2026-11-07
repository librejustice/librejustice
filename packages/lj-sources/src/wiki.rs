//! French Wikipedia and Wikidata through their public APIs.

use std::collections::{BTreeSet, VecDeque};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::error::{Result, SourceError};

pub const USER_AGENT: &str = "librejustice-wiki-names/0.1 (+https://librejustice.fr)";
pub const THROTTLE: Duration = Duration::from_secs(2);
const WIKIPEDIA_API: &str = "https://fr.wikipedia.org/w/api.php";
const WIKIDATA_SPARQL: &str = "https://query.wikidata.org/sparql";
const TITLES_PER_REQUEST: usize = 40;
const CATEGORY_DEPTH: usize = 3;
const ATTEMPTS: u64 = 5;

/// A page with its wikitext, and the Wikidata item it is the page of.
#[derive(Debug, Clone)]
pub struct WikiPage {
    pub title: String,
    pub wikitext: String,
    pub wikidata: Option<String>,
}

/// A decision Wikidata identifies by its ECLI or by its Légifrance page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WikidataDecision {
    pub item: String,
    pub ecli: Option<String>,
    /// The Légifrance id of the decision, `JURITEXT…` or `CETATEXT…`.
    pub legifrance: Option<String>,
    pub label_fr: Option<String>,
    pub page_fr: Option<String>,
}

pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(SourceError::from)
}

/// The titles of the pages under a category and its subcategories.
pub async fn category_pages(client: &reqwest::Client, root: &str) -> Result<BTreeSet<String>> {
    let mut pages = BTreeSet::new();
    let mut seen = BTreeSet::from([root.to_string()]);
    let mut todo = VecDeque::from([(root.to_string(), 0usize)]);
    while let Some((category, depth)) = todo.pop_front() {
        let mut next: Option<String> = None;
        loop {
            let mut params = vec![
                ("list", "categorymembers".to_string()),
                ("cmtitle", category.clone()),
                ("cmtype", "page|subcat".to_string()),
                ("cmlimit", "500".to_string()),
            ];
            params.extend(next.take().map(|c| ("cmcontinue", c)));
            let body = wikipedia_query(client, params).await?;
            for member in body["query"]["categorymembers"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let title = member["title"].as_str().unwrap_or_default().to_string();
                match member["ns"].as_i64() {
                    Some(14) if depth < CATEGORY_DEPTH && seen.insert(title.clone()) => {
                        todo.push_back((title, depth + 1))
                    }
                    Some(0) => {
                        pages.insert(title);
                    }
                    _ => {}
                }
            }
            match body["continue"]["cmcontinue"].as_str() {
                Some(c) => next = Some(c.to_string()),
                None => break,
            }
        }
    }
    Ok(pages)
}

/// The pages behind these titles, redirects followed, each once; a missing title is absent.
pub async fn pages(client: &reqwest::Client, titles: &[String]) -> Result<Vec<WikiPage>> {
    let mut out = Vec::with_capacity(titles.len());
    let mut seen = BTreeSet::new();
    for batch in titles.chunks(TITLES_PER_REQUEST) {
        let params = vec![
            ("prop", "revisions|pageprops".to_string()),
            ("rvprop", "content".to_string()),
            ("rvslots", "main".to_string()),
            ("ppprop", "wikibase_item".to_string()),
            ("redirects", "1".to_string()),
            ("titles", batch.join("|")),
        ];
        let body = wikipedia_query(client, params).await?;
        for page in body["query"]["pages"].as_array().into_iter().flatten() {
            let Some(wikitext) = page["revisions"][0]["slots"]["main"]["content"].as_str() else {
                continue;
            };
            let title = page["title"].as_str().unwrap_or_default().to_string();
            if !seen.insert(title.clone()) {
                continue;
            }
            out.push(WikiPage {
                title,
                wikitext: wikitext.to_string(),
                wikidata: page["pageprops"]["wikibase_item"]
                    .as_str()
                    .map(str::to_string),
            });
        }
    }
    Ok(out)
}

/// One per item, the decisions under a French, EU or Council of Europe ECLI, or typed
/// as one with a Légifrance full text (P953 untyped is a scan WDQS cuts at 60 s).
pub async fn wikidata_decisions(client: &reqwest::Client) -> Result<Vec<WikidataDecision>> {
    let query = r#"SELECT ?item ?ecli ?url ?label ?page WHERE {
  { ?item wdt:P3570 ?ecli .
    FILTER(STRSTARTS(?ecli, "ECLI:FR:") || STRSTARTS(?ecli, "ECLI:EU:") || STRSTARTS(?ecli, "ECLI:CE:")) }
  UNION
  { VALUES ?type { wd:Q327000 wd:Q2334719 wd:Q8016240 wd:Q30328717 wd:Q2032782 }
    ?item wdt:P31 ?type ; wdt:P953 ?url .
    FILTER(CONTAINS(STR(?url), "legifrance.gouv.fr")) }
  OPTIONAL { ?item rdfs:label ?label FILTER(LANG(?label) = "fr") }
  OPTIONAL { ?link schema:about ?item ; schema:isPartOf <https://fr.wikipedia.org/> ; schema:name ?page }
}"#;
    let body = get_json(
        client,
        WIKIDATA_SPARQL,
        &[("query", query.to_string()), ("format", "json".to_string())],
    )
    .await?;
    Ok(wikidata_rows(&body))
}

/// The SPARQL rows folded by item, the first value of each field kept.
fn wikidata_rows(body: &Value) -> Vec<WikidataDecision> {
    static LEGIFRANCE_ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:JURITEXT|CETATEXT)\d{12}").expect("regex"));
    let text = |b: &Value, k: &str| b[k]["value"].as_str().map(str::to_string);
    let mut items: Vec<WikidataDecision> = Vec::new();
    for b in body["results"]["bindings"].as_array().into_iter().flatten() {
        let Some(item) = text(b, "item").and_then(|i| Some(i.rsplit('/').next()?.to_string()))
        else {
            continue;
        };
        let legifrance =
            text(b, "url").and_then(|u| Some(LEGIFRANCE_ID.find(&u)?.as_str().to_string()));
        let row = WikidataDecision {
            item,
            ecli: text(b, "ecli"),
            legifrance,
            label_fr: text(b, "label"),
            page_fr: text(b, "page"),
        };
        match items.iter_mut().find(|d| d.item == row.item) {
            Some(d) => {
                d.ecli = d.ecli.take().or(row.ecli);
                d.legifrance = d.legifrance.take().or(row.legifrance);
                d.label_fr = d.label_fr.take().or(row.label_fr);
                d.page_fr = d.page_fr.take().or(row.page_fr);
            }
            None => items.push(row),
        }
    }
    items
}

async fn wikipedia_query(
    client: &reqwest::Client,
    mut params: Vec<(&str, String)>,
) -> Result<Value> {
    params.extend([
        ("action", "query".to_string()),
        ("format", "json".to_string()),
        ("formatversion", "2".to_string()),
    ]);
    get_json(client, WIKIPEDIA_API, &params).await
}

/// One request after [`THROTTLE`], retried with a growing wait while throttled.
async fn get_json(client: &reqwest::Client, url: &str, params: &[(&str, String)]) -> Result<Value> {
    for attempt in 1..=ATTEMPTS {
        tokio::time::sleep(THROTTLE).await;
        let resp = client.get(url).query(params).send().await?;
        match resp.status().as_u16() {
            200 => return Ok(resp.json().await?),
            429 | 503 => tokio::time::sleep(Duration::from_secs(30 * attempt)).await,
            status => return Err(SourceError::Invalid(format!("{url}: status {status}"))),
        }
    }
    Err(SourceError::Invalid(format!(
        "{url}: still throttled after {ATTEMPTS} attempts"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_item_is_one_decision_its_ecli_and_legifrance_id_folded() {
        let body = json!({"results": {"bindings": [
            {"item": {"value": "http://www.wikidata.org/entity/Q1"},
             "ecli": {"value": "ECLI:FR:CCASS:2010:CO00732"},
             "label": {"value": "arrêt Faurecia"}},
            {"item": {"value": "http://www.wikidata.org/entity/Q1"},
             "url": {"value": "https://www.legifrance.gouv.fr/juri/id/JURITEXT000022427254?init=true"}},
            {"item": {"value": "http://www.wikidata.org/entity/Q2"},
             "url": {"value": "https://www.legifrance.gouv.fr/affichJuriAdmin.do?idTexte=CETATEXT000038351092"}},
        ]}});
        assert_eq!(
            wikidata_rows(&body),
            [
                WikidataDecision {
                    item: "Q1".into(),
                    ecli: Some("ECLI:FR:CCASS:2010:CO00732".into()),
                    legifrance: Some("JURITEXT000022427254".into()),
                    label_fr: Some("arrêt Faurecia".into()),
                    page_fr: None,
                },
                WikidataDecision {
                    item: "Q2".into(),
                    legifrance: Some("CETATEXT000038351092".into()),
                    ..Default::default()
                },
            ]
        );
    }
}
