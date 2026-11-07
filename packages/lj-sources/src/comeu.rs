//! The European Commission's competition case search: its search API lists decisions and documents.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const SEARCH_URL: &str = "https://webgate.ec.europa.eu/es/search-api/rest/search";
/// The search API's public key.
const API_KEY: &str = "CS_PROD_ODSE_PROD";
pub const DOCUMENT_BASE: &str = "https://ec.europa.eu/competition/";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(1);
/// The instruments of the search, as its `caseInstrument` names them.
pub const INSTRUMENTS: [&str; 5] = ["AT", "M", "SA", "InstrumentDMA", "InstrumentFS"];
pub const DECISION: &str = "METADATA_DECISION";
pub const ATTACHMENT: &str = "METADATA_DECISION_ATTACHMENT";
const PAGE_SIZE: usize = 100;
/// Past this many results a query answers empty pages.
const QUERY_CAP: usize = 10_000;
const FIRST_YEAR: i32 = 1950;

/// One record of the search: its reference and its metadata, the search's own fields left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub reference: String,
    pub metadata: Map<String, Value>,
}

impl Record {
    /// The first value of a metadata field.
    pub fn first(&self, field: &str) -> Option<&str> {
        self.metadata.get(field)?.as_array()?.first()?.as_str()
    }
}

pub struct ComeuClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl ComeuClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
        }
        let response = request.send().await;
        *last = Some(Instant::now());
        Ok(response?)
    }

    async fn page(&self, query: &Value, page: usize) -> Result<Value> {
        let boundary = "ljcomeu7d0f3a9c1b2e";
        let part = |name: &str, value: &Value| {
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"blob\"\r\n\
                 Content-Type: application/json\r\n\r\n{value}\r\n"
            )
        };
        let body = format!(
            "{}{}--{boundary}--\r\n",
            part("query", query),
            part(
                "sort",
                &json!([{"field": "metadataReference", "order": "ASC"}])
            )
        );
        let page = page.to_string();
        let size = PAGE_SIZE.to_string();
        let mut tries = 0;
        loop {
            let request = self
                .http
                .post(SEARCH_URL)
                .query(&[
                    ("apiKey", API_KEY),
                    ("text", "*"),
                    ("pageNumber", page.as_str()),
                    ("pageSize", size.as_str()),
                ])
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body.clone());
            match self
                .send(request)
                .await
                .and_then(|r| Ok(r.error_for_status()?))
            {
                Ok(response) => return Ok(sonic_rs::from_str(&response.text().await?)?),
                Err(error) if tries < 3 => {
                    tries += 1;
                    tracing::warn!(%error, tries, "comeu search failed, asked again");
                    tokio::time::sleep(Duration::from_secs(5 * tries)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Every record `query` matches, `None` past [`QUERY_CAP`]. The index pages a query being
    /// reindexed with records repeated and others left out, so a short reading is asked again.
    async fn all(&self, query: &Value) -> Result<Option<Vec<Record>>> {
        let mut tries = 0;
        loop {
            let first = self.page(query, 1).await?;
            let total = first["totalResults"].as_u64().unwrap_or(0) as usize;
            if total > QUERY_CAP {
                return Ok(None);
            }
            let mut records = records_of(&first);
            for page in 2..=total.div_ceil(PAGE_SIZE) {
                records.extend(records_of(&self.page(query, page).await?));
            }
            let mut seen = HashSet::new();
            records.retain(|r| seen.insert(r.reference.clone()));
            if records.len() >= total {
                return Ok(Some(records));
            }
            if tries == 2 {
                return Err(SourceError::Invalid(format!(
                    "comeu search: {} distinct records of {total}",
                    records.len()
                )));
            }
            tries += 1;
            tracing::warn!(
                read = records.len(),
                total,
                "comeu search paged short, asked again"
            );
        }
    }

    async fn capped(
        &self,
        query: &Value,
        scope: &str,
        kind: &str,
        part: &str,
    ) -> Result<Vec<Record>> {
        self.all(query).await?.ok_or_else(|| {
            SourceError::Invalid(format!(
                "comeu {scope} {kind} {part}: over {QUERY_CAP} records"
            ))
        })
    }

    /// Every record of `kind` of an instrument, year by year when one query cannot hold them.
    pub async fn list(&self, instrument: &str, kind: &str, last_year: i32) -> Result<Vec<Record>> {
        let base = [
            term("metadataType", kind),
            term("caseInstrument", instrument),
        ];
        if let Some(records) = self.all(&must(&base)).await? {
            return Ok(records);
        }
        let field = date_field(kind);
        let undated = json!({"bool": {"must": base, "must_not": [{"exists": {"field": field}}]}});
        let mut out = self.capped(&undated, instrument, kind, "undated").await?;
        for year in FIRST_YEAR..=last_year {
            let range = json!({"range": {field: {
                "gte": format!("{year}-01-01T00:00:00.000Z"),
                "lt": format!("{}-01-01T00:00:00.000Z", year + 1),
            }}});
            let query = must(&[base[0].clone(), base[1].clone(), range]);
            out.extend(
                self.capped(&query, instrument, kind, &year.to_string())
                    .await?,
            );
        }
        Ok(out)
    }

    /// The attachments of an instrument published since `since` (`YYYY-MM-DD`).
    pub async fn attachments_since(&self, instrument: &str, since: &str) -> Result<Vec<Record>> {
        let query = must(&[
            term("metadataType", ATTACHMENT),
            term("caseInstrument", instrument),
            json!({"range": {"attachmentPublicationBusinessDate": {"gte": format!("{since}T00:00:00.000Z")}}}),
        ]);
        self.capped(&query, instrument, ATTACHMENT, since).await
    }

    /// The decisions and the decision documents of a case.
    pub async fn of_case(&self, case_number: &str) -> Result<Vec<Record>> {
        let query = must(&[
            json!({"terms": {"metadataType": [DECISION, ATTACHMENT]}}),
            term("caseNumber", case_number),
        ]);
        self.capped(&query, case_number, "case", "records").await
    }

    /// The PDF of an attachment link, `None` when the Commission answers 404.
    pub async fn pdf(&self, instrument: &str, link: &str) -> Result<Option<Vec<u8>>> {
        let url = document_url(instrument, link)
            .ok_or_else(|| SourceError::Invalid(format!("comeu instrument {instrument}")))?;
        let response = self.send(self.http.get(&url)).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.bytes().await?.to_vec()))
    }
}

/// The URL of an attachment link: the front resolves it under its instrument's folder.
pub fn document_url(instrument: &str, link: &str) -> Option<String> {
    let folder = match instrument {
        "AT" => "antitrust",
        "M" => "mergers",
        "SA" => "state_aid",
        "InstrumentDMA" => "digital_markets_act",
        "InstrumentFS" => "foreign_subsidies",
        _ => return None,
    };
    Some(format!(
        "{DOCUMENT_BASE}{folder}/{}",
        link.trim_start_matches('/')
    ))
}

fn date_field(kind: &str) -> &'static str {
    if kind == ATTACHMENT {
        "attachmentPublicationBusinessDate"
    } else {
        "decisionAdoptionDate"
    }
}

fn term(field: &str, value: &str) -> Value {
    json!({"term": {field: value}})
}

fn must(clauses: &[Value]) -> Value {
    json!({"bool": {"must": clauses}})
}

/// The records of a search page, without the fields the search changes at each indexing.
pub fn records_of(page: &Value) -> Vec<Record> {
    const OWN: [&str; 4] = [
        "corporate-search-version",
        "datasource",
        "metadataEnvironment",
        "url",
    ];
    page["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let reference = r["reference"].as_str()?.to_string();
            let metadata = r["metadata"]
                .as_object()?
                .iter()
                .filter(|(k, _)| !k.starts_with("es") && !OWN.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Some(Record {
                reference,
                metadata,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_keeps_its_metadata_without_the_search_s_own_fields() {
        let page = json!({"totalResults": 1, "results": [{
            "reference": "AT.39839-DEC{628AC6BD}",
            "metadata": {
                "caseNumber": ["AT.39839"],
                "esDA_IngestDate": ["2025-01-01T00:00:00.000+0100"],
                "esST_checksum": ["x"],
                "corporate-search-version": ["2"],
                "url": ["https://x"],
                "decisionTypes": ["ATDecTypeIOP"],
            },
        }]});
        let records = records_of(&page);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reference, "AT.39839-DEC{628AC6BD}");
        let mut keys: Vec<&str> = records[0].metadata.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["caseNumber", "decisionTypes"]);
        assert_eq!(records[0].first("caseNumber"), Some("AT.39839"));
    }
}
