//! Legalis (`www.legalis.net`): its WordPress sitemap of decisions, each post with the time it was
//! last modified, and a decision's page, through the egress proxy, 3 s after the previous request.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lj_parse::LegalisPage;
use regex::Regex;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;

pub const BASE_URL: &str = "https://www.legalis.net";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(3);
const ATTEMPTS: u32 = 4;
const DECISIONS: &str = "https://www.legalis.net/jurisprudences/";

/// The site's answer: what it served, or the status it refused the request with.
pub enum Answer<T> {
    Page(T),
    Refused(u16),
}

/// A decision of the sitemap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub slug: String,
    /// The post's `lastmod`, as the sitemap writes it.
    pub modified: String,
}

pub struct LegalisClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
    sent: AtomicUsize,
}

impl LegalisClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "legalis.net is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
            sent: AtomicUsize::new(0),
        })
    }

    pub fn sent(&self) -> usize {
        self.sent.load(Ordering::Relaxed)
    }

    /// GETs `url` at the site's pace, again after a network failure or a 5xx, each wait one pace
    /// longer. A 403 or a 429 is the site's refusal, never waited out; `None` for a page gone.
    async fn get(&self, url: &str) -> Result<Answer<Option<String>>> {
        let mut last = self.last.lock().await;
        for attempt in 1.. {
            if let Some(at) = *last {
                tokio::time::sleep((PACE * attempt).saturating_sub(at.elapsed())).await;
            }
            let sent = self.http.get(url).send().await;
            *last = Some(Instant::now());
            self.sent.fetch_add(1, Ordering::Relaxed);
            let response = match sent {
                Ok(response) => response,
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect() || e.is_timeout() || e.is_request()) =>
                {
                    tracing::warn!(attempt, url, error = %e, "legalis: request failed, sent again");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = response.status();
            match status.as_u16() {
                403 | 429 => return Ok(Answer::Refused(status.as_u16())),
                404 | 410 => return Ok(Answer::Page(None)),
                _ if status.is_server_error() && attempt < ATTEMPTS => {
                    tracing::warn!(attempt, url, %status, "legalis: server error, sent again");
                }
                _ => {
                    return Ok(Answer::Page(Some(
                        response.error_for_status()?.text().await?,
                    )))
                }
            }
        }
        unreachable!("the attempts loop returns")
    }

    async fn sitemap(&self, url: &str) -> Result<Answer<String>> {
        match self.get(url).await? {
            Answer::Page(Some(xml)) => Ok(Answer::Page(xml)),
            Answer::Page(None) => Err(SourceError::Invalid(format!("legalis: {url} is gone"))),
            Answer::Refused(status) => Ok(Answer::Refused(status)),
        }
    }

    /// Every decision the sitemap lists, read from each of its decision files.
    pub async fn entries(&self) -> Result<Answer<Vec<Entry>>> {
        let index = match self.sitemap(&format!("{BASE_URL}/wp-sitemap.xml")).await? {
            Answer::Page(xml) => xml,
            Answer::Refused(status) => return Ok(Answer::Refused(status)),
        };
        let mut entries = Vec::new();
        for url in parse_index(&index) {
            match self.sitemap(&url).await? {
                Answer::Page(xml) => entries.extend(parse_sitemap(&xml)),
                Answer::Refused(status) => return Ok(Answer::Refused(status)),
            }
        }
        Ok(Answer::Page(entries))
    }

    /// The page of the decision at `slug`.
    pub async fn page(&self, slug: &str) -> Result<Answer<Option<String>>> {
        self.get(&format!("{DECISIONS}{slug}/")).await
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a valid pattern"))
}

/// The decision files of the sitemap index.
pub fn parse_index(xml: &str) -> Vec<String> {
    static LOC: OnceLock<Regex> = OnceLock::new();
    regex(
        &LOC,
        r"<loc>\s*(https://www\.legalis\.net/wp-sitemap-posts-jurisprudence-\d+\.xml)\s*</loc>",
    )
    .captures_iter(xml)
    .map(|c| c[1].to_string())
    .collect()
}

/// The decisions a sitemap file lists, by slug.
pub fn parse_sitemap(xml: &str) -> Vec<Entry> {
    static URL: OnceLock<Regex> = OnceLock::new();
    regex(
        &URL,
        r"(?s)<url>\s*<loc>\s*https://www\.legalis\.net/jurisprudences/([^/<\s]+)/?\s*</loc>\s*<lastmod>\s*([^<\s]+)\s*</lastmod>",
    )
    .captures_iter(xml)
    .map(|c| Entry {
        slug: c[1].to_string(),
        modified: c[2].to_string(),
    })
    .collect()
}

/// A decision page's post, title and body; `None` for a page that shows no decision.
pub fn parse_page(html: &str) -> Option<LegalisPage> {
    static SHORTLINK: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static HEADS: OnceLock<Regex> = OnceLock::new();
    let post_id = regex(
        &SHORTLINK,
        r#"<link rel=['"]shortlink['"] href=['"]https?://www\.legalis\.net/\?p=(\d+)['"]"#,
    )
    .captures(html)?[1]
        .parse()
        .ok()?;
    let title = regex(&TITLE, r#"(?s)<h2 class="titre_decision">(.*?)</h2>"#).captures(html)?;
    let body = &html[title.get(0)?.end()..];
    let body = &body[..body.find(r#"<div class="bas_article">"#)?];
    let body = regex(
        &HEADS,
        r#"(?s)<h3 class="sous_titre_decision">.*?</h3>|<p class="mots_cles_decision">.*?</p>"#,
    )
    .replace_all(body, "");
    Some(LegalisPage {
        post_id,
        title: strip_html(&title[1]).replace('\n', " "),
        text: strip_html(&body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_names_the_decision_files_alone() {
        let xml = r#"<sitemapindex><sitemap><loc>https://www.legalis.net/wp-sitemap-posts-post-1.xml</loc></sitemap><sitemap><loc>https://www.legalis.net/wp-sitemap-posts-jurisprudence-1.xml</loc></sitemap><sitemap><loc>https://www.legalis.net/wp-sitemap-posts-jurisprudence-2.xml</loc></sitemap></sitemapindex>"#;
        assert_eq!(
            parse_index(xml),
            vec![
                "https://www.legalis.net/wp-sitemap-posts-jurisprudence-1.xml",
                "https://www.legalis.net/wp-sitemap-posts-jurisprudence-2.xml",
            ]
        );
    }

    #[test]
    fn a_sitemap_gives_each_decision_with_its_modification() {
        let xml = r#"<urlset><url><loc>https://www.legalis.net/jurisprudences/cour-dappel-de-paris-arret-du-2-mai-2012/</loc><lastmod>2012-05-09T12:19:04+01:00</lastmod></url><url><loc>https://www.legalis.net/actualite/une-breve/</loc><lastmod>2012-05-10T10:00:00+01:00</lastmod></url></urlset>"#;
        assert_eq!(
            parse_sitemap(xml),
            vec![Entry {
                slug: "cour-dappel-de-paris-arret-du-2-mai-2012".into(),
                modified: "2012-05-09T12:19:04+01:00".into(),
            }]
        );
    }

    #[test]
    fn a_page_gives_its_post_its_title_and_its_body_without_the_keywords() {
        let html = r#"<head><link rel='shortlink' href='https://www.legalis.net/?p=2048' /></head>
<div class="clear"></div>
        <h2 class="titre_decision">Cour d&rsquo;appel de Paris, arrêt du 2 mai 2012</h2>
        <h3 class="sous_titre_decision">Société A / Société B</h3>
            <p class="mots_cles_decision">
                contrat - responsabilité            </p>
        <p>La société A demande <em>la résolution</em> du contrat.</p>
<p>PAR CES MOTIFS<br />
Rejette la demande.</p>
        <div class="spacer-10 clear">&nbsp;</div>
        <div class="bas_article">&nbsp;</div>
<h2>En complément</h2>"#;
        let page = parse_page(html).unwrap();
        assert_eq!(page.post_id, 2048);
        assert_eq!(
            page.title,
            "Cour d\u{2019}appel de Paris, arrêt du 2 mai 2012"
        );
        assert_eq!(
            page.text,
            "La société A demande la résolution du contrat.\nPAR CES MOTIFS\nRejette la demande."
        );
        assert_eq!(parse_page("<html><h2>Actualité</h2></html>"), None);
    }
}
