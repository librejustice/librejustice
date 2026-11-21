//! Légimonaco, Monaco's official legal portal: the decisions its public Elasticsearch serves, text
//! included, read in pages sorted by the day the portal deployed them.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;

use crate::error::{Result, SourceError};

pub const SEARCH_URL: &str = "https://legimonaco.mc/~~search/depot/_search";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// 200 decisions weigh about 5 MB and come back in 2 s.
pub const PAGE: usize = 200;
/// Read from HUDOC, the Court's own source.
const ECHR: &str = "cour-europeenne-droits-homme";

/// A decision's place in the walk: when the portal deployed it, then its content id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub deployed: i64,
    pub content_id: String,
}

/// A decision of the index: its `_source`, and the bodies of its sections titled `Abstract` and
/// `Résumé`, which open its `enBody`.
pub struct Hit {
    pub key: Key,
    pub source: Box<RawValue>,
    pub abstract_body: Option<String>,
    pub summary_body: Option<String>,
}

#[derive(Deserialize)]
struct Answer<T> {
    hits: Hits<T>,
}

#[derive(Deserialize)]
struct Hits<T> {
    hits: Vec<T>,
}

#[derive(Deserialize)]
struct RawHit {
    #[serde(rename = "_source")]
    source: Box<RawValue>,
    sort: (i64, String),
    #[serde(default)]
    inner_hits: Option<InnerHits>,
}

#[derive(Deserialize)]
struct InnerHits {
    entry: Answer<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "_source")]
    source: EntrySource,
}

#[derive(Deserialize)]
struct EntrySource {
    #[serde(rename = "enTitle")]
    title: Option<String>,
    #[serde(rename = "enBody", default)]
    body: String,
}

pub struct LegimonacoClient {
    http: reqwest::Client,
}

impl LegimonacoClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "legimonaco.mc is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { http })
    }

    /// At most `size` decisions deployed after `after`, the European Court's left out.
    pub async fn page(&self, after: Option<&Key>, size: usize) -> Result<Vec<Hit>> {
        let mut body = json!({
            "size": size,
            "query": {"bool": {
                "filter": [{"term": {"type": "case"}}],
                "must_not": [{"term": {"jurisdiction": ECHR}}],
                "should": [{"has_child": {
                    "type": "entry",
                    "score_mode": "none",
                    "query": {"bool": {"should": [
                        {"match_phrase": {"enTitle": "Abstract"}},
                        {"match_phrase": {"enTitle": "Résumé"}},
                    ]}},
                    "inner_hits": {"size": 10, "_source": ["enTitle", "enBody"]},
                }}],
            }},
            "sort": [{"deployDate": "asc"}, {"contentId": "asc"}],
        });
        if let Some(key) = after {
            body["search_after"] = json!([key.deployed, key.content_id]);
        }
        let response = self.http.post(SEARCH_URL).json(&body).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!("legimonaco answers {status}")));
        }
        let bytes = response.bytes().await?;
        let answer: Answer<RawHit> = serde_json::from_slice(&bytes).map_err(|e| {
            SourceError::Invalid(format!(
                "legimonaco page of {} bytes after {after:?}: {e}",
                bytes.len()
            ))
        })?;
        Ok(answer.hits.hits.into_iter().map(hit).collect())
    }
}

fn hit(raw: RawHit) -> Hit {
    let mut abstract_body = None;
    let mut summary_body = None;
    for entry in raw.inner_hits.into_iter().flat_map(|i| i.entry.hits.hits) {
        let slot = match entry.source.title.as_deref() {
            Some("Abstract") => &mut abstract_body,
            Some("Résumé") => &mut summary_body,
            _ => continue,
        };
        slot.get_or_insert(entry.source.body);
    }
    Hit {
        key: Key {
            deployed: raw.sort.0,
            content_id: raw.sort.1,
        },
        source: raw.source,
        abstract_body,
        summary_body,
    }
}
