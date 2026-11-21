//! JuriCAF (`juricaf.org`): the decision pages its imports page and its search listings name, read
//! one at a time.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use regex::Regex;
use reqwest::StatusCode;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://juricaf.org";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(2);

/// The decisions of one country the site imported on one day, as its imports page counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub day: NaiveDate,
    pub country: String,
    pub count: usize,
}

/// A court and its number of decisions, as the statistics page counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Court {
    pub country: String,
    pub court: String,
    pub count: usize,
}

/// A search over one country's decisions, or one court's, by the day the site imported them and by
/// their date, both inclusive.
#[derive(Debug, Clone, Copy)]
pub struct Search<'a> {
    pub country: &'a str,
    pub court: Option<&'a str>,
    pub imported: Option<NaiveDate>,
    pub decided: Option<(NaiveDate, NaiveDate)>,
}

impl Search<'_> {
    fn path(&self) -> String {
        let mut clauses = Vec::new();
        if let Some(day) = self.imported {
            clauses.push(format!("date_import:[{day} TO {day}]"));
        }
        if let Some((from, to)) = self.decided {
            clauses.push(format!("date_arret:[{from} TO {to}]"));
        }
        let query = if clauses.is_empty() {
            "+".to_string()
        } else {
            segment(&clauses.join(" "))
        };
        let mut facet = format!("facet_pays:{}", self.country.replace(' ', "_"));
        if let Some(court) = self.court {
            facet.push_str(&format!(",facet_juridiction:{}", court.replace(' ', "_")));
        }
        let facet = segment(&facet);
        format!("{BASE_URL}/recherche/{query}/{facet}")
    }
}

fn segment(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// The decisions a search lists.
#[derive(Debug, PartialEq, Eq)]
pub struct Listing {
    /// How many decisions the search matches.
    pub total: usize,
    /// How many the response carries, fewer than `total` past the site's cap.
    pub returned: usize,
    pub slugs: Vec<String>,
}

pub struct JuricafClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl JuricafClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "juricaf.org is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
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

    /// A refusal or a sign of load (403, 429, 503) stops the crawl rather than waiting it out.
    async fn send(&self, url: &str) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep(PACE.saturating_sub(at.elapsed())).await;
        }
        let response = self.http.get(url).send().await;
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
        Ok(self.send(url).await?.error_for_status()?.text().await?)
    }

    /// The imports of the 30 days before `until`, that day included.
    pub async fn imports(&self, until: NaiveDate) -> Result<Vec<Import>> {
        import_rows(
            &self
                .get(&format!("{BASE_URL}/stats/imports/{until}"))
                .await?,
        )
    }

    /// Every court of every country.
    pub async fn courts(&self) -> Result<Vec<Court>> {
        stats_courts(&self.get(&format!("{BASE_URL}/stats")).await?)
    }

    /// How many decisions a search matches, read from its feed, which carries 30 of them.
    pub async fn count(&self, search: &Search<'_>) -> Result<usize> {
        let url = format!("{}?format=rss", search.path());
        feed_total(&self.get(&url).await?)
            .ok_or_else(|| SourceError::Invalid(format!("juricaf feed without its total: {url}")))
    }

    /// The decisions a search matches, up to the site's cap.
    pub async fn listing(&self, search: &Search<'_>) -> Result<Listing> {
        let url = format!("{}?format=json", search.path());
        listing(&self.get(&url).await?).ok_or_else(|| {
            SourceError::Invalid(format!("juricaf listing without its total: {url}"))
        })
    }

    /// A decision page, `None` when gone though a listing names it.
    pub async fn page(&self, slug: &str) -> Result<Option<String>> {
        let response = self.send(&format!("{BASE_URL}/arret/{slug}")).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.text().await?))
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("juricaf regex"))
}

fn digits(text: &str) -> usize {
    text.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a run of digits")
}

/// The page names each country once a day, then its count, both linked to that day's listing.
fn import_rows(html: &str) -> Result<Vec<Import>> {
    static ROW: OnceLock<Regex> = OnceLock::new();
    if !html.contains("Imports de décisions") {
        return Err(SourceError::Invalid(
            "juricaf imports page without its table".into(),
        ));
    }
    let row = regex(
        &ROW,
        r#"<a href="/recherche/date_import%3A(\d{4}-\d{2}-\d{2})/facet_pays%3A[^"]*">([^<]+)</a>\s*</li>\s*</td>\s*<td>\s*<a href="[^"]*">(\d+)</a>"#,
    );
    row.captures_iter(html)
        .map(|c| {
            Ok(Import {
                day: NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").map_err(|e| {
                    SourceError::Invalid(format!("juricaf import day {}: {e}", &c[1]))
                })?,
                country: html_escape::decode_html_entities(c[2].trim()).into_owned(),
                count: digits(&c[3]),
            })
        })
        .collect()
}

fn stats_courts(html: &str) -> Result<Vec<Court>> {
    static COUNTRY: OnceLock<Regex> = OnceLock::new();
    static COURT: OnceLock<Regex> = OnceLock::new();
    let country = regex(
        &COUNTRY,
        r#"(?s)<strong><a href="/recherche/\+\?facet_facets=pays%3A[^"]*">([^<]+)</a></strong>\s*<ul class="subul">(.*?)</ul>"#,
    );
    let court = regex(&COURT, r"<a [^>]*>([^<(]+?) \((\d[\d\s]*) décisions? de");
    let mut courts = Vec::new();
    for c in country.captures_iter(html) {
        let name = html_escape::decode_html_entities(c[1].trim()).into_owned();
        let list = c.get(2).map_or("", |m| m.as_str());
        let before = courts.len();
        courts.extend(court.captures_iter(list).map(|k| Court {
            country: name.clone(),
            court: html_escape::decode_html_entities(k[1].trim()).into_owned(),
            count: digits(&k[2]),
        }));
        if courts.len() - before != list.matches("<a ").count() {
            return Err(SourceError::Invalid(format!(
                "juricaf statistics page: a court of {name} reads as no count"
            )));
        }
    }
    if courts.is_empty() {
        return Err(SourceError::Invalid(
            "juricaf statistics page without a court".into(),
        ));
    }
    Ok(courts)
}

fn feed_total(xml: &str) -> Option<usize> {
    static TOTAL: OnceLock<Regex> = OnceLock::new();
    regex(&TOTAL, r"sur un total de (\d+) résultats")
        .captures(xml)
        .map(|c| digits(&c[1]))
}

/// The listing is JSON written by hand, its titles unescaped, so its ids are read by pattern.
fn listing(json: &str) -> Option<Listing> {
    static TOTAL: OnceLock<Regex> = OnceLock::new();
    static ID: OnceLock<Regex> = OnceLock::new();
    let total = regex(&TOTAL, r#"^\s*\{\s*"nb_resultat"\s*:\s*(\d+)"#).captures(json)?;
    let ids: Vec<&str> = regex(&ID, r#"\{\s*"id"\s*:\s*"([^"]*)""#)
        .captures_iter(json)
        .map(|c| c.get(1).map_or("", |m| m.as_str()))
        .collect();
    Some(Listing {
        total: digits(&total[1]),
        returned: ids.len(),
        slugs: ids
            .into_iter()
            .filter(|id| is_slug(id))
            .map(str::to_string)
            .collect(),
    })
}

/// A slug becomes a file name, so one outside `[A-Za-z0-9_-]` is not taken.
fn is_slug(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_imports_page_yields_each_country_s_count_by_day() {
        let html = r#"<p>Imports de décisions de cours suprême des 30 jours précédents le <span>vendredi 13 novembre 2026</span>.</p>
            <tr><th class="import-date" colspan="2"> <a href="/stats/imports/2026-11-13">2026-11-13</a></th>
                <tr style="font-weight:300;">
                  <td>
                    <li style="list-style-position:inside; list-style-image: url(/images/drapeaux/Suisse.png)"><strong><a href="/recherche/date_import%3A2026-11-13/facet_pays%3ASuisse">Suisse</a></li>                  </td>
                  <td>
                    <a href="/recherche/date_import%3A2026-11-13/facet_pays%3ASuisse">3</a>                  </td>
                </tr>
          </tr>
                    <tr>
            </th>
                <tr style="font-weight:300;">
                  <td>
                    <li style="list-style-position:inside; list-style-image: url(/images/drapeaux/C%C3%B4te_d_Ivoire.png)"><strong><a href="/recherche/date_import%3A2026-11-06/facet_pays%3AC%C3%B4te_d%27Ivoire">Côte d&#039;Ivoire</a></li>                  </td>
                  <td>
                    <a href="/recherche/date_import%3A2026-11-06/facet_pays%3AC%C3%B4te_d%27Ivoire">26</a>                  </td>
                </tr>
          <a href="/stats/imports/2026-10-14"><span class="btn btn-primary import-plus">Voir plus</span></a>"#;
        let day = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        assert_eq!(
            import_rows(html).unwrap(),
            [
                Import {
                    day: day("2026-11-13"),
                    country: "Suisse".into(),
                    count: 3
                },
                Import {
                    day: day("2026-11-06"),
                    country: "Côte d'Ivoire".into(),
                    count: 26
                },
            ]
        );
        assert!(import_rows("<html>Imports sous 30 jours</html>").is_err());
    }

    #[test]
    fn the_statistics_page_yields_each_court_with_its_country() {
        let html = r#"<li style="list-style-image: url(/images/drapeaux/Burkina_Faso.png)"><strong><a href="/recherche/+?facet_facets=pays%3ABurkina_Faso">Burkina Faso</a></strong><ul class="subul"><li><a href="/recherche/+/facet_pays%3ABurkina_Faso%2Cfacet_juridiction%3ACour_d%27appel">Cour d'appel (98 décisions de 2005 à 2012)</a></li></ul></li><li style="list-style-image: url(/images/drapeaux/Luxembourg.png)"><strong><a href="/recherche/+?facet_facets=pays%3ALuxembourg">Luxembourg</a></strong><ul class="subul"><li><a href="/recherche/+/facet_pays%3ALuxembourg%2Cfacet_juridiction%3ATribunal_administratif">Tribunal administratif (19 017 décisions de 1997 à du 2)</a></li><li><a href="/recherche/+/facet_pays%3ALuxembourg%2Cfacet_juridiction%3AAutorit%C3%A9">Autorité de protection des données (1 décision de 2022)</a></li></ul></li>"#;
        let court = |country: &str, court: &str, count| Court {
            country: country.into(),
            court: court.into(),
            count,
        };
        assert_eq!(
            stats_courts(html).unwrap(),
            [
                court("Burkina Faso", "Cour d'appel", 98),
                court("Luxembourg", "Tribunal administratif", 19_017),
                court("Luxembourg", "Autorité de protection des données", 1),
            ]
        );
    }

    #[test]
    fn a_listing_yields_its_ids_though_a_title_breaks_its_json() {
        let json = r#"{ "nb_resultat" : 26, "docs" : [ { "id" : "BELGIQUE-COURCONSTITUTIONNELLE-20261001-1182026", "pays" : "Belgique", "titre" : "Belgique, Cour constitutionnelle, 01 octobre 2026, 118/2026", "formation" : "", "date_arret" : "2026-10-01T12:00:00Z", "juridiction" : "Cour constitutionnelle" }, { "id" : "BELGIQUE-COURDECASSATION-20240620-P230116F", "pays" : "Belgique", "titre" : "Belgique, Cour de cassation, 20 juin 2024, "Le Ciel" c/ X", "formation" : "", "date_arret" : "2024-06-20T12:00:00Z", "juridiction" : "Cour de cassation" }, { "id" : "../etc", "pays" : "Belgique", "titre" : "", "formation" : "", "date_arret" : "", "juridiction" : "" } ] }"#;
        assert!(serde_json::from_str::<serde_json::Value>(json).is_err());
        assert_eq!(
            listing(json),
            Some(Listing {
                total: 26,
                returned: 3,
                slugs: vec![
                    "BELGIQUE-COURCONSTITUTIONNELLE-20261001-1182026".into(),
                    "BELGIQUE-COURDECASSATION-20240620-P230116F".into(),
                ],
            })
        );
        assert_eq!(listing("<html>Erreur</html>"), None);
    }

    #[test]
    fn a_search_names_its_clauses_and_country_as_the_site_reads_them() {
        let day = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        let search = Search {
            country: "Côte d'Ivoire",
            court: None,
            imported: Some(day("2026-11-06")),
            decided: Some((day("0001-01-01"), day("2002-12-31"))),
        };
        assert_eq!(
            search.path(),
            "https://juricaf.org/recherche/date_import%3A%5B2026-11-06%20TO%202026-11-06%5D%20date_arret%3A%5B0001-01-01%20TO%202002-12-31%5D/facet_pays%3AC%C3%B4te_d%27Ivoire"
        );
        let all = Search {
            country: "Belgique",
            court: None,
            imported: None,
            decided: None,
        };
        assert_eq!(
            all.path(),
            "https://juricaf.org/recherche/+/facet_pays%3ABelgique"
        );
        let court = Search {
            court: Some("Cour de cassation"),
            ..all
        };
        assert_eq!(
            court.path(),
            "https://juricaf.org/recherche/+/facet_pays%3ABelgique%2Cfacet_juridiction%3ACour_de_cassation"
        );
        assert_eq!(
            feed_total("<description><![CDATA[Les 30 arrêts les plus récents de cette collection (sur un total de 11291 résultats).]]></description>"),
            Some(11_291)
        );
    }
}
