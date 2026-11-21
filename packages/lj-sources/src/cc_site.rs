//! The decisions of conseil-constitutionnel.fr: the pages its feed lists, a page's fields.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lj_parse::CcSitePage;
use regex::Regex;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::html_strip::decode_entities;

const FEED: &str = "https://www.conseil-constitutionnel.fr/flux/rss.xml";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(1);
const ATTEMPTS: u32 = 5;

pub struct CcClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl CcClient {
    /// Goes out directly, without the egress gateway.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
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
                    tracing::warn!(attempt, url, error = %e, "cc-site: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    /// The decision pages the feed lists, newest first.
    pub async fn feed(&self) -> Result<Vec<String>> {
        Ok(parse_feed(&self.get(FEED).await?.text().await?))
    }

    pub async fn page(&self, url: &str) -> Result<CcSitePage> {
        Ok(parse_page(&self.get(url).await?.text().await?, url))
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

pub fn parse_feed(xml: &str) -> Vec<String> {
    static LINK: OnceLock<Regex> = OnceLock::new();
    let mut urls: Vec<String> = Vec::new();
    for c in regex(
        &LINK,
        r"<link>(https://www\.conseil-constitutionnel\.fr/decision/\d{4}/[0-9_]+[A-Z]+\.htm)</link>",
    )
    .captures_iter(xml)
    {
        if !urls.iter().any(|u| u == &c[1]) {
            urls.push(c[1].to_string());
        }
    }
    urls
}

/// Markup to text, entities decoded, every run of spaces one space. An inline tag
/// leaves no space: `n<sup>o </sup>513189` reads `no 513189`.
fn text(html: &str) -> String {
    static INLINE: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let html = regex(
        &INLINE,
        r"(?i)</?(?:a|abbr|b|em|i|span|strong|sub|sup|u)\b[^>]*>",
    )
    .replace_all(html, "");
    decode_entities(&regex(&TAG, r"(?s)<[^>]*>").replace_all(&html, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What follows the opening tag of the first element whose class names the field.
fn field<'a>(html: &'a str, field: &str) -> Option<&'a str> {
    let name = format!("name-field-{field}");
    let at = html
        .match_indices(&name)
        .map(|(at, _)| at)
        .find(|&at| matches!(html.as_bytes().get(at + name.len()), Some(b' ' | b'"')))?;
    let open = at + html[at..].find('>')? + 1;
    Some(&html[open..])
}

pub fn parse_page(html: &str, url: &str) -> CcSitePage {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static BREAK: OnceLock<Regex> = OnceLock::new();
    let until_div = |s: &str| text(s.split("</div>").next().unwrap_or_default());
    let content = field(html, "contenu-original").unwrap_or_default();
    let content = content
        .find("cartouche-decision-print")
        .and_then(|at| content[..at].rfind('<'))
        .map_or(content, |end| &content[..end]);
    CcSitePage {
        url: url.to_string(),
        title: regex(&TITLE, r"<title>([^<]*)</title>")
            .captures(html)
            .map(|c| text(&c[1]))
            .unwrap_or_default(),
        parties: field(html, "titre-complet")
            .map(until_div)
            .unwrap_or_default(),
        solution: field(html, "solution-normalisee")
            .map(until_div)
            .filter(|s| !s.is_empty()),
        paragraphs: regex(&BREAK, r"(?i)</p>|<br\s*/?>|</h\d>|</li>")
            .split(content)
            .map(text)
            .filter(|p| !p.is_empty())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_gives_its_fields_and_its_text_a_paragraph_a_line() {
        let html = r#"<title>Décision n° 2026-1228 QPC du 22 octobre 2026 | Conseil constitutionnel</title>
            <div class="field name-field-titre-complet">M. A. [Objet]</div>
            <div class="field name-field-solution-normalisee">Conformité</div>
            <div class="text-formatted field name-field-contenu-original"><p>LE CONSEIL CONSTITUTIONNEL A ÉTÉ SAISI le 22&nbsp;juillet&nbsp;2026 (décision n<sup>o </sup>513189).</p>
            <p><u>Au vu des textes suivants</u>&nbsp;:</p><p>Rendu public le 22 octobre 2026.</p>
            <p>JORF n°0248 du 23 octobre 2026, texte n° 87<br>ECLI : FR : CC : 2026 : 2026.1228.QPC</p>
            </div><div class="cartouche-decision-print"><p>Autre décision</p></div>"#;
        let page = parse_page(html, "u");
        assert_eq!(
            page.title,
            "Décision n° 2026-1228 QPC du 22 octobre 2026 | Conseil constitutionnel"
        );
        assert_eq!(page.parties, "M. A. [Objet]");
        assert_eq!(page.solution.as_deref(), Some("Conformité"));
        assert_eq!(
            page.paragraphs,
            vec![
                "LE CONSEIL CONSTITUTIONNEL A ÉTÉ SAISI le 22 juillet 2026 (décision no 513189).",
                "Au vu des textes suivants :",
                "Rendu public le 22 octobre 2026.",
                "JORF n°0248 du 23 octobre 2026, texte n° 87",
                "ECLI : FR : CC : 2026 : 2026.1228.QPC",
            ]
        );

        let feed = r#"<item><link>https://www.conseil-constitutionnel.fr/actualites/x</link></item>
            <item><link>https://www.conseil-constitutionnel.fr/decision/2026/20261230_1231QPC.htm</link></item>
            <item><link>https://www.conseil-constitutionnel.fr/decision/2026/2026341L.htm</link></item>"#;
        assert_eq!(
            parse_feed(feed),
            vec![
                "https://www.conseil-constitutionnel.fr/decision/2026/20261230_1231QPC.htm",
                "https://www.conseil-constitutionnel.fr/decision/2026/2026341L.htm",
            ]
        );
    }
}
