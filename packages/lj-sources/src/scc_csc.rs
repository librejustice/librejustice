//! The judgments of the Supreme Court of Canada on `decisions.scc-csc.ca`: the French listing of a
//! year, newest first, and a judgment's page. Every request leaves through the egress proxy, 30
//! seconds after the previous one.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use lj_parse::SccPage;
use regex::Regex;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://decisions.scc-csc.ca";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(30);
const ATTEMPTS: u32 = 5;
/// The judgments a listing page holds.
pub const PER_PAGE: usize = 25;
/// What the site serves a client it takes for a robot instead of the page asked.
const REFUSAL: &str = "robocop/";

/// The site's answer: the page asked, or its refusal.
pub enum Answer<T> {
    Page(T),
    Refused,
}

/// A page of a year's judgments, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub year: i32,
    pub ids: Vec<u64>,
    /// The year's judgments over all its pages.
    pub total: usize,
    /// Every year the listing offers, newest first.
    pub years: Vec<i32>,
}

impl Listing {
    pub fn pages(&self) -> usize {
        self.total.div_ceil(PER_PAGE)
    }
}

pub struct SccClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
    sent: std::sync::atomic::AtomicUsize,
}

impl SccClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the Supreme Court of Canada's site is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
            sent: Default::default(),
        })
    }

    /// The requests sent so far.
    pub fn sent(&self) -> usize {
        self.sent.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// GETs `path` at the site's pace, again after a network failure or a 5xx, up to [`ATTEMPTS`]
    /// times, each wait one pace longer.
    async fn get(&self, path: &str) -> Result<Answer<String>> {
        let url = format!("{BASE_URL}{path}");
        let mut last = self.last.lock().await;
        for attempt in 1.. {
            if let Some(at) = *last {
                tokio::time::sleep((PACE * attempt).saturating_sub(at.elapsed())).await;
            }
            let sent = self.http.get(&url).send().await;
            *last = Some(Instant::now());
            self.sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match sent.and_then(reqwest::Response::error_for_status) {
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect()
                            || e.is_timeout()
                            || e.is_request()
                            || e.status().is_some_and(|s| s.is_server_error())) =>
                {
                    tracing::warn!(attempt, url, error = %e, "scc-csc: request failed, sent again");
                }
                sent => {
                    let response = sent?;
                    if response.url().path().contains(REFUSAL) {
                        return Ok(Answer::Refused);
                    }
                    let body = response.text().await?;
                    return Ok(if body.contains(REFUSAL) {
                        Answer::Refused
                    } else {
                        Answer::Page(body)
                    });
                }
            }
        }
        unreachable!("the attempts loop returns")
    }

    /// Page `page` of `year`'s judgments, of the newest year with `None`.
    pub async fn listing(&self, year: Option<i32>, page: usize) -> Result<Answer<Listing>> {
        let path = match year {
            Some(year) => format!("/scc-csc/scc-csc/fr/{year}/nav_date.do?page={page}&iframe=true"),
            None => format!("/scc-csc/scc-csc/fr/nav_date.do?page={page}&iframe=true"),
        };
        Ok(match self.get(&path).await? {
            Answer::Page(html) => {
                Answer::Page(parse_listing(&html).ok_or_else(|| {
                    SourceError::Invalid(format!("scc-csc: {path} is no listing"))
                })?)
            }
            Answer::Refused => Answer::Refused,
        })
    }

    /// The French page of judgment `id`, as the site serves it.
    pub async fn item(&self, id: u64) -> Result<Answer<String>> {
        self.get(&format!(
            "/scc-csc/scc-csc/fr/item/{id}/index.do?iframe=true"
        ))
        .await
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// The judgments a listing page links, its year, its count and the years it offers; `None` for a
/// page that names no selected year or no count.
pub fn parse_listing(html: &str) -> Option<Listing> {
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static SELECTED: OnceLock<Regex> = OnceLock::new();
    static YEAR: OnceLock<Regex> = OnceLock::new();
    static TOTAL: OnceLock<Regex> = OnceLock::new();
    let year: i32 = regex(
        &SELECTED,
        r#"<li class="selected\s*"><span title="(\d{4})""#,
    )
    .captures(html)?[1]
        .parse()
        .ok()?;
    let total: usize = regex(&TOTAL, r"([\d,]+)&nbsp;résultat\(s\)").captures(html)?[1]
        .replace(',', "")
        .parse()
        .ok()?;
    let mut ids: Vec<u64> = Vec::new();
    for c in regex(&ITEM, r#"href="/scc-csc/scc-csc/fr/item/(\d+)/index\.do""#).captures_iter(html)
    {
        let id = c[1].parse().ok()?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut years: Vec<i32> = regex(&YEAR, r#"/scc-csc/scc-csc/fr/(\d{4})/nav_date\.do""#)
        .captures_iter(html)
        .filter_map(|c| c[1].parse().ok())
        .chain([year])
        .collect();
    years.sort_unstable_by(|a, b| b.cmp(a));
    years.dedup();
    Some(Listing {
        year,
        ids,
        total,
        years,
    })
}

/// Markup to text, entities decoded, a `<br>` starting a line, each line's spaces collapsed.
fn lines(html: &str) -> String {
    static BREAK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let html = regex(&BREAK, r"(?i)<br\s*/?>").replace_all(html, "\n");
    decode_entities(&regex(&TAG, r"(?s)<[^>]*>").replace_all(&html, ""))
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A judgment page's title, information table and text. The table leaves its value cells
/// unclosed, so a value runs to the end of its row.
pub fn parse_item(id: u64, html: &str) -> SccPage {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static ROW: OnceLock<Regex> = OnceLock::new();
    let title = regex(&TITLE, r#"(?s)<h3 class="title">(.*?)</h3>"#)
        .captures(html)
        .map(|c| lines(&c[1]).replace('\n', " "))
        .unwrap_or_default();
    let fields = regex(
        &ROW,
        r#"(?s)<td class="label">(.*?)</td>\s*<td class="metadata">(.*?)</tr>"#,
    )
    .captures_iter(html)
    .map(|c| (lines(&c[1]), lines(&c[2])))
    .collect();
    let text = html
        .find(r#"<div id="document-content">"#)
        .map(|start| {
            let body = &html[start..];
            let end = body
                .find(r#"<div id="lexboxAnnotationHook">"#)
                .unwrap_or(body.len());
            strip_html(&body[..end])
        })
        .unwrap_or_default();
    SccPage {
        id,
        title,
        fields,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_judgment_page_gives_its_unclosed_rows_and_its_text() {
        let html = r#"<h3 class="title">Qu&eacute;bec (Procureur g&eacute;n&eacute;ral) c. SGS Canada inc.</h3>
    <table>
                    <tr>
                        <td class="label">Date</td>
                        <td class="metadata">
                                2026-11-06
                    </tr>
                    <tr>
                        <td class="label">Référence neutre</td>
                        <td class="metadata">
                                2026 CSC 37
                    </tr>
                    <tr>
                        <td class="label">Sujets</td>
                        <td class="metadata">
                                Droit constitutionnel<br/>Relations de travail<br/>
                    </tr>
    </table>
<div id="document-content">
<div class="documentcontent"><p class="MsoNormal">COUR SUPRÊME DU CANADA</p>
<p>Référence&nbsp;: Québec (Procureur général) c. SGS Canada inc., 2026 CSC 37</p>
<p>Procureur général du Québec&nbsp;&nbsp;&nbsp;&nbsp; Appelant</p></div></div>
<div id="lexboxAnnotationHook"></div>
<div id="decisia-reflex2-dialog" title="Lien généré automatiquement par Norma"></div>"#;
        let page = parse_item(21749, html);
        assert_eq!(page.title, "Québec (Procureur général) c. SGS Canada inc.");
        assert_eq!(
            page.fields,
            vec![
                ("Date".to_string(), "2026-11-06".to_string()),
                ("Référence neutre".to_string(), "2026 CSC 37".to_string()),
                (
                    "Sujets".to_string(),
                    "Droit constitutionnel\nRelations de travail".to_string()
                ),
            ]
        );
        assert_eq!(
            page.text,
            "COUR SUPRÊME DU CANADA\nRéférence : Québec (Procureur général) c. SGS Canada inc., 2026 CSC 37\nProcureur général du Québec Appelant"
        );
    }

    #[test]
    fn a_listing_page_gives_its_year_its_count_and_the_years_offered() {
        let html = r#"<ul class="listing date">
                <li class=""><a target="_parent" href="/scc-csc/scc-csc/fr/1971/nav_date.do" title="1971">1971</a></li>
                <li class="selected "><span title="1970">1970</span></li>
    </ul>
        <h2>1,102&nbsp;résultat(s)</h2>
<span class="title"><a target="_parent" href="/scc-csc/scc-csc/fr/item/4907/index.do">B.C. Telephone</a></span>
<a target="_blank" href="/scc-csc/scc-csc/fr/4907/1/document.do"></a>
<span class="title"><a target="_parent" href="/scc-csc/scc-csc/fr/item/4174/index.do">Deakins</a></span>
<a target="_parent" href="/scc-csc/scc-csc/fr/1970/nav_date.do?page=2">2</a>"#;
        let listing = parse_listing(html).unwrap();
        assert_eq!(listing.year, 1970);
        assert_eq!(listing.ids, vec![4907, 4174]);
        assert_eq!(listing.total, 1102);
        assert_eq!(listing.pages(), 45);
        assert_eq!(listing.years, vec![1971, 1970]);
    }
}
