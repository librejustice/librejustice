//! The Conseil du contentieux des étrangers (`rvv-cce.be`): the counts of its arrêts in a language
//! by year and month, a month's listing, and an arrêt's PDF, one request every ten seconds.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use lj_parse::CceLanguage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://www.rvv-cce.be";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// The site's `robots.txt` asks for a `Crawl-delay` of 10.
pub const PACE: Duration = Duration::from_secs(10);
/// The arrêts a listing page holds.
pub const PER_PAGE: usize = 50;
/// The pauses before a request is sent again after a transport error; a refusal is not sent again.
const RETRY_PAUSES: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

/// An arrêt as a row of the listing gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub number: u32,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub url: String,
    pub procedure: Option<String>,
    pub country: Option<String>,
    pub composition: Option<String>,
    pub judge: Option<String>,
}

/// A page of a listing: the arrêts the listing counts over all its pages, and this page's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub total: usize,
    pub rows: Vec<Row>,
}

impl Listing {
    pub fn pages(&self) -> usize {
        self.total.div_ceil(PER_PAGE)
    }
}

pub struct CceClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
    sent: AtomicUsize,
}

impl CceClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "rvv-cce.be is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
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
            sent: AtomicUsize::new(0),
        })
    }

    /// The requests sent so far.
    pub fn sent(&self) -> usize {
        self.sent.load(Ordering::Relaxed)
    }

    /// One request [`PACE`] after the previous, again after each of [`RETRY_PAUSES`] on a
    /// transport error. A 401, 403, 429 or 503 is a refusal that stops the caller; a 404 is `None`.
    async fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let mut last = self.last.lock().await;
        let mut pauses = RETRY_PAUSES.iter();
        loop {
            if let Some(at) = *last {
                tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
            }
            let sent = self.http.get(url).send().await;
            *last = Some(Instant::now());
            self.sent.fetch_add(1, Ordering::Relaxed);
            let read = match sent {
                Ok(response) => match response.status().as_u16() {
                    status @ (401 | 403 | 429 | 503) => {
                        return Err(SourceError::Invalid(format!(
                            "rvv-cce.be refuses with {status} on {url}"
                        )))
                    }
                    404 => return Ok(None),
                    _ => response.error_for_status()?.bytes().await,
                },
                Err(e) => Err(e),
            };
            match read {
                Ok(bytes) => return Ok(Some(bytes.to_vec())),
                Err(e) => match pauses.next() {
                    Some(pause) if !e.is_builder() => {
                        tracing::warn!(url, error = %e, pause_s = pause.as_secs(), "rvv-cce.be: transport error, sent again after a pause");
                        tokio::time::sleep(*pause).await;
                    }
                    _ => return Err(e.into()),
                },
            }
        }
    }

    async fn page(&self, path: &str) -> Result<String> {
        let url = format!("{BASE_URL}{path}");
        let bytes = self
            .get(&url)
            .await?
            .ok_or_else(|| SourceError::Invalid(format!("rvv-cce.be answers 404 on {url}")))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The arrêts in `language` of each year, `("2026", 7337)`.
    pub async fn years(&self, language: CceLanguage) -> Result<Vec<(String, usize)>> {
        let path = listing_path(language);
        let html = self.page(&path).await?;
        let years = facets(&html, &format!("{path}/date/"));
        if years.is_empty() {
            return Err(SourceError::Invalid(format!(
                "rvv-cce.be: no year counts on {path}"
            )));
        }
        Ok(years)
    }

    /// The arrêts in `language` of each month of `year`, `("2026-09", 912)`.
    pub async fn months(&self, language: CceLanguage, year: &str) -> Result<Vec<(String, usize)>> {
        let path = format!("{}/date/{year}", listing_path(language));
        let html = self.page(&path).await?;
        let months = facets(&html, &format!("{path}/date/"));
        if months.is_empty() {
            return Err(SourceError::Invalid(format!(
                "rvv-cce.be: no month counts on {path}"
            )));
        }
        Ok(months)
    }

    /// Page `page`, from 0, of the arrêts in `language` of `month`, `2026-09`.
    pub async fn listing(
        &self,
        language: CceLanguage,
        month: &str,
        page: usize,
    ) -> Result<Listing> {
        let year = &month[..4];
        let mut path = format!("{}/date/{year}/date/{month}", listing_path(language));
        if page > 0 {
            path.push_str(&format!("?page={page}"));
        }
        let html = self.page(&path).await?;
        parse_listing(&html)
            .ok_or_else(|| SourceError::Invalid(format!("rvv-cce.be: {path} is no listing")))
    }

    /// An arrêt's PDF, `None` when the site no longer has it.
    pub async fn pdf(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let Some(bytes) = self.get(url).await? else {
            return Ok(None);
        };
        if !bytes.starts_with(b"%PDF") {
            return Err(SourceError::Invalid(format!(
                "rvv-cce.be serves no PDF on {url}"
            )));
        }
        Ok(Some(bytes))
    }
}

/// The French interface's listing of the arrêts in `language`, whose rows read the same whatever
/// the arrêts' language.
fn listing_path(language: CceLanguage) -> String {
    let facet = match language {
        CceLanguage::French => "french",
        CceLanguage::Dutch => "dutch",
        CceLanguage::German => "german",
    };
    format!("/fr/arr/lang/{facet}")
}

/// The values of the facet links under `prefix` and their counts, in the page's order.
pub fn facets(html: &str, prefix: &str) -> Vec<(String, usize)> {
    let link = Regex::new(&format!(
        r#"href="{}([0-9-]+)"[^>]*>[^<]*<span class="facetcount">(\d+)</span>"#,
        regex::escape(prefix)
    ))
    .expect("cce facet regex");
    link.captures_iter(html)
        .filter_map(|c| Some((c[1].to_string(), c[2].parse().ok()?)))
        .collect()
}

static TOTAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Nombre de résultats:\s*(\d+)").expect("cce total regex"));
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"href="([^"]+\.pdf)"[^>]*>\s*Arr\S*\s+(\d+)\s+de\s+<span[^>]*content="(\d{4}-\d{2}-\d{2})"#)
        .expect("cce row regex")
});
static PROCEDURE: LazyLock<Regex> = LazyLock::new(|| field_regex("procedure"));
static COMPOSITION: LazyLock<Regex> = LazyLock::new(|| field_regex("composition"));
static JUDGE: LazyLock<Regex> = LazyLock::new(|| field_regex("judge"));
static COUNTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)Pays d&#039;origine:\s*<strong>([^<]*)</strong>").expect("cce country regex")
});

fn field_regex(name: &str) -> Regex {
    Regex::new(&format!(
        r#"(?s)views-field-field-{name}">.*?<strong[^>]*>([^<]*)</strong>"#
    ))
    .expect("cce field regex")
}

fn value(re: &Regex, row: &str) -> Option<String> {
    re.captures(row)
        .map(|c| decode_entities(c[1].trim()))
        .filter(|v| !v.is_empty())
}

/// A listing page: its total, then a row per arrêt it links a PDF for. `None` when the page
/// states no total, which no listing omits.
pub fn parse_listing(html: &str) -> Option<Listing> {
    let total = TOTAL.captures(html)?[1].parse().ok()?;
    let rows = html
        .split("<li class=\"views-row")
        .skip(1)
        .filter_map(|row| {
            let head = HEAD.captures(row)?;
            Some(Row {
                number: head[2].parse().ok()?,
                date: head[3].to_string(),
                url: head[1].to_string(),
                procedure: value(&PROCEDURE, row),
                country: value(&COUNTRY, row),
                composition: value(&COMPOSITION, row),
                judge: value(&JUDGE, row),
            })
        })
        .collect();
    Some(Listing { total, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<div class="view-header">
      Nombre de résultats: 788 (251 - 300)    </div>
<div class="item-list">    <ol>          <li class="views-row views-row-1 views-row-odd views-row-first">
          <h2 class="field-content"><a href="https://www.rvv-cce.be/sites/default/files/arr/a353949.an_.pdf">Arrêt 353949 de <span class="date-display-single" property="dc:date" datatype="xsd:dateTime" content="2026-10-02T00:00:00+02:00">02/10/2026</span></a></h2>
  <div class="views-field views-field-field-procedure">    Procédure:     <strong class="field-content">Annulation</strong>  </div>
      Pays d&#039;origine:     <strong>Côte d&#039;Ivoire</strong>
  <div class="views-field views-field-field-composition">    Composition:     <strong class="field-content">Assemblée Générale</strong>  </div>
  <div class="views-field views-field-field-judge">    Juge-rapporteur:     <strong class="field-content">DE BRUYN D.</strong>  </div>
            </li>
          <li class="views-row views-row-2 views-row-even">
          <h2 class="field-content"><a href="https://www.rvv-cce.be/sites/default/files/arr/A21149.AN.pdf">Arrêt 21149 de <span class="date-display-single" property="dc:date" datatype="xsd:dateTime" content="2008-12-30T00:00:00+01:00">30/12/2008</span></a></h2>
  <div class="views-field views-field-field-procedure">    Procédure:     <strong class="field-content">Plein contentieux</strong>  </div>
            </li>
      </ol></div>
<li class="leaf"><div class="facetapi-facet facetapi-inactive"><a href="/fr/arr/lang/french/date/2008/date/2008-01" rel="nofollow" class="facetapi-inactive">Janvier 2008 <span class="facetcount">37</span><span class="element-invisible"> Apply Janvier 2008 filter </span></a></div></li>
<li class="leaf"><div class="facetapi-facet facetapi-inactive"><a href="/fr/arr/lang/french/date/2008" rel="nofollow" class="facetapi-inactive">2008 <span class="facetcount">2676</span></a></div></li>"#;

    #[test]
    fn a_listing_page_gives_its_total_and_each_arret_s_fields() {
        let listing = parse_listing(PAGE).unwrap();
        assert_eq!(listing.total, 788);
        assert_eq!(listing.pages(), 16);
        assert_eq!(
            listing.rows,
            vec![
                Row {
                    number: 353_949,
                    date: "2026-10-02".into(),
                    url: "https://www.rvv-cce.be/sites/default/files/arr/a353949.an_.pdf".into(),
                    procedure: Some("Annulation".into()),
                    country: Some("Côte d'Ivoire".into()),
                    composition: Some("Assemblée Générale".into()),
                    judge: Some("DE BRUYN D.".into()),
                },
                Row {
                    number: 21_149,
                    date: "2008-12-30".into(),
                    url: "https://www.rvv-cce.be/sites/default/files/arr/A21149.AN.pdf".into(),
                    procedure: Some("Plein contentieux".into()),
                    country: None,
                    composition: None,
                    judge: None,
                },
            ]
        );
        assert_eq!(parse_listing("<html>Un instant</html>"), None);
    }

    #[test]
    fn the_facets_under_a_path_are_its_years_or_its_months() {
        assert_eq!(
            facets(PAGE, "/fr/arr/lang/french/date/2008/date/"),
            vec![("2008-01".to_string(), 37)]
        );
        assert_eq!(
            facets(PAGE, "/fr/arr/lang/french/date/"),
            vec![("2008".to_string(), 2676)]
        );
    }
}
