//! JuriCAF (`juricaf.org`): the decision pages its sitemaps and country feeds name, read one at a time.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use reqwest::header::{IF_MODIFIED_SINCE, LAST_MODIFIED};
use reqwest::StatusCode;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://juricaf.org";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(2);

/// A decision page a sitemap names, with the day it was last modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapEntry {
    pub slug: String,
    pub lastmod: Option<String>,
}

/// A sitemap read again: its `Last-Modified` and its entries.
pub struct Sitemap {
    pub last_modified: Option<String>,
    pub entries: Vec<SitemapEntry>,
}

pub struct JuricafClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl JuricafClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    /// A refusal or a sign of load (403, 429, 503) stops the crawl rather than waiting it out.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
        }
        let response = request.send().await;
        *last = Some(Instant::now());
        let response = response?;
        let status = response.status().as_u16();
        if matches!(status, 403 | 429 | 503) {
            return Err(SourceError::Invalid(format!(
                "juricaf answers {status} on {}",
                response.url()
            )));
        }
        Ok(response)
    }

    async fn get(&self, url: &str) -> Result<String> {
        Ok(self
            .send(self.http.get(url))
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    /// The sitemaps the index names.
    pub async fn sitemaps(&self) -> Result<Vec<String>> {
        Ok(locs(&self.get(&format!("{BASE_URL}/sitemap.xml")).await?))
    }

    /// A sitemap's decision pages, `None` when unchanged since the `Last-Modified` it last gave.
    pub async fn sitemap(&self, url: &str, since: Option<&str>) -> Result<Option<Sitemap>> {
        let mut request = self.http.get(url);
        if let Some(since) = since {
            request = request.header(IF_MODIFIED_SINCE, since);
        }
        let response = self.send(request).await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let response = response.error_for_status()?;
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(Some(Sitemap {
            last_modified,
            entries: sitemap_entries(&response.text().await?),
        }))
    }

    /// The slugs of a country's 30 latest decisions by date, the country named as its pages do.
    pub async fn feed(&self, country: &str) -> Result<Vec<String>> {
        let url = reqwest::Url::parse(&format!(
            "{BASE_URL}/recherche/+/facet_pays:{country}?format=rss"
        ))
        .map_err(|e| SourceError::Invalid(format!("juricaf feed of {country}: {e}")))?;
        Ok(feed_slugs(&self.get(url.as_str()).await?))
    }

    /// A decision page, `None` when gone though a sitemap still names it.
    pub async fn page(&self, slug: &str) -> Result<Option<String>> {
        let response = self
            .send(self.http.get(format!("{BASE_URL}/arret/{slug}")))
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.text().await?))
    }
}

fn locs(xml: &str) -> Vec<String> {
    static LOC: OnceLock<Regex> = OnceLock::new();
    LOC.get_or_init(|| Regex::new(r"<loc>\s*([^<\s]+)\s*</loc>").expect("loc regex"))
        .captures_iter(xml)
        .map(|c| c[1].to_string())
        .collect()
}

/// A slug becomes a file name, so one outside `[A-Za-z0-9_-]` is not taken.
fn slug_of(url: &str) -> Option<&str> {
    static SLUG: OnceLock<Regex> = OnceLock::new();
    let slug = url.split_once("juricaf.org/arret/")?.1;
    SLUG.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]+$").expect("slug regex"))
        .is_match(slug)
        .then_some(slug)
}

fn sitemap_entries(xml: &str) -> Vec<SitemapEntry> {
    static URL: OnceLock<Regex> = OnceLock::new();
    static LASTMOD: OnceLock<Regex> = OnceLock::new();
    let url = URL.get_or_init(|| Regex::new(r"(?s)<url>(.*?)</url>").expect("url regex"));
    let lastmod = LASTMOD
        .get_or_init(|| Regex::new(r"<lastmod>\s*([^<\s]+)\s*</lastmod>").expect("lastmod regex"));
    url.captures_iter(xml)
        .filter_map(|c| {
            let block = c.get(1)?.as_str();
            let slug = slug_of(locs(block).first()?)?.to_string();
            Some(SitemapEntry {
                slug,
                lastmod: lastmod.captures(block).map(|m| m[1].to_string()),
            })
        })
        .collect()
}

fn feed_slugs(xml: &str) -> Vec<String> {
    static LINK: OnceLock<Regex> = OnceLock::new();
    LINK.get_or_init(|| Regex::new(r"<link>\s*([^<\s]+)\s*</link>").expect("link regex"))
        .captures_iter(xml)
        .filter_map(|c| slug_of(&c[1]).map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sitemap_yields_the_slugs_a_file_name_can_hold_with_their_day() {
        let xml = "<urlset>\
            <url><loc>https://juricaf.org/arret/SENEGAL-COURSUPREME-20241114-57</loc><lastmod>2026-02-22</lastmod></url>\
            <url><loc>https://juricaf.org/arret/../etc</loc></url>\
            <url><loc>https://juricaf.org/recherche/x</loc></url>\
            </urlset>";
        assert_eq!(
            sitemap_entries(xml),
            [SitemapEntry {
                slug: "SENEGAL-COURSUPREME-20241114-57".into(),
                lastmod: Some("2026-02-22".into()),
            }]
        );
        let index = "<sitemapindex><sitemap><loc>https://juricaf.org/sitemap_00.xml</loc></sitemap></sitemapindex>";
        assert_eq!(locs(index), ["https://juricaf.org/sitemap_00.xml"]);
    }

    #[test]
    fn a_feed_yields_the_slugs_of_its_items() {
        let rss = "<rss><channel><link>https://juricaf.org</link>\
            <item><link>http://juricaf.org/arret/SENEGAL-CONSEILCONSTITUTIONNEL-20250423-1C25</link></item>\
            </channel></rss>";
        assert_eq!(
            feed_slugs(rss),
            ["SENEGAL-CONSEILCONSTITUTIONNEL-20250423-1C25"]
        );
    }
}
