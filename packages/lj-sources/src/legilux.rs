//! Legilux, the Luxembourg legislation portal: its acts in force and their consolidated states
//! from its SPARQL endpoint, and each state's Akoma Ntoso file, through the egress proxy.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::publisher::{literal_day, Consolidation, PacedClient};

pub const SPARQL_URL: &str = "https://data.legilux.public.lu/sparqlendpoint";
const ELI: &str = "http://data.legilux.public.lu/eli/etat/";
/// The files the data host lists, served by the portal's host.
const FILESTORE: (&str, &str) = (
    "http://data.legilux.public.lu/filestore/",
    "https://legilux.public.lu/filestore/",
);
/// The types of the acts read: laws, grand-ducal and ministerial regulations and orders, and the
/// Constitution.
const TYPES: [&str; 8] = [
    "LOI",
    "RGD",
    "AGD",
    "AMIN",
    "RMIN",
    "A",
    "ARGD",
    "Constitution",
];

static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z_]+$").unwrap());

#[derive(Deserialize)]
struct Answer {
    results: Bindings,
}

#[derive(Deserialize)]
struct Bindings {
    bindings: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    date: Value,
    url: Value,
    modified: Option<Value>,
}

#[derive(Deserialize)]
struct Value {
    value: String,
}

#[derive(Deserialize)]
struct Table {
    results: Rows,
}

#[derive(Deserialize)]
struct Rows {
    bindings: Vec<HashMap<String, Value>>,
}

/// An act in force, with its states: its consolidated ones, else the file it was published as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiluxWork {
    /// Its work's ELI after `eli/etat/leg/` (`loi/2005/02/04/n1`), after `eli/etat/` for an
    /// administrative act (`adm/amin/2018/10/11/b3533`), `CODE-<name>` for a code, `CONSTITUTION`.
    pub key: String,
    pub kind: String,
    pub title: String,
    pub adopted: Option<NaiveDate>,
    pub consolidated: bool,
    /// Oldest first; none when its only file is a scan.
    pub states: Vec<Consolidation>,
}

fn filestore(url: &str) -> String {
    url.replacen(FILESTORE.0, FILESTORE.1, 1)
}

pub struct LegiluxClient {
    paced: PacedClient,
}

impl LegiluxClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "data.legilux.public.lu")?,
        })
    }

    /// The states of the code `eli/etat/leg/code/<code>` with a French XML file, oldest first, each
    /// with the day that file was last modified.
    pub async fn consolidations(&self, code: &str) -> Result<Vec<Consolidation>> {
        if !CODE.is_match(code) {
            return Err(SourceError::Invalid(format!(
                "not a Legilux code: {code:?}"
            )));
        }
        let query = format!(
            r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT ?date ?url ?modified WHERE {{
  ?cons a jolux:Consolidation ; jolux:dateApplicability ?date ; jolux:isRealizedBy ?expr .
  FILTER(STRSTARTS(STR(?cons), "http://data.legilux.public.lu/eli/etat/leg/code/{code}/"))
  ?expr jolux:language <http://publications.europa.eu/resource/authority/language/FRA> ; jolux:isEmbodiedBy ?man .
  ?man jolux:userFormat <http://data.legilux.public.lu/resource/authority/user-format/xml> ; jolux:isExemplifiedBy ?url .
  OPTIONAL {{ ?man <http://purl.org/dc/terms/modified> ?modified }}
}}"#
        );
        let body = self
            .paced
            .send(|| {
                self.paced
                    .http
                    .post(SPARQL_URL)
                    .header("Accept", "application/sparql-results+json")
                    .form(&[("query", query.as_str())])
            })
            .await?;
        let answer: Answer = serde_json::from_slice(&body).map_err(|e| {
            SourceError::Invalid(format!("legilux SPARQL answer for code {code}: {e}"))
        })?;
        let mut rows: Vec<(String, String, Option<NaiveDate>)> = answer
            .results
            .bindings
            .into_iter()
            .map(|r| {
                let modified = r.modified.map(|m| literal_day(&m.value)).transpose()?;
                Ok((r.date.value, filestore(&r.url.value), modified))
            })
            .collect::<Result<_>>()?;
        rows.sort();
        let mut out: Vec<Consolidation> = Vec::new();
        for (date, url, modified) in rows {
            match out.last_mut() {
                Some(last) if last.applies_from == date => {
                    last.xml_url = url;
                    last.modified = last.modified.max(modified);
                }
                _ => out.push(Consolidation {
                    applies_from: date,
                    xml_url: url,
                    modified,
                    token: None,
                }),
            }
        }
        Ok(out)
    }

    pub async fn xml(&self, url: &str) -> Result<String> {
        self.paced.text(url).await
    }

    async fn table(&self, query: &str) -> Result<Vec<HashMap<String, String>>> {
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
        let table: Table = serde_json::from_slice(&body)
            .map_err(|e| SourceError::Invalid(format!("legilux SPARQL answer: {e}")))?;
        Ok(table
            .results
            .bindings
            .into_iter()
            .map(|row| row.into_iter().map(|(k, v)| (k, v.value)).collect())
            .collect())
    }

    /// Every act in force of [`TYPES`] with a French title, a code keyed by its name: a work
    /// consolidated with each French XML state, any other with the XML file it was published as,
    /// from its entry into force.
    pub async fn index(&self) -> Result<Vec<LegiluxWork>> {
        struct Code {
            name: String,
            title: String,
            day: String,
        }
        let mut consolidations: HashMap<String, Vec<(String, String, Option<NaiveDate>)>> =
            HashMap::new();
        let mut codes: HashMap<String, Code> = HashMap::new();
        for r in self
            .table(
                r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT ?c ?w ?date ?title ?url (MAX(?mod) AS ?modified) WHERE {
  ?c a jolux:Consolidation ; jolux:isMemberOf ?w ; jolux:dateApplicability ?date ; jolux:isRealizedBy ?e .
  ?e jolux:language <http://publications.europa.eu/resource/authority/language/FRA> ; jolux:title ?title ; jolux:isEmbodiedBy ?m .
  ?m jolux:userFormat <http://data.legilux.public.lu/resource/authority/user-format/xml> ; jolux:isExemplifiedBy ?url .
  OPTIONAL { ?m <http://purl.org/dc/terms/modified> ?mod }
} GROUP BY ?c ?w ?date ?title ?url"#,
            )
            .await?
        {
            let (Some(c), Some(w), Some(date), Some(url), Some(title)) = (
                r.get("c"),
                r.get("w"),
                r.get("date"),
                r.get("url"),
                r.get("title"),
            ) else {
                continue;
            };
            let modified = r.get("modified").map(|m| literal_day(m)).transpose()?;
            if let Some((name, _)) = c
                .strip_prefix(ELI)
                .and_then(|p| p.strip_prefix("leg/code/"))
                .and_then(|p| p.split_once('/'))
            {
                if codes.get(w).is_none_or(|code| *date > code.day) {
                    codes.insert(
                        w.clone(),
                        Code {
                            name: name.to_string(),
                            title: title.clone(),
                            day: date.clone(),
                        },
                    );
                }
            }
            consolidations
                .entry(w.clone())
                .or_default()
                .push((date.clone(), filestore(url), modified));
        }
        let mut works = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for kind in TYPES {
            for r in self
                .table(&format!(
                    r#"PREFIX jolux: <http://data.legilux.public.lu/resource/ontology/jolux#>
SELECT ?w ?title ?date ?force ?pub (SAMPLE(?u) AS ?url) (MAX(?mod) AS ?modified) WHERE {{
  ?act a jolux:Act ; jolux:typeDocument <http://data.legilux.public.lu/resource/authority/resource-type/{kind}> ;
       jolux:inForceStatus <http://data.legilux.public.lu/resource/authority/application-status/in-force> ;
       jolux:isMemberOf ?w ; jolux:isRealizedBy ?e .
  ?e jolux:language <http://publications.europa.eu/resource/authority/language/FRA> ; jolux:title ?title .
  OPTIONAL {{ ?act jolux:dateDocument ?date }}
  OPTIONAL {{ ?act jolux:dateEntryInForce ?force }}
  OPTIONAL {{ ?act jolux:publicationDate ?pub }}
  OPTIONAL {{ ?e jolux:isEmbodiedBy ?m . ?m jolux:userFormat ?f ; jolux:isExemplifiedBy ?u .
             FILTER(?f IN (<http://data.legilux.public.lu/resource/authority/user-format/xml>, <http://data.legilux.public.lu/resource/authority/user-format/xml-akomantoso>))
             OPTIONAL {{ ?m <http://purl.org/dc/terms/modified> ?mod }} }}
}} GROUP BY ?w ?title ?date ?force ?pub"#
                ))
                .await?
            {
                let (Some(w), Some(title)) = (r.get("w"), r.get("title")) else {
                    continue;
                };
                if !seen.insert(w.clone()) {
                    continue;
                }
                let day = |k: &str| r.get(k).map(|d| literal_day(d)).transpose();
                let adopted = day("date")?;
                let (key, title, mut listed) = match (codes.get(w), consolidations.get(w)) {
                    (Some(code), Some(listed)) => (
                        format!("CODE-{}", code.name.to_uppercase().replace('_', "-")),
                        code.title.clone(),
                        listed.clone(),
                    ),
                    (None, Some(listed)) if kind == "Constitution" => {
                        ("CONSTITUTION".to_string(), title.clone(), listed.clone())
                    }
                    (None, Some(listed)) => (work_key(w)?, title.clone(), listed.clone()),
                    (_, None) => {
                        let from = day("force")?.or(day("pub")?).or(adopted);
                        let listed = match (r.get("url"), from) {
                            (Some(url), Some(from)) => {
                                vec![(from.to_string(), filestore(url), day("modified")?)]
                            }
                            _ => Vec::new(),
                        };
                        (work_key(w)?, title.clone(), listed)
                    }
                };
                listed.sort();
                let mut states: Vec<Consolidation> = Vec::new();
                for (date, xml_url, modified) in listed {
                    match states.last_mut() {
                        Some(last) if last.applies_from == date => {
                            last.xml_url = xml_url;
                            last.modified = last.modified.max(modified);
                        }
                        _ => states.push(Consolidation {
                            applies_from: date,
                            xml_url,
                            modified,
                            token: None,
                        }),
                    }
                }
                works.push(LegiluxWork {
                    key,
                    kind: kind.to_string(),
                    title: title
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim_end_matches('.')
                        .to_string(),
                    adopted,
                    consolidated: consolidations.contains_key(w),
                    states,
                });
            }
        }
        Ok(works)
    }
}

/// `loi/2005/02/04/n1` for `…/eli/etat/leg/loi/2005/02/04/n1`, `adm/amin/…` for an administrative act.
fn work_key(work: &str) -> Result<String> {
    let path = work
        .strip_prefix(ELI)
        .ok_or_else(|| SourceError::Invalid(format!("not a Legilux work: {work:?}")))?;
    Ok(path.strip_prefix("leg/").unwrap_or(path).to_string())
}
