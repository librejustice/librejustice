//! Fedlex, the Swiss Confederation's publication platform: the acts of its systematic collection
//! and the consolidated states of each from its SPARQL endpoint, and each state's Akoma Ntoso file,
//! through the egress proxy.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::publisher::{literal_day, Consolidation, PacedClient};

pub const SPARQL_URL: &str = "https://fedlex.data.admin.ch/sparqlendpoint";

static RS_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+(\.[0-9]+)*$").unwrap());
static MANIFESTATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-xml-(\d+)\.xml$").unwrap());

#[derive(Deserialize)]
struct Row {
    lang: Value,
    date: Value,
    url: Value,
    modified: Option<Value>,
}

#[derive(Deserialize)]
struct Value {
    value: String,
}

#[derive(Deserialize)]
struct ListedRow {
    #[serde(rename = "abstract")]
    work: Value,
    rs: Value,
    lang: Value,
    title: Value,
    short: Option<Value>,
}

#[derive(Deserialize)]
struct ChangedRow {
    rs: Value,
}

#[derive(Deserialize)]
struct CompilationRow {
    act: Value,
    kind: Value,
    published: Option<Value>,
    adopted: Option<Value>,
    title: Option<Value>,
}

/// An act of the official compilation (RO) that published or changed an act in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilationAct {
    /// Its path under `eli/oc/`: `2021/747`, `24/233_245_233`.
    pub path: String,
    pub published: Option<String>,
    pub adopted: Option<String>,
    pub title: Option<String>,
    /// `basic` when it publishes the act, else each of Fedlex's impact types it has on it.
    pub kinds: Vec<String>,
}

const OC: &str = "https://fedlex.data.admin.ch/eli/oc/";
const IMPACT_TYPE: &str = "https://fedlex.data.admin.ch/vocabulary/impact-type/";

/// An act of the systematic collection, with its title and abbreviation in each official language
/// Fedlex gives (`de`, `fr`, `it`).
pub struct ListedAct {
    pub rs: String,
    /// Its ELI in the systematic collection, `https://fedlex.data.admin.ch/eli/cc/24/233_245_233`.
    pub eli: String,
    pub titles: BTreeMap<&'static str, (String, Option<String>)>,
}

const LANGUAGES: [(&str, &str); 3] = [("DEU", "de"), ("FRA", "fr"), ("ITA", "it")];
const LANGUAGE_URI: &str = "http://publications.europa.eu/resource/authority/language/";

pub struct FedlexClient {
    paced: PacedClient,
}

impl FedlexClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "fedlex.data.admin.ch")?,
        })
    }

    /// The states of the act in force numbered `rs` in the systematic collection that have a
    /// French Akoma Ntoso file, oldest first, the latest file of a day when Fedlex holds several,
    /// each with the day its consolidation was last modified.
    pub async fn consolidations(&self, rs: &str) -> Result<Vec<Consolidation>> {
        Ok(self
            .expressions(rs, &["fr"])
            .await?
            .remove("fr")
            .unwrap_or_default())
    }

    /// The states of the act in force numbered `rs` in each of `languages` (`de`, `fr`, `it`)
    /// that has an Akoma Ntoso file in it, as [`Self::consolidations`] gives the French ones.
    pub async fn expressions(
        &self,
        rs: &str,
        languages: &[&str],
    ) -> Result<BTreeMap<&'static str, Vec<Consolidation>>> {
        if !RS_NUMBER.is_match(rs) {
            return Err(SourceError::Invalid(format!("not an RS number: {rs:?}")));
        }
        let uris = languages
            .iter()
            .map(|l| {
                LANGUAGES
                    .iter()
                    .find(|(_, code)| code == l)
                    .map(|(uri, _)| format!("<{LANGUAGE_URI}{uri}>"))
                    .ok_or_else(|| SourceError::Invalid(format!("not a Fedlex language: {l:?}")))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let query = format!(
            r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT ?lang ?date ?url ?modified WHERE {{
  ?abstract jolux:historicalLegalId "{rs}" ;
    jolux:inForceStatus <https://fedlex.data.admin.ch/vocabulary/enforcement-status/0> .
  ?cons jolux:isMemberOf ?abstract ; jolux:dateApplicability ?date ; jolux:isRealizedBy ?expr .
  ?expr jolux:language ?lang ; jolux:isEmbodiedBy ?man .
  FILTER(?lang IN ({uris}))
  ?man jolux:userFormat <https://fedlex.data.admin.ch/vocabulary/user-format/xml> ; jolux:isExemplifiedBy ?url .
  OPTIONAL {{ ?cons <http://purl.org/dc/terms/modified> ?modified }}
}}"#
        );
        let rows: Vec<Row> = self.select(&query).await.map_err(|e| match e {
            SourceError::Invalid(m) => SourceError::Invalid(format!("RS {rs}: {m}")),
            e => e,
        })?;
        let mut rows: Vec<(&'static str, String, u32, String, Option<NaiveDate>)> = rows
            .into_iter()
            .map(|r| {
                let language = LANGUAGES
                    .iter()
                    .find(|(uri, _)| r.lang.value.strip_prefix(LANGUAGE_URI) == Some(uri))
                    .map(|(_, code)| *code)
                    .ok_or_else(|| {
                        SourceError::Invalid(format!("RS {rs}: language {:?}", r.lang.value))
                    })?;
                let rank = MANIFESTATION
                    .captures(&r.url.value)
                    .and_then(|c| c[1].parse().ok())
                    .unwrap_or(0);
                let modified = r.modified.map(|m| literal_day(&m.value)).transpose()?;
                Ok((language, r.date.value, rank, r.url.value, modified))
            })
            .collect::<Result<_>>()?;
        rows.sort();
        let mut out: BTreeMap<&'static str, Vec<Consolidation>> = BTreeMap::new();
        for (language, date, _, url, modified) in rows {
            let states = out.entry(language).or_default();
            match states.last_mut() {
                Some(last) if last.applies_from == date => {
                    last.xml_url = url;
                    last.modified = last.modified.max(modified);
                }
                _ => states.push(Consolidation {
                    applies_from: date,
                    xml_url: url,
                    modified,
                    token: None,
                }),
            }
        }
        Ok(out)
    }

    async fn select<T: serde::de::DeserializeOwned>(&self, query: &str) -> Result<Vec<T>> {
        let body = self
            .paced
            .send(|| {
                self.paced
                    .http
                    .post(SPARQL_URL)
                    .header("Accept", "application/sparql-results+json")
                    .form(&[("query", query)])
            })
            .await?;
        #[derive(Deserialize)]
        struct Answer<T> {
            results: Bindings<T>,
        }
        #[derive(Deserialize)]
        struct Bindings<T> {
            bindings: Vec<T>,
        }
        let answer: Answer<T> = serde_json::from_slice(&body)
            .map_err(|e| SourceError::Invalid(format!("fedlex SPARQL answer: {e}")))?;
        Ok(answer.results.bindings)
    }

    /// The internal acts of the systematic collection in force, the treaties (`0.x`) left out.
    /// One unsorted query: the endpoint refuses to sort past 10,000 rows, and the three languages
    /// give about 5,400.
    pub async fn index(&self) -> Result<Vec<ListedAct>> {
        let languages = LANGUAGES
            .iter()
            .map(|(code, _)| format!("<{LANGUAGE_URI}{code}>"))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT DISTINCT ?abstract ?rs ?lang ?title ?short WHERE {{
  ?abstract a jolux:ConsolidationAbstract ; jolux:historicalLegalId ?rs ;
    jolux:inForceStatus <https://fedlex.data.admin.ch/vocabulary/enforcement-status/0> ;
    jolux:isRealizedBy ?expr .
  FILTER(!STRSTARTS(?rs, "0."))
  ?expr jolux:language ?lang ; jolux:title ?title .
  FILTER(?lang IN ({languages}))
  OPTIONAL {{ ?expr jolux:titleShort ?short }}
}}"#
        );
        let rows: Vec<ListedRow> = self.select(&query).await?;
        let mut acts: BTreeMap<String, ListedAct> = BTreeMap::new();
        for r in rows {
            let Some(lang) = LANGUAGES
                .iter()
                .find(|(code, _)| r.lang.value.strip_prefix(LANGUAGE_URI) == Some(*code))
                .map(|(_, lang)| *lang)
            else {
                continue;
            };
            let act = acts
                .entry(r.work.value.clone())
                .or_insert_with(|| ListedAct {
                    rs: r.rs.value,
                    eli: r.work.value,
                    titles: BTreeMap::new(),
                });
            act.titles
                .entry(lang)
                .or_insert((r.title.value, r.short.map(|s| s.value)));
        }
        Ok(acts.into_values().collect())
    }

    /// The RS numbers of the acts with a consolidation modified on or after `since`.
    pub async fn changed_since(&self, since: NaiveDate) -> Result<HashSet<String>> {
        let query = format!(
            r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT DISTINCT ?rs WHERE {{
  ?cons jolux:isMemberOf ?abstract ; <http://purl.org/dc/terms/modified> ?modified .
  ?abstract jolux:historicalLegalId ?rs .
  FILTER(?modified >= "{since}"^^<http://www.w3.org/2001/XMLSchema#date>)
}}"#
        );
        let rows: Vec<ChangedRow> = self.select(&query).await?;
        Ok(rows.into_iter().map(|r| r.rs.value).collect())
    }

    /// The RO acts of the act in force numbered `rs`: the one that published it, and those whose
    /// legal analysis names an impact on it or on one of its parts, ordered by path.
    pub async fn compilation_acts(&self, rs: &str) -> Result<Vec<CompilationAct>> {
        if !RS_NUMBER.is_match(rs) {
            return Err(SourceError::Invalid(format!("not an RS number: {rs:?}")));
        }
        let query = format!(
            r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT DISTINCT ?act ?kind ?published ?adopted ?title WHERE {{
  ?abstract jolux:historicalLegalId "{rs}" ;
    jolux:inForceStatus <https://fedlex.data.admin.ch/vocabulary/enforcement-status/0> .
  {{ ?abstract jolux:basicAct ?act . BIND("basic" AS ?kind) }}
  UNION
  {{ ?part jolux:legalResourceSubdivisionIsPartOf ?abstract .
    ?impact jolux:impactToLegalResource ?part ; jolux:legalResourceImpactHasType ?kind .
    ?analysis jolux:legalAnalysisHasLegalResourceImpact ?impact ; jolux:legalAnalysisOfLegalResource ?act . }}
  UNION
  {{ ?impact jolux:impactToLegalResource ?abstract ; jolux:legalResourceImpactHasType ?kind .
    ?analysis jolux:legalAnalysisHasLegalResourceImpact ?impact ; jolux:legalAnalysisOfLegalResource ?act . }}
  OPTIONAL {{ ?act jolux:publicationDate ?published }}
  OPTIONAL {{ ?act jolux:dateDocument ?adopted }}
  OPTIONAL {{ ?act jolux:isRealizedBy ?expr .
    ?expr jolux:language <{LANGUAGE_URI}FRA> ; jolux:title ?title }}
}}"#
        );
        let rows: Vec<CompilationRow> = self.select(&query).await?;
        let mut acts: BTreeMap<String, CompilationAct> = BTreeMap::new();
        for r in rows {
            let Some(path) = r.act.value.strip_prefix(OC) else {
                continue;
            };
            let kind = r
                .kind
                .value
                .strip_prefix(IMPACT_TYPE)
                .unwrap_or(&r.kind.value);
            let act = acts
                .entry(path.to_string())
                .or_insert_with(|| CompilationAct {
                    path: path.to_string(),
                    published: None,
                    adopted: None,
                    title: None,
                    kinds: Vec::new(),
                });
            act.published = act.published.take().or(r.published.map(|v| v.value));
            act.adopted = act.adopted.take().or(r.adopted.map(|v| v.value));
            act.title = act.title.take().or(r.title.map(|v| v.value));
            if !act.kinds.iter().any(|k| k == kind) {
                act.kinds.push(kind.to_string());
            }
        }
        let mut acts: Vec<CompilationAct> = acts.into_values().collect();
        for act in &mut acts {
            act.kinds.sort();
        }
        Ok(acts)
    }

    pub async fn xml(&self, url: &str) -> Result<String> {
        self.paced.text(url).await
    }
}
