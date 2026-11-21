//! entscheidsuche.ch: the Swiss decisions its public Elasticsearch serves, text included, read in
//! pages sorted by id, the labels of its courts and chambers, and the job files of its spiders.

use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};

use crate::error::{Result, SourceError};

pub const SEARCH_URL: &str = "https://entscheidsuche.ch/_search.php";
pub const DOCS_URL: &str = "https://entscheidsuche.ch/docs";
pub const FACETS_URL: &str = "https://entscheidsuche.ch/docs/Facetten.json";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// 200 documents weigh about 15 MB and come back in 5 s; 500 come back empty.
pub const PAGE: usize = 200;
/// Ids and scrape days only: 5,000 a page, 1.5 MB in 10 s.
pub const LISTING_PAGE: usize = 5_000;
/// The index keeps the first 100,000 characters of a document's text, and says so in its
/// `content_length`.
pub const TEXT_CAP: u64 = 100_000;

/// A document of the index, its `_source` as the index serves it.
pub struct Hit {
    pub id: String,
    pub source: Box<RawValue>,
}

/// A document of the index by its id, the day its spider last scraped it, and whether the index
/// cut its text.
pub struct Listed {
    pub id: String,
    pub scrapedate: Option<String>,
    pub cut: bool,
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
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_source")]
    source: Box<RawValue>,
}

#[derive(Deserialize)]
struct ListedHit {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_source")]
    source: ListedSource,
}

#[derive(Deserialize)]
struct ListedSource {
    scrapedate: Option<String>,
    attachment: Option<ListedAttachment>,
}

#[derive(Deserialize)]
struct ListedAttachment {
    content_length: Option<u64>,
}

pub struct EntscheidsucheClient {
    http: reqwest::Client,
}

impl EntscheidsucheClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "entscheidsuche.ch is read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
                    .into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self { http })
    }

    async fn search<T: for<'de> Deserialize<'de>>(&self, body: &Value) -> Result<Vec<T>> {
        let response = self.http.post(SEARCH_URL).json(body).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "entscheidsuche answers {status}"
            )));
        }
        let bytes = response.bytes().await?;
        let answer: Answer<T> = serde_json::from_slice(&bytes).map_err(|e| {
            SourceError::Invalid(format!(
                "entscheidsuche page of {} bytes after {:?}: {e}",
                bytes.len(),
                body.get("search_after")
            ))
        })?;
        Ok(answer.hits.hits)
    }

    /// The documents of `courts` after `after` by id, scraped on `since` or later.
    pub async fn page(
        &self,
        courts: &[&str],
        after: Option<&str>,
        since: Option<&str>,
    ) -> Result<Vec<Hit>> {
        let mut filter = vec![json!({"terms": {"hierarchy": courts}})];
        if let Some(since) = since {
            filter.push(json!({"range": {"scrapedate": {"gte": since}}}));
        }
        let mut body = json!({
            "size": PAGE,
            "query": {"bool": {"filter": filter}},
            "sort": [{"id": "asc"}],
        });
        if let Some(after) = after {
            body["search_after"] = json!([after]);
        }
        Ok(self
            .search::<RawHit>(&body)
            .await?
            .into_iter()
            .map(|h| Hit {
                id: h.id,
                source: h.source,
            })
            .collect())
    }

    /// The ids of `courts` after `after`, with their scrape day.
    pub async fn listing(&self, courts: &[&str], after: Option<&str>) -> Result<Vec<Listed>> {
        let mut body = json!({
            "size": LISTING_PAGE,
            "_source": ["scrapedate", "attachment.content_length"],
            "query": {"bool": {"filter": [{"terms": {"hierarchy": courts}}]}},
            "sort": [{"id": "asc"}],
        });
        if let Some(after) = after {
            body["search_after"] = json!([after]);
        }
        Ok(self
            .search::<ListedHit>(&body)
            .await?
            .into_iter()
            .map(|h| Listed {
                id: h.id,
                scrapedate: h.source.scrapedate,
                cut: h
                    .source
                    .attachment
                    .and_then(|a| a.content_length)
                    .is_some_and(|l| l >= TEXT_CAP),
            })
            .collect())
    }

    /// The documents of these ids, at most [`PAGE`].
    pub async fn by_ids(&self, ids: &[&str]) -> Result<Vec<Hit>> {
        let body = json!({
            "size": PAGE,
            "query": {"ids": {"values": ids}},
            "sort": [{"id": "asc"}],
        });
        Ok(self
            .search::<RawHit>(&body)
            .await?
            .into_iter()
            .map(|h| Hit {
                id: h.id,
                source: h.source,
            })
            .collect())
    }

    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.http.get(url).send().await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(SourceError::Invalid(format!("{url} answers {status}")));
        }
        Ok(response.bytes().await?.to_vec())
    }

    /// The file the index extracted a document's text from, `content_url` as the index names it.
    pub async fn original(&self, content_url: &str) -> Result<Vec<u8>> {
        if !content_url.starts_with(DOCS_URL) {
            return Err(SourceError::Invalid(format!(
                "entscheidsuche original outside {DOCS_URL}: {content_url}"
            )));
        }
        self.get(content_url).await
    }

    /// Every canton of the index, its courts under `gerichte`, their chambers under `kammern`.
    pub async fn facets(&self) -> Result<Value> {
        let bytes = self.get(FACETS_URL).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| SourceError::Invalid(format!("entscheidsuche facets: {e}")))
    }

    /// A spider's job file, `Job_<start>_<id>.json`: every file the run holds, its checksum
    /// and status.
    pub async fn job(&self, spider: &str, name: &str) -> Result<Value> {
        let bytes = self
            .get(&format!("{DOCS_URL}/Jobs/{spider}/{name}"))
            .await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| SourceError::Invalid(format!("entscheidsuche job {spider}/{name}: {e}")))
    }
}
