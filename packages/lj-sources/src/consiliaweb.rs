//! ConsiliaWeb, the Conseil d'État's opinions: the census newest first, a file, through the egress proxy.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://www.conseil-etat.fr";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(1);
const ATTEMPTS: u32 = 5;

/// The census fields an opinion keeps, the engine's entity offsets left out.
const KEPT: &[&str] = &[
    "Id",
    "FileName",
    "FileExt",
    "IndexationTime",
    "SourceCsv1",
    "SourceCsv3",
    "SourceStr7",
    "SourceStr11",
    "SourceStr40",
    "SourceStr45",
    "SourceStr46",
    "SourceDateTime1",
    "SourceDateTime2",
];

/// A page of the census, 20 opinions.
#[derive(Debug, Deserialize)]
pub struct CensusPage {
    #[serde(rename = "PageCount")]
    pub page_count: u32,
    #[serde(rename = "Documents")]
    pub documents: Vec<Value>,
}

pub struct ConsiliaClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl ConsiliaClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "ConsiliaWeb is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    /// Sends `url` at the pace, again after a network failure or a 5xx, each wait one pace longer.
    async fn get(&self, url: &str) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        for attempt in 1.. {
            if let Some(at) = *last {
                tokio::time::sleep((PACE * attempt).saturating_sub(at.elapsed())).await;
            }
            let sent = self.http.get(url).send().await;
            *last = Some(Instant::now());
            match sent.and_then(reqwest::Response::error_for_status) {
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect()
                            || e.is_timeout()
                            || e.is_request()
                            || e.status().is_some_and(|s| s.is_server_error())) =>
                {
                    tracing::warn!(attempt, error = %e, "consiliaweb: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    /// Page `page` of the census, newest first, each opinion as [`KEPT`] keeps it.
    pub async fn census_page(&self, page: u32) -> Result<CensusPage> {
        let url = format!(
            "{BASE_URL}/xsearch?type=json&SourceStr4=CW&sort=SourceDateTime1.desc&PageNumber={page}"
        );
        let mut census: CensusPage = self.get(&url).await?.json().await?;
        for hit in &mut census.documents {
            if let Value::Object(fields) = hit {
                let kept: Map<String, Value> = KEPT
                    .iter()
                    .filter_map(|k| fields.remove(*k).map(|v| (k.to_string(), v)))
                    .collect();
                *fields = kept;
            }
        }
        Ok(census)
    }

    /// The file of the opinion whose census id is `id`, as ConsiliaWeb serves it.
    pub async fn file(&self, id: &str) -> Result<Vec<u8>> {
        Ok(self.get(&file_url(id)).await?.bytes().await?.to_vec())
    }
}

/// The address of the file of the opinion whose census id is `id`, `/Consilia_Web/CW/|112718`.
pub fn file_url(id: &str) -> String {
    format!("{BASE_URL}/plugin?plugin=Service.downloadFilePagePlugin&Index=Consilia_Web&Id={id}")
}
