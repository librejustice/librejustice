//! HAL source (`api.archives-ouvertes.fr`) : public Solr search API, no key.
//!
//! The User-Agent is declared honestly: presenting a browser string triggers
//! the anti-scraping wall that sits in front of the web host.

use crate::error::{Result, SourceError};
use lj_parse::HalDeposit;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.archives-ouvertes.fr/search/";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const THROTTLE: Duration = Duration::from_millis(1100);

/// Law deposits per page.
pub const PAGE: usize = 1_000;

/// Opening value of `cursorMark`, Solr's deep-paging token.
pub const CURSOR_START: &str = "*";

const FIELDS: &str = "halId_s,title_s,subTitle_s,abstract_s,keyword_s,authFullName_s,\
                      journalTitle_s,volume_s,issue_s,page_s,producedDateY_i,uri_s,\
                      submitType_s,domain_s,publisherLink_s";

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("reqwest client build")
}

/// The client of a job whose every request, files included, leaves through `proxy`.
pub fn http_client_via(proxy: &str) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .proxy(reqwest::Proxy::all(proxy)?)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(240))
        .build()?)
}

/// A file above this size is a scanned book or a corpus annex.
pub const MAX_FILE_BYTES: usize = 80_000_000;

/// A French law deposit with a file, and where HAL serves it.
#[derive(Debug, Clone)]
pub struct HalFile {
    pub hal_id: String,
    pub url: String,
}

pub struct HalFilePage {
    pub files: Vec<HalFile>,
    pub next_cursor: String,
    pub total: usize,
}

/// The French law deposits with a file, sound and pictures out; `None` takes the whole fonds.
pub async fn scan_files(
    client: &reqwest::Client,
    window_days: Option<u32>,
    cursor: &str,
) -> Result<HalFilePage> {
    let mut query = "domain_s:1.shs.droit AND submitType_s:file AND language_s:fr \
                     AND -docType_s:(VIDEO OR SON OR IMG OR MAP OR SOFTWARE OR POSTER)"
        .to_string();
    if let Some(d) = window_days {
        query.push_str(&format!(" AND submittedDate_tdate:[NOW-{d}DAYS TO NOW]"));
    }
    let rows = PAGE.to_string();
    let url = url::Url::parse_with_params(
        BASE_URL,
        &[
            ("q", query.as_str()),
            ("wt", "json"),
            ("rows", rows.as_str()),
            ("sort", "docid asc"),
            ("cursorMark", cursor),
            ("fl", "halId_s,fileMain_s"),
        ],
    )
    .map_err(|e| SourceError::Invalid(format!("hal url: {e}")))?;
    let resp = client.get(url).send().await?;
    if resp.status().as_u16() != 200 {
        return Err(SourceError::Invalid(format!(
            "hal: status {}",
            resp.status()
        )));
    }
    let body: serde_json::Value = sonic_rs::from_str(&resp.text().await?)?;
    let docs = body["response"]["docs"]
        .as_array()
        .ok_or_else(|| SourceError::Invalid("hal: answer without docs".to_string()))?;
    Ok(HalFilePage {
        files: docs
            .iter()
            .filter_map(|d| {
                Some(HalFile {
                    hal_id: first_str(&d["halId_s"])?,
                    url: first_str(&d["fileMain_s"])?,
                })
            })
            .collect(),
        next_cursor: body["nextCursorMark"]
            .as_str()
            .ok_or_else(|| SourceError::Invalid("hal: answer without nextCursorMark".to_string()))?
            .to_string(),
        total: body["response"]["numFound"].as_u64().unwrap_or(0) as usize,
    })
}

/// A deposit's main file, `None` when HAL serves no PDF under [`MAX_FILE_BYTES`], a refusal
/// a retry would meet again; an error is the network's or the server's.
pub async fn fetch_file(client: &reqwest::Client, url: &str) -> Result<Option<Vec<u8>>> {
    let mut resp = client.get(url).send().await?;
    let status = resp.status();
    if status.as_u16() == 429 || status.is_server_error() {
        return Err(SourceError::Invalid(format!("hal file: status {status}")));
    }
    if !status.is_success() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_FILE_BYTES {
            return Ok(None);
        }
    }
    Ok(bytes.starts_with(b"%PDF-").then_some(bytes))
}

/// One page of law deposits and the token that opens the next one.
pub struct HalPage {
    pub deposits: Vec<HalDeposit>,
    pub next_cursor: String,
    pub total: usize,
}

/// Scans the law fonds, oldest `docid` first, `window_days` bounding the
/// submission date; `None` takes the whole fonds.
///
/// `submittedDate_tdate` and not `modifiedDate_tdate`: HAL re-touches its
/// notices constantly, far more often than it receives new ones.
pub async fn scan_page(
    client: &reqwest::Client,
    window_days: Option<u32>,
    cursor: &str,
) -> Result<HalPage> {
    let query = match window_days {
        Some(d) => format!("domain_s:1.shs.droit AND submittedDate_tdate:[NOW-{d}DAYS TO NOW]"),
        None => "domain_s:1.shs.droit".to_string(),
    };
    let rows = PAGE.to_string();
    let url = url::Url::parse_with_params(
        BASE_URL,
        &[
            ("q", query.as_str()),
            ("wt", "json"),
            ("rows", rows.as_str()),
            ("sort", "docid asc"),
            ("cursorMark", cursor),
            ("fl", FIELDS),
        ],
    )
    .map_err(|e| SourceError::Invalid(format!("url hal: {e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| SourceError::Invalid(format!("requête hal: {e}")))?;
    if resp.status().as_u16() != 200 {
        return Err(SourceError::Invalid(format!(
            "hal: statut {}",
            resp.status()
        )));
    }
    let text = resp.text().await?;
    let body: serde_json::Value = sonic_rs::from_str(&text)?;
    let docs = body["response"]["docs"]
        .as_array()
        .ok_or_else(|| SourceError::Invalid("hal: réponse sans docs".to_string()))?;
    let next_cursor = body["nextCursorMark"]
        .as_str()
        .ok_or_else(|| SourceError::Invalid("hal: réponse sans nextCursorMark".to_string()))?
        .to_string();
    Ok(HalPage {
        deposits: docs.iter().map(deposit_from_doc).collect(),
        next_cursor,
        total: body["response"]["numFound"].as_u64().unwrap_or(0) as usize,
    })
}

fn first_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Array(a) => a.first().and_then(|x| x.as_str()).map(str::to_string),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn all_str(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(str::to_string)
            .collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn deposit_from_doc(d: &serde_json::Value) -> HalDeposit {
    HalDeposit {
        hal_id: first_str(&d["halId_s"]).unwrap_or_default(),
        title: first_str(&d["title_s"]).unwrap_or_default(),
        subtitle: first_str(&d["subTitle_s"]),
        abstract_text: first_str(&d["abstract_s"]),
        keywords: all_str(&d["keyword_s"]),
        authors: all_str(&d["authFullName_s"]),
        journal: first_str(&d["journalTitle_s"]),
        volume: first_str(&d["volume_s"]),
        issue: first_str(&d["issue_s"]),
        pages: first_str(&d["page_s"]),
        year: d["producedDateY_i"].as_i64().map(|y| y as i32),
        uri: first_str(&d["uri_s"]).unwrap_or_default(),
        submit_type: first_str(&d["submitType_s"]).unwrap_or_default(),
        domains: all_str(&d["domain_s"]),
        publisher_links: all_str(&d["publisherLink_s"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_maps_singletons_and_arrays() {
        let doc = serde_json::json!({
            "halId_s": "hal-04001730",
            "title_s": ["Note sous Civ. 1re"],
            "subTitle_s": ["n° 17-17.319"],
            "abstract_s": "Observations",
            "keyword_s": ["responsabilité", "faute"],
            "authFullName_s": ["P. Jourdain"],
            "journalTitle_s": ["RTDCiv"],
            "volume_s": "vol. 134",
            "issue_s": ["n° 2"],
            "page_s": "p. 407-422",
            "producedDateY_i": 2018,
            "uri_s": "https://hal.science/hal-04001730",
            "submitType_s": "notice",
            "domain_s": ["1.shs", "1.shs.droit"],
        });
        let d = deposit_from_doc(&doc);
        assert_eq!(d.hal_id, "hal-04001730");
        assert_eq!(d.title, "Note sous Civ. 1re");
        assert_eq!(d.subtitle.as_deref(), Some("n° 17-17.319"));
        assert_eq!(d.abstract_text.as_deref(), Some("Observations"));
        assert_eq!(d.keywords.len(), 2);
        assert_eq!(d.volume.as_deref(), Some("vol. 134"));
        assert_eq!(d.issue.as_deref(), Some("n° 2"));
        assert_eq!(d.pages.as_deref(), Some("p. 407-422"));
        assert_eq!(d.year, Some(2018));
        assert!(d.is_law());
    }

    #[test]
    fn missing_fields_stay_empty() {
        let d = deposit_from_doc(&serde_json::json!({"halId_s": "hal-1"}));
        assert_eq!(d.hal_id, "hal-1");
        assert!(d.title.is_empty());
        assert!(d.subtitle.is_none());
        assert!(d.keywords.is_empty());
        assert!(!d.is_law());
    }
}
