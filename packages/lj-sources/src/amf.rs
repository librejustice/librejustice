//! The decisions of the AMF's Commission des sanctions: the sitemap that lists their pages with the
//! date each last changed, a page, its file. The cache holds `pages.jsonl`, the last line of a URL winning.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.amf-france.org";
const SITEMAP: &str = "https://www.amf-france.org/sitemap.xml";
const DECISIONS: &str = "/fr/sanctions-transactions/decisions-de-la-commission-des-sanctions/san-";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(2);
const ATTEMPTS: u32 = 5;

/// A decision page as the sitemap lists it.
#[derive(Debug, Clone)]
pub struct Listed {
    pub url: String,
    pub lastmod: String,
}

/// A decision page: what it says of the decision, and the file it serves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmfPage {
    pub url: String,
    pub lastmod: String,
    /// `SAN-2021-08`.
    pub number: String,
    pub title: String,
    /// The day the page was put online, as it reads: `04 mai 2021`.
    pub published: Option<String>,
    /// The file's path on the site, `/sites/institutionnel/files/…pdf`.
    pub file: String,
    pub keywords: Vec<String>,
    pub appeals: Vec<Appeal>,
}

/// A row of the page's appeals table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appeal {
    pub date: Option<String>,
    pub court: String,
    pub title: String,
    pub text: String,
}

pub struct AmfClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl AmfClient {
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

    /// Sends a GET of `url` at the site's pace, again after a network failure or a 5xx, up to
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
                    tracing::warn!(attempt, error = %e, "amf: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    /// Every decision page the sitemap lists.
    pub async fn listed(&self) -> Result<Vec<Listed>> {
        let sitemap = self.get(SITEMAP).await?.text().await?;
        let listed = parse_sitemap(&sitemap);
        if listed.is_empty() {
            return Err(SourceError::Invalid(
                "the AMF sitemap lists no decision of the Commission des sanctions".into(),
            ));
        }
        Ok(listed)
    }

    /// The page `listed`, `None` when it names no decision file.
    pub async fn page(&self, listed: &Listed) -> Result<Option<AmfPage>> {
        let html = self.get(&listed.url).await?.text().await?;
        Ok(parse_page(&html, listed))
    }

    /// The file at `path`, `None` when the site answers 404.
    pub async fn file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.get(&format!("{BASE_URL}{path}")).await {
            Ok(response) => Ok(Some(response.bytes().await?.to_vec())),
            Err(SourceError::Http(e)) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

pub fn parse_sitemap(sitemap: &str) -> Vec<Listed> {
    static URL: OnceLock<Regex> = OnceLock::new();
    regex(&URL, r"<url><loc>([^<]+)</loc><lastmod>([^<]+)</lastmod>")
        .captures_iter(sitemap)
        .filter(|c| c[1].contains(DECISIONS))
        .map(|c| Listed {
            url: c[1].to_string(),
            lastmod: c[2].to_string(),
        })
        .collect()
}

/// The text of an HTML fragment, its pieces between tags joined by a space.
fn cell_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    regex(&TAG, r"(?s)<[^>]*>")
        .split(html)
        .map(|piece| {
            decode_entities(piece)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_page(html: &str, listed: &Listed) -> Option<AmfPage> {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static PUBLISHED: OnceLock<Regex> = OnceLock::new();
    static FILE: OnceLock<Regex> = OnceLock::new();
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    static KEYWORDS: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static APPEALS: OnceLock<Regex> = OnceLock::new();
    static ROW: OnceLock<Regex> = OnceLock::new();
    static DATETIME: OnceLock<Regex> = OnceLock::new();
    let title = cell_text(&regex(&TITLE, r"(?s)<h1[^>]*>(.*?)</h1>").captures(html)?[1]);
    let file = regex(
        &FILE,
        r#"href="(?:https://www\.amf-france\.org)?(/sites/[^"]+\.pdf)"[^>]*>\s*Télécharger\s*<"#,
    )
    .captures(html)?[1]
        .to_string();
    let published = regex(
        &PUBLISHED,
        r#"<div class="date">([^<]+)</div>\s*<ul class="list-simple-link">"#,
    )
    .captures(html)
    .map(|c| cell_text(&c[1]));
    let number_re = regex(&NUMBER, r"(?i)^san-\d{4}-\d+");
    let slug = listed.url.rsplit('/').next().unwrap_or_default();
    let number = number_re
        .find(&title)
        .or_else(|| number_re.find(slug))?
        .as_str()
        .to_uppercase();
    let keywords = regex(
        &KEYWORDS,
        r#"(?s)<ul class="list-tags-secondaire">(.*?)</ul>"#,
    )
    .captures(html)
    .map(|c| {
        regex(&TAG, r#"(?s)class="tag">(.*?)</a>"#)
            .captures_iter(&c[1])
            .map(|t| cell_text(&t[1]))
            .collect()
    })
    .unwrap_or_default();
    let appeals = regex(&APPEALS, r#"(?s)>Recours</h2>.*?<tbody>(.*?)</tbody>"#)
        .captures(html)
        .map(|c| {
            regex(
                &ROW,
                r#"(?s)<tr[^>]*>\s*<td[^>]*>(.*?)</td>\s*<td[^>]*>(.*?)</td>\s*<td[^>]*>\s*<div[^>]*>(.*?)</div>(.*?)</td>\s*</tr>"#,
            )
            .captures_iter(&c[1])
            .map(|r| Appeal {
                date: regex(&DATETIME, r#"datetime="(\d{4}-\d{2}-\d{2})"#)
                    .captures(&r[1])
                    .map(|d| d[1].to_string()),
                court: cell_text(&r[2]),
                title: cell_text(&r[3]),
                text: cell_text(&r[4]),
            })
            .collect()
        })
        .unwrap_or_default();
    Some(AmfPage {
        url: listed.url.clone(),
        lastmod: listed.lastmod.clone(),
        number,
        title,
        published,
        file,
        keywords,
        appeals,
    })
}

pub fn read_pages(dir: &Path) -> Result<HashMap<String, AmfPage>> {
    let path = dir.join("pages.jsonl");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let mut pages = HashMap::new();
    for line in fs::read_to_string(&path)?.lines() {
        let page: AmfPage = serde_json::from_str(line)
            .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))?;
        pages.insert(page.url.clone(), page);
    }
    Ok(pages)
}

pub fn append_page(dir: &Path, page: &AmfPage) -> Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("pages.jsonl"))?;
    let line = serde_json::to_string(page)
        .map_err(|e| SourceError::Invalid(format!("page {}: {e}", page.url)))?;
    writeln!(file, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_page_gives_its_title_file_keywords_and_appeals() {
        let listed = Listed {
            url: format!("{BASE_URL}{DECISIONS}2021-08"),
            lastmod: "2026-05-04T12:58Z".into(),
        };
        let html = r##"<div class="date">04 mai 2021</div><ul class="list-simple-link"><li>
            <a href="https://www.amf-france.org/sites/institutionnel/files/private/2026-04/san-2021-08.pdf" class="x"> Télécharger </a></li></ul>
            <h1 class="like-h1"> SAN-2021-08 - Décision du 30 avril 2021 à l&#039;égard de la société X</h1>
            <ul class="list-tags-secondaire"><li><a href="/fr/listing_sanction/P" class="tag">Procédure</a></li></ul>
            <h2 class="like-h2">Recours</h2><table><tbody>
            <tr role="row"><td data-order="1"><div class="date"><time datetime="2023-03-31T16:18:24+02:00">31 mars 2023</time></div></td>
            <td><a href="#" class="tag">Conseil d&#039;Etat</a></td>
            <td><div class="like-h3">Recours contre la décision</div><p>Le recours est rejeté.</p></td></tr>
            </tbody></table>"##;
        let page = parse_page(html, &listed).unwrap();
        assert_eq!(page.number, "SAN-2021-08");
        assert!(page.title.ends_with("à l'égard de la société X"));
        assert_eq!(page.published.as_deref(), Some("04 mai 2021"));
        assert_eq!(
            page.file,
            "/sites/institutionnel/files/private/2026-04/san-2021-08.pdf"
        );
        assert_eq!(page.keywords, vec!["Procédure"]);
        assert_eq!(page.appeals.len(), 1);
        assert_eq!(page.appeals[0].date.as_deref(), Some("2023-03-31"));
        assert_eq!(page.appeals[0].court, "Conseil d'Etat");
        assert_eq!(page.appeals[0].text, "Le recours est rejeté.");
    }
}
