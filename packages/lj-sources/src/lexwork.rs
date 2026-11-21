//! LexWork, the platform 19 Swiss cantons publish their systematic collection on: a canton's acts
//! in force from its index, the versions of an act, and each version's structured JSON.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::publisher::{Consolidation, PacedClient};

/// A canton on LexWork: its code, its host, the language its acts are read in (French where the
/// canton legislates in French), and the number prefixes of the communes' acts its collection
/// files beside its own.
pub struct Canton {
    pub code: &'static str,
    pub host: &'static str,
    pub lang: &'static str,
    pub communes: &'static [&'static str],
}

pub const CANTONS: &[Canton] = &[
    canton("AG", "gesetzessammlungen.ag.ch", "de"),
    canton("AI", "ai.clex.ch", "de"),
    canton("AR", "ar.clex.ch", "de"),
    canton("BE", "www.belex.sites.be.ch", "fr"),
    canton("BL", "bl.clex.ch", "de"),
    Canton {
        communes: &["BaB ", "BeB ", "BeE ", "RiB ", "RiE "],
        ..canton("BS", "www.gesetzessammlung.bs.ch", "de")
    },
    canton("FR", "bdlf.fr.ch", "fr"),
    canton("GL", "gesetze.gl.ch", "de"),
    canton("GR", "www.gr-lex.gr.ch", "de"),
    canton("LU", "srl.lu.ch", "de"),
    canton("NW", "gesetze.nw.ch", "de"),
    canton("OW", "gdb.ow.ch", "de"),
    canton("SG", "www.gesetzessammlung.sg.ch", "de"),
    canton("SH", "rechtsbuch.sh.ch", "de"),
    canton("SO", "bgs.so.ch", "de"),
    canton("TG", "www.rechtsbuch.tg.ch", "de"),
    canton("UR", "rechtsbuch.ur.ch", "de"),
    canton("VS", "lex.vs.ch", "fr"),
    canton("ZG", "bgs.zg.ch", "de"),
];

const fn canton(code: &'static str, host: &'static str, lang: &'static str) -> Canton {
    Canton {
        code,
        host,
        lang,
        communes: &[],
    }
}

static DAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{2})\.(\d{2})\.(\d{4})").unwrap());

/// An act in force in a canton's index, with the structured document of its current version, which
/// a version published as a PDF alone lacks.
#[derive(Debug, Clone, Deserialize)]
pub struct IndexedAct {
    pub systematic_number: String,
    pub title: String,
    abrogated: bool,
    pub structured_document_id: Option<u64>,
}

#[derive(Deserialize)]
struct Answer {
    text_of_law: Act,
}

#[derive(Deserialize)]
struct Act {
    date_of_decision: Option<String>,
    abbreviation: Option<String>,
    current_version: Version,
    #[serde(default)]
    old_versions: Vec<Version>,
    #[serde(default)]
    future_versions: Vec<Version>,
}

#[derive(Deserialize)]
struct Version {
    id: u64,
    version_dates_str: String,
    structured_document_id: Option<u64>,
}

/// An act's versions with a structured document, oldest first, the day it was adopted, and the
/// short title and abbreviation its own record gives, `Baugesetz, BauG`.
pub struct Listing {
    pub adopted: Option<String>,
    pub consolidations: Vec<Consolidation>,
    pub abbreviation: Option<String>,
}

pub struct LexworkClient {
    paced: PacedClient,
    pub canton: &'static Canton,
    /// The last act listed: its current version's URL and the answer that carries it.
    listed: Mutex<Option<(String, String)>>,
}

impl LexworkClient {
    pub fn new(proxy: &str, canton: &'static Canton) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, canton.host)?,
            canton,
            listed: Mutex::new(None),
        })
    }

    fn act_url(&self, number: &str, tail: &[&str]) -> Result<String> {
        let mut url = url::Url::parse(&format!(
            "https://{}/api/{}/texts_of_law/",
            self.canton.host, self.canton.lang
        ))
        .map_err(|e| SourceError::Invalid(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| SourceError::Invalid(format!("{} cannot carry a path", self.canton.host)))?
            .pop_if_empty()
            .push(number)
            .extend(tail);
        Ok(url.to_string())
    }

    /// The act's page on the canton's site.
    pub fn page_url(&self, number: &str) -> String {
        format!(
            "https://{}/app/{}/texts_of_law/{number}",
            self.canton.host, self.canton.lang
        )
    }

    /// The canton's own acts in force, in the index's order.
    pub async fn index(&self) -> Result<Vec<IndexedAct>> {
        let url = format!(
            "https://{}/api/{}/texts_of_law/lightweight_index",
            self.canton.host, self.canton.lang
        );
        let body = self.paced.send(|| self.paced.http.get(&url)).await?;
        let by_category: std::collections::BTreeMap<String, Vec<IndexedAct>> =
            serde_json::from_slice(&body)
                .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
        Ok(by_category
            .into_values()
            .flatten()
            .filter(|a| {
                !a.abrogated
                    && !self
                        .canton
                        .communes
                        .iter()
                        .any(|p| a.systematic_number.starts_with(p))
            })
            .collect())
    }

    /// The versions of the act numbered `number` that have a structured document, oldest first,
    /// the newest of a day when the canton keeps several, each named by its document.
    pub async fn listing(&self, number: &str) -> Result<Listing> {
        let url = self.act_url(number, &["show_as_json"])?;
        let body = self.paced.text(&url).await?;
        let answer: Answer =
            serde_json::from_str(&body).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
        let act = answer.text_of_law;
        let current = act.current_version.id;
        let mut rows: Vec<(String, u64, u64)> = Vec::new();
        for v in std::iter::once(act.current_version)
            .chain(act.old_versions)
            .chain(act.future_versions)
        {
            let Some(document) = v.structured_document_id else {
                continue;
            };
            let day = DAY.captures(&v.version_dates_str).ok_or_else(|| {
                SourceError::Invalid(format!(
                    "{url}: version {} without a start day: {:?}",
                    v.id, v.version_dates_str
                ))
            })?;
            rows.push((
                format!("{}-{}-{}", &day[3], &day[2], &day[1]),
                v.id,
                document,
            ));
        }
        rows.sort();
        let mut consolidations: Vec<Consolidation> = Vec::new();
        for (day, id, document) in rows {
            let state = Consolidation {
                xml_url: self.act_url(number, &["versions", &id.to_string(), "show_as_json"])?,
                applies_from: day,
                modified: None,
                token: Some(document),
            };
            if id == current {
                *self.listed.lock().await = Some((state.xml_url.clone(), body.clone()));
            }
            match consolidations.last_mut() {
                Some(last) if last.applies_from == state.applies_from => *last = state,
                _ => consolidations.push(state),
            }
        }
        let adopted = act
            .date_of_decision
            .filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-');
        Ok(Listing {
            adopted,
            consolidations,
            abbreviation: act.abbreviation.filter(|a| !a.trim().is_empty()),
        })
    }

    /// The JSON of one version; the current one comes from the listing that named it.
    pub async fn xml(&self, url: &str) -> Result<String> {
        if let Some((listed, body)) = self.listed.lock().await.as_ref() {
            if listed == url {
                return Ok(body.clone());
            }
        }
        self.paced.text(url).await
    }
}
