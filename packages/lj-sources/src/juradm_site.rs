//! The « dernières décisions » of the sites of the administrative courts: listing pages, an item's
//! page, the files it links. The cache holds `items.jsonl`, the last line of a URL winning.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lj_parse::JuradmCourt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

const LISTING: &str = "/decisions-de-justice/dernieres-decisions";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// One host serves every site.
pub const PACE: Duration = Duration::from_secs(1);
const ATTEMPTS: u32 = 5;

/// `cergy-pontoise.tribunal-administratif.fr` for `ta_cergy_pontoise`.
pub fn host(court: &JuradmCourt) -> String {
    let (kind, place) = court
        .code
        .split_once('_')
        .expect("a court code has a prefix");
    let domain = match kind {
        "caa" => "cour-administrative-appel.fr",
        _ => "tribunal-administratif.fr",
    };
    format!("{}.{domain}", place.replace('_', "-"))
}

/// An item as a listing page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub url: String,
    pub title: String,
    pub date: Option<String>,
}

/// An item read: its page, and every file it links, with where the file sits in the bucket when kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRead {
    pub court: String,
    pub url: String,
    pub title: String,
    pub date: Option<String>,
    pub files: Vec<FileRead>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRead {
    pub url: String,
    pub key: Option<String>,
}

pub enum Fetched {
    /// The bytes, and the address the redirects end at.
    Served { url: String, bytes: Vec<u8> },
    /// A 404: asking again the next day gets the same answer.
    Gone,
    /// A 5xx past every attempt.
    Failing,
}

pub struct JuradmClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl JuradmClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the court sites are read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
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

    /// Sends a GET of `url` at the sites' pace, again after a network failure or a 5xx, up to
    /// [`ATTEMPTS`] times, each wait one pace longer.
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
                    tracing::warn!(attempt, url, error = %e, "juradm-site: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    /// The items of listing page `page`, from 1; empty past the last page.
    pub async fn listing(&self, court: &JuradmCourt, page: u32) -> Result<Vec<Listed>> {
        let url = format!("https://{}{LISTING}?page={page}", host(court));
        match self.get(&url).await {
            Ok(response) => Ok(parse_listing(&response.text().await?, &host(court))),
            Err(SourceError::Http(e)) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                Ok(Vec::new())
            }
            Err(SourceError::Http(e)) if e.status().is_some_and(|s| s.is_server_error()) => {
                tracing::warn!(
                    url,
                    "juradm-site: a listing page is not served, the court's listing ends there"
                );
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    /// The files the item's page links, as absolute URLs, `None` when the page answers a 5xx past
    /// every attempt.
    pub async fn files(&self, court: &JuradmCourt, listed: &Listed) -> Result<Option<Vec<String>>> {
        match self.get(&listed.url).await {
            Ok(response) => Ok(Some(parse_files(&response.text().await?, &host(court)))),
            Err(SourceError::Http(e)) if e.status().is_some_and(|s| s.is_server_error()) => {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// The file at `url`, or why the site does not serve it.
    pub async fn file(&self, url: &str) -> Result<Fetched> {
        match self.get(url).await {
            Ok(response) => Ok(Fetched::Served {
                url: response.url().to_string(),
                bytes: response.bytes().await?.to_vec(),
            }),
            Err(SourceError::Http(e)) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                Ok(Fetched::Gone)
            }
            Err(SourceError::Http(e)) if e.status().is_some_and(|s| s.is_server_error()) => {
                Ok(Fetched::Failing)
            }
            Err(e) => Err(e),
        }
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    decode_entities(&regex(&TAG, r"(?s)<[^>]*>").replace_all(html, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_listing(html: &str, host: &str) -> Vec<Listed> {
    static BODY: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    html.split(r#"<li class="list__item"#)
        .skip(1)
        .filter_map(|block| {
            let path = regex(
                &BODY,
                r#"class="list__item__body"\s+href="(/decisions-de-justice/dernieres-decisions/[^"?#]+)""#,
            )
            .captures(block)?;
            Some(Listed {
                url: format!("https://{host}{}", &path[1]),
                title: regex(&TITLE, r#"(?s)class="list__item__title">(.*?)</p>"#)
                    .captures(block)
                    .map(|c| text(&c[1]))
                    .unwrap_or_default(),
                date: regex(&DATE, r#"class="list__item__date" datetime="(\d{4}-\d{2}-\d{2})""#)
                    .captures(block)
                    .map(|c| c[1].to_string()),
            })
        })
        .collect()
}

/// The files of the page's own host, each once, in the order the page links them: a PDF under
/// `/content/download/`, or an entry of the `/Media/` library, which redirects to one.
pub fn parse_files(html: &str, host: &str) -> Vec<String> {
    static FILE: OnceLock<Regex> = OnceLock::new();
    let mut files: Vec<String> = Vec::new();
    for c in regex(
        &FILE,
        r#"href="(?:https?://([^/"]+))?(/content/download/\d+/[^"]+?\.pdf|/Media/[^"]+)""#,
    )
    .captures_iter(html)
    {
        if c.get(1).is_some_and(|h| h.as_str() != host) {
            continue;
        }
        let url = format!("https://{host}{}", &c[2]);
        if !files.contains(&url) {
            files.push(url);
        }
    }
    files
}

/// `243305` in `https://…/content/download/243305/document/…pdf`.
pub fn file_id(url: &str) -> Option<&str> {
    url.split("/content/download/")
        .nth(1)?
        .split('/')
        .next()
        .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
}

pub fn read_items(dir: &Path) -> Result<HashMap<String, ItemRead>> {
    let path = dir.join("items.jsonl");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let mut items = HashMap::new();
    for line in fs::read_to_string(&path)?.lines() {
        let item: ItemRead = serde_json::from_str(line)
            .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))?;
        items.insert(item.url.clone(), item);
    }
    Ok(items)
}

pub fn append_item(dir: &Path, item: &ItemRead) -> Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("items.jsonl"))?;
    let line = serde_json::to_string(item)
        .map_err(|e| SourceError::Invalid(format!("item {}: {e}", item.url)))?;
    writeln!(file, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_court_code_gives_its_site() {
        let court = lj_parse::juradm_court("ta_chalons_en_champagne").unwrap();
        assert_eq!(
            host(court),
            "chalons-en-champagne.tribunal-administratif.fr"
        );
        let court = lj_parse::juradm_court("caa_versailles").unwrap();
        assert_eq!(host(court), "versailles.cour-administrative-appel.fr");
    }

    #[test]
    fn a_listing_gives_each_item_once_and_an_item_its_own_pdfs() {
        let host = "cergy-pontoise.tribunal-administratif.fr";
        let html = r#"<ul><li class="list__item">
            <a href="/decisions-de-justice/dernieres-decisions/polemia" class="u-img-4-3"></a>
            <a class="list__item__body"  href="/decisions-de-justice/dernieres-decisions/polemia" >
            <p class="list__item__title">Les juges des r&eacute;f&eacute;r&eacute;s rejettent</p></a>
            <time class="list__item__date" datetime="2026-10-30">30 octobre 2026</time></li>
            <li class="list__item">
            <a class="list__item__body"  href="/decisions-de-justice/dernieres-decisions/creche" >
            <p class="list__item__title">Crèche</p></a></li></ul>"#;
        let listed = parse_listing(html, host);
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed[0].url,
            format!("https://{host}/decisions-de-justice/dernieres-decisions/polemia")
        );
        assert_eq!(listed[0].title, "Les juges des référés rejettent");
        assert_eq!(listed[0].date.as_deref(), Some("2026-10-30"));
        assert_eq!(listed[1].date, None);

        let html = format!(
            r#"<a href="/content/download/243305/document/2623718%20-%20POLEMIA.pdf" title="T"></a>
            <a href="https://{host}/content/download/243305/document/2623718%20-%20POLEMIA.pdf"></a>
            <a href="https://www.conseil-etat.fr/content/download/1/document/ce.pdf"></a>
            <a href="/content/download/9/document/photo.jpg"></a>
            <a href="/Media/mediatheque-ta-cergy-pontoise/documents/2026/2604927"></a>"#
        );
        let files = parse_files(&html, host);
        assert_eq!(
            files,
            vec![
                format!(
                    "https://{host}/content/download/243305/document/2623718%20-%20POLEMIA.pdf"
                ),
                format!(
                    "https://{host}/Media/mediatheque-ta-cergy-pontoise/documents/2026/2604927"
                ),
            ]
        );
        assert_eq!(file_id(&files[0]), Some("243305"));
    }
}
