//! Juportal (`juportal.be`), the Belgian judiciary's portal: the ECLIs its RSS feeds list and the
//! decision page of each, read one at a time. And the ECLIs the European ECLI search engine lists
//! for Belgium, with the key its own page loads.

use std::io::Write;
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::NaiveDate;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://juportal.be";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(2);
/// The most items a feed carries; a range that reaches it is read again in halves.
pub const FEED_LIMIT: usize = 1_000;
pub const ECLI_CONFIG_URL: &str = "https://webgate.ec.europa.eu/ecli/assets/env-json-config.json";
pub const ECLI_SEARCH_URL: &str = "https://api.tech.ec.europa.eu/ecli/search";
/// The most results an ECLI search page carries.
pub const ECLI_PAGE: usize = 100;

/// The pauses before a request is sent again after a transport error, a dropped connection or a
/// gateway timeout; a refusal is not sent again.
const RETRY_PAUSES: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

/// An answer read whole.
struct Answer {
    status: u16,
    url: String,
    body: String,
}

impl Answer {
    /// The body of a 2xx answer.
    fn ok(self, host: &str) -> Result<String> {
        if !(200..300).contains(&self.status) {
            return Err(SourceError::Invalid(format!(
                "{host} answers {} on {}",
                self.status, self.url
            )));
        }
        Ok(self.body)
    }

    fn json<T: DeserializeOwned>(self, host: &str) -> Result<T> {
        let url = self.url.clone();
        serde_json::from_str(&self.ok(host)?)
            .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }
}

/// One request at a time to a host, at least [`PACE`] apart, and twice as long as the last answer
/// took when the host answers slowly.
struct Pacer(Mutex<Option<(Instant, Duration)>>);

impl Pacer {
    fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Sends `request` and reads its answer whole, again after each of [`RETRY_PAUSES`] when the
    /// connection fails. A 401, 403, 429 or 503 is a refusal and stops the caller.
    async fn send(&self, host: &str, request: reqwest::RequestBuilder) -> Result<Answer> {
        let mut last = self.0.lock().await;
        let mut pauses = RETRY_PAUSES.iter();
        loop {
            if let Some((at, took)) = *last {
                tokio::time::sleep(PACE.max(took * 2).saturating_sub(at.elapsed())).await;
            }
            let started = Instant::now();
            let read = Self::read(
                request
                    .try_clone()
                    .expect("a request with no streamed body"),
            )
            .await;
            *last = Some((Instant::now(), started.elapsed()));
            match read {
                Ok(answer) if matches!(answer.status, 401 | 403 | 429 | 503) => {
                    return Err(SourceError::Invalid(format!(
                        "{host} answers {} on {}",
                        answer.status, answer.url
                    )));
                }
                Ok(answer) => return Ok(answer),
                Err(e) => match pauses.next() {
                    Some(pause) if !e.is_builder() => {
                        tracing::warn!(host, error = %e, pause_s = pause.as_secs(), "transport error, sent again after a pause");
                        tokio::time::sleep(*pause).await;
                    }
                    _ => return Err(e.into()),
                },
            }
        }
    }

    async fn read(request: reqwest::RequestBuilder) -> reqwest::Result<Answer> {
        let response = request.send().await?;
        let status = response.status().as_u16();
        let url = response.url().to_string();
        Ok(Answer {
            status,
            url,
            body: response.text().await?,
        })
    }
}

fn http(proxy: &str, host: &str) -> Result<reqwest::ClientBuilder> {
    if proxy.is_empty() {
        return Err(SourceError::Invalid(format!(
            "{host} is read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
        )));
    }
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .proxy(reqwest::Proxy::all(proxy)?)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120)))
}

/// A PHP-serialised string, as the portal's saved searches write their criteria.
fn php_string(s: &str) -> String {
    format!("s:{}:\"{s}\";", s.len())
}

/// The feed of the decisions in `language`, `FR` or `NL`, the portal published from `from` to `to`,
/// both inclusive: the criteria of a search, PHP-serialised, deflated and in base64, as the portal's
/// « Fil RSS » link writes them.
pub fn feed_url(from: NaiveDate, to: NaiveDate, language: &str) -> String {
    let on = |code: &str| Some(if code == language { "on" } else { "" });
    let (from, to) = (from.to_string(), to.to_string());
    let limit = FEED_LIMIT.to_string();
    let criteria: [(&str, Option<&str>); 53] = [
        ("CRITTEXPRESSION", Some("")),
        ("CRITTRECHTEXTE", Some("on")),
        ("CRITTRECHECLITITRE", Some("")),
        ("CRITTRECHMOCLELIB", Some("")),
        ("CRITTRECHRESUME", Some("")),
        ("CRITTRECHNOTE", Some("")),
        ("CRITTRECHPUBLIC", Some("")),
        ("CRITTRECHLANGNL", on("NL")),
        ("CRITTRECHLANGFR", on("FR")),
        ("CRITTRECHLANGDE", Some("")),
        ("CRITTRECHFIELDLAW", None),
        ("CRITTRECHECLITYPEDEC", None),
        ("CRITTRECHJUSTELJURID", None),
        ("CRITTRECHSELANNEE", None),
        ("CRITTRECHECLINUMERO", Some("")),
        ("CRITTRECHECLICOUR", Some("")),
        ("CRITTRECHECLIANNEE", Some("")),
        ("CRITTRECHECLIORDRE", Some("")),
        ("CRITTRECHECLIADDLINK", Some("")),
        ("CRITTRECHNOROLE", Some("")),
        ("CRITTRECHDECISIONDE", Some("")),
        ("CRITTRECHDECISIONA", Some("")),
        ("CRITTRECHPUBLICATDE", Some(&from)),
        ("CRITTRECHPUBLICATA", Some(&to)),
        ("CRITTRECHTHESCAS1", None),
        ("CRITTRECHTHESCASID", None),
        ("CRITTRECHTHESUTU1", None),
        ("CRITTRECHTHESUTUID", None),
        ("CRITTRECHBASELEGALE", None),
        ("CRITTRECHBASELEGDATE", Some("")),
        ("CRITTRECHBASELEGNUM", Some("")),
        ("CRITTRECHBASELEGART", Some("")),
        ("CRITTRECHMODE", Some("BOOLEAN")),
        ("CRITTRECHOPER", Some("AND")),
        ("CRITTRECHSCORE", Some("0")),
        ("CRITTRECHLIMIT", Some(&limit)),
        ("CRITTRECHNPPAGE", Some("20")),
        ("CRITTRECHBALLOON", Some("")),
        ("CRITTRECHHILIGHT", Some("")),
        ("CRITTRECHSHOWRESUME", Some("")),
        ("CRITTRECHSHOWTHECAS", Some("")),
        ("CRITTRECHSHOWTHEUTU", Some("")),
        ("CRITTRECHSHOWMOTLIB", Some("")),
        ("CRITTRECHSHOWBASLEG", Some("")),
        ("CRITTRECHSHOWFICHES", Some("ALL")),
        ("CRITTRECHORDER", Some("DATEPUB")),
        ("CRITTRECHDESCASC", Some("DESC")),
        ("FILTRE_CHECKECLICOUR", None),
        ("FILTRE_CHECKECLITYPEDOC", None),
        ("FILTRE_CHECKECLIANNEE", None),
        ("FILTRE_CHECKFIELDLAW", None),
        ("CRITTRECHLANGUE", Some("FR")),
        ("CRITTRECHNOM", Some("JUPORTARECH")),
    ];
    let mut serialised = format!("a:{}:{{", criteria.len());
    for (key, value) in criteria {
        serialised.push_str(&php_string(key));
        match value {
            Some(v) => serialised.push_str(&php_string(v)),
            None => serialised.push_str("a:0:{}"),
        }
    }
    serialised.push('}');
    let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    deflate
        .write_all(serialised.as_bytes())
        .expect("a write into memory");
    let deflated = deflate.finish().expect("a write into memory");
    format!(
        "{BASE_URL}/moteur/Flux-RSS.rss?{}",
        base64::engine::general_purpose::STANDARD.encode(deflated)
    )
}

pub struct JuportalClient {
    http: reqwest::Client,
    pacer: Pacer,
}

impl JuportalClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            http: http(proxy, "juportal.be")?.build()?,
            pacer: Pacer::new(),
        })
    }

    /// The ECLIs the portal published in French from `from` to `to`, at most [`FEED_LIMIT`].
    pub async fn feed(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        language: &str,
    ) -> Result<Vec<String>> {
        let url = feed_url(from, to, language);
        let xml = self
            .pacer
            .send("juportal.be", self.http.get(&url))
            .await?
            .ok("juportal.be")?;
        lj_parse::juportal_feed(&xml)
            .map_err(|e| SourceError::Invalid(format!("{e} ({from} to {to})")))
    }

    /// The page of a decision in `language`, `FR` or `NL`, `None` when the portal knows no such ECLI.
    /// A page that is no decision page is a challenge or a refusal, and stops the crawl.
    pub async fn page(&self, ecli: &str, language: &str) -> Result<Option<String>> {
        let url = format!("{BASE_URL}/content/{ecli}/{language}");
        let Answer { status, body, .. } =
            self.pacer.send("juportal.be", self.http.get(&url)).await?;
        if matches!(status, 400 | 404) && body.contains("Invalid ECLI") {
            return Ok(None);
        }
        if status != 200 || !lj_parse::is_juportal_page(&body) {
            return Err(SourceError::Invalid(format!(
                "juportal.be answers {status} with no decision page on {url}"
            )));
        }
        Ok(Some(body))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcliAnswer {
    total_results: usize,
    #[serde(default)]
    search_result_item_list: Vec<EcliItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcliItem {
    ecli_identifier: String,
    #[serde(default)]
    available_languages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Facets {
    facets_list: Vec<Facet>,
}

#[derive(Deserialize)]
struct Facet {
    name: String,
    values: Vec<FacetValue>,
}

#[derive(Deserialize)]
struct FacetValue {
    value: String,
    count: usize,
}

/// A page of the Belgian decisions the ECLI search engine lists.
pub struct EcliPage {
    pub total: usize,
    /// Each ECLI with every language the portal holds its decision in.
    pub items: Vec<(String, Vec<String>)>,
}

pub struct EcliClient {
    http: reqwest::Client,
    pacer: Pacer,
    key: String,
}

fn criteria(languages: &[&str], extra: Value) -> Value {
    let mut criteria = vec![
        json!({"name": "agreementCheckbox", "value": "true", "multipleValuesList": null}),
        json!({"name": "organisations", "value": null, "multipleValuesList": ["BE"]}),
        json!({"name": "referenceType", "value": "Any", "multipleValuesList": null}),
        json!({"name": "referenceRelation", "value": "Any", "multipleValuesList": null}),
        json!({"name": "languages", "value": null, "multipleValuesList": languages}),
    ];
    if let Value::Array(extra) = extra {
        criteria.extend(extra);
    }
    Value::Array(criteria)
}

impl EcliClient {
    /// Reads the key the search engine's own page loads, which turns over.
    pub async fn new(proxy: &str) -> Result<Self> {
        // Asked for gzip, the search engine ends its answer by closing the connection without a TLS
        // close_notify, which rustls reads as a truncation; uncompressed, it answers chunked.
        let http = http(proxy, "the ECLI search engine")?.no_gzip().build()?;
        let pacer = Pacer::new();
        let config: Value = pacer
            .send("webgate.ec.europa.eu", http.get(ECLI_CONFIG_URL))
            .await?
            .json("webgate.ec.europa.eu")?;
        let key = config
            .get("apiKey")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| SourceError::Invalid(format!("no apiKey in {ECLI_CONFIG_URL}")))?
            .to_string();
        Ok(Self { http, pacer, key })
    }

    async fn post<T: DeserializeOwned>(&self, url: &str, body: &Value) -> Result<T> {
        let request = self
            .http
            .post(url)
            .header("apikey", &self.key)
            .header("Accept", "application/json")
            .json(body);
        self.pacer
            .send("api.tech.ec.europa.eu", request)
            .await?
            .json("api.tech.ec.europa.eu")
    }

    /// The years Belgium's decisions in French or Dutch were given in, with their counts.
    pub async fn years(&self) -> Result<Vec<(i32, usize)>> {
        let body = json!({
            "interfaceLanguage": "fr",
            "criteria": criteria(&["FR", "NL"], json!([
                {"name": "publicationFromDate", "value": "01/01/1900", "multipleValuesList": null}
            ])),
            "pageSize": 1,
            "pageNumber": 1,
            "searchType": "ADVANCED_SEARCH",
            "contentLanguage": "fr",
        });
        let facets: Facets = self
            .post(&format!("{ECLI_SEARCH_URL}/facets"), &body)
            .await?;
        let years = facets
            .facets_list
            .into_iter()
            .find(|f| f.name == "DATE-YEAR")
            .ok_or_else(|| SourceError::Invalid("ECLI facets without DATE-YEAR".into()))?;
        years
            .values
            .into_iter()
            .map(|v| {
                v.value
                    .parse()
                    .map(|year| (year, v.count))
                    .map_err(|e| SourceError::Invalid(format!("ECLI year {:?}: {e}", v.value)))
            })
            .collect()
    }

    /// Page `number`, from 1, of the Belgian decisions given in `year` the search engine lists under
    /// `language`, `FR` or `NL`. It lists a decision held in French alone under French, and one held
    /// in French and Dutch under Dutch.
    pub async fn page(&self, year: i32, language: &str, number: usize) -> Result<EcliPage> {
        let body = json!({
            "interfaceLanguage": "fr",
            "criteria": criteria(&[language], json!([
                {"name": "decisionFromDate", "value": format!("01/01/{year}"), "multipleValuesList": null},
                {"name": "decisionToDate", "value": format!("31/12/{year}"), "multipleValuesList": null}
            ])),
            "pageSize": ECLI_PAGE,
            "pageNumber": number,
            "searchType": "ADVANCED_SEARCH",
            "contentLanguage": "fr",
        });
        let answer: EcliAnswer = self.post(ECLI_SEARCH_URL, &body).await?;
        Ok(EcliPage {
            total: answer.total_results,
            items: answer
                .search_result_item_list
                .into_iter()
                .map(|i| (i.ecli_identifier.trim().to_string(), i.available_languages))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_feed_url_carries_the_portal_s_serialised_criteria_deflated() {
        let day = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        let url = feed_url(day("2026-11-10"), day("2026-11-14"), "FR");
        let query = url
            .strip_prefix("https://juportal.be/moteur/Flux-RSS.rss?")
            .unwrap();
        let deflated = base64::engine::general_purpose::STANDARD
            .decode(query)
            .unwrap();
        let mut serialised = String::new();
        flate2::read::ZlibDecoder::new(&deflated[..])
            .read_to_string(&mut serialised)
            .unwrap();
        assert!(serialised.starts_with("a:53:{s:15:\"CRITTEXPRESSION\";s:0:\"\";"));
        assert!(serialised.contains(
            "s:19:\"CRITTRECHPUBLICATDE\";s:10:\"2026-11-10\";s:18:\"CRITTRECHPUBLICATA\";s:10:\"2026-11-14\";"
        ));
        assert!(serialised.contains("s:20:\"CRITTRECHJUSTELJURID\";a:0:{}"));
        assert!(serialised
            .contains("s:15:\"CRITTRECHLANGNL\";s:0:\"\";s:15:\"CRITTRECHLANGFR\";s:2:\"on\";"));
        assert!(serialised.ends_with("s:12:\"CRITTRECHNOM\";s:11:\"JUPORTARECH\";}"));
    }
}
