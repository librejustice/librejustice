//! LexFind, the portal the Conference of State Chancellors runs over every Swiss systematic
//! collection: a canton's acts in force, each act's versions since May 2006, each version the
//! PDF its canton published, and the canton's changes, newest first, through the egress proxy.
//! A version is handed on as the `pdftohtml -xml` rendering of its PDF.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::NaiveDate;
use lj_parse::LexfindLayout;
use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::lexwork::Listing;
use crate::publisher::{Consolidation, PacedClient};

pub const HOST: &str = "www.lexfind.ch";
const SITE: &str = "https://www.lexfind.ch";
/// The systematic nodes asked for in one request: 328 for Neuchâtel overflow the URL.
const NODES_PER_REQUEST: usize = 50;

pub struct LexfindCanton {
    pub code: &'static str,
    /// The canton's id at LexFind.
    pub entity: u32,
    pub lang: &'static str,
    pub layout: LexfindLayout,
}

pub const CANTONS: &[LexfindCanton] = &[
    LexfindCanton {
        code: "GE",
        entity: 8,
        lang: "fr",
        layout: LexfindLayout::Ge,
    },
    LexfindCanton {
        code: "ZH",
        entity: 26,
        lang: "de",
        layout: LexfindLayout::Zh,
    },
    LexfindCanton {
        code: "TI",
        entity: 21,
        lang: "it",
        layout: LexfindLayout::Ti,
    },
    LexfindCanton {
        code: "NE",
        entity: 13,
        lang: "fr",
        layout: LexfindLayout::Ne,
    },
    LexfindCanton {
        code: "SZ",
        entity: 19,
        lang: "de",
        layout: LexfindLayout::Sz,
    },
    LexfindCanton {
        code: "JU",
        entity: 11,
        lang: "fr",
        layout: LexfindLayout::Ju,
    },
];

/// An act in force, as its systematic node lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct LexfindAct {
    pub id: u64,
    pub systematic_number: String,
    pub title: String,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    tols: Vec<LexfindAct>,
}

#[derive(Deserialize)]
struct Versions {
    families: Vec<Vec<Vec<Version>>>,
}

#[derive(Deserialize)]
struct Version {
    id: u64,
    version_active_since: Option<String>,
    dtah_urls: Vec<VersionUrl>,
}

#[derive(Deserialize)]
struct VersionUrl {
    language: String,
    url: String,
}

#[derive(Deserialize)]
struct Changes {
    recent_changes: Vec<Change>,
    next_batch: Option<String>,
}

#[derive(Deserialize)]
struct Change {
    change_date: String,
    text_of_law: ChangedAct,
}

#[derive(Deserialize)]
struct ChangedAct {
    id: u64,
}

/// `28.10.2008` as a day.
fn day(dotted: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(dotted, "%d.%m.%Y").ok()
}

pub struct LexfindClient {
    paced: Arc<PacedClient>,
    pub canton: &'static LexfindCanton,
}

impl LexfindClient {
    /// A client for each canton `selected` keeps, every request of them all through one pace.
    pub fn per_canton(proxy: &str, selected: impl Fn(&str) -> bool) -> Result<Vec<Self>> {
        let paced = Arc::new(PacedClient::new(proxy, HOST)?);
        Ok(CANTONS
            .iter()
            .filter(|c| selected(c.code))
            .map(|canton| Self {
                paced: paced.clone(),
                canton,
            })
            .collect())
    }

    fn api(&self, path: &str) -> String {
        format!("{SITE}/api/frontend/v1/{}/{path}", self.canton.lang)
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let body = self.paced.text(url).await?;
        serde_json::from_str(&body).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// The act's page on the portal.
    pub fn page_url(&self, id: u64) -> String {
        format!("{SITE}/fe/{lang}/tol/{id}/{lang}", lang = self.canton.lang)
    }

    /// The acts in force, from every node of the canton's systematic tree.
    pub async fn acts(&self) -> Result<Vec<LexfindAct>> {
        let entity = self.canton.entity;
        let tree: HashMap<String, serde_json::Value> = self
            .json(&self.api(&format!("entities/{entity}/systematics?active_only=true")))
            .await?;
        let nodes: Vec<&String> = tree.keys().filter(|k| !k.is_empty()).collect();
        let mut acts: BTreeMap<u64, LexfindAct> = BTreeMap::new();
        for chunk in nodes.chunks(NODES_PER_REQUEST) {
            let query: Vec<String> = chunk
                .iter()
                .map(|n| format!("tols_for_systematics%5B%5D={n}"))
                .collect();
            let listed: HashMap<String, Node> = self
                .json(&self.api(&format!(
                    "entities/{entity}/systematics?active_only=true&{}",
                    query.join("&")
                )))
                .await?;
            for (key, node) in listed {
                if chunk.contains(&&key) {
                    acts.extend(node.tols.into_iter().map(|a| (a.id, a)));
                }
            }
        }
        Ok(acts.into_values().collect())
    }

    /// The act's states, oldest first: a day the portal found several versions of keeps the one
    /// it found last, a correction of the one before.
    pub async fn listing(&self, id: u64) -> Result<Listing> {
        let url = self.api(&format!("texts-of-law/{id}/with-version-groups"));
        let versions: Versions = self.json(&url).await?;
        let mut by_day: BTreeMap<NaiveDate, (u64, String)> = BTreeMap::new();
        for v in versions.families.into_iter().flatten().flatten() {
            let Some(applies_from) = v.version_active_since.as_deref().and_then(day) else {
                continue;
            };
            let Some(file) = v.dtah_urls.iter().find(|u| u.language == self.canton.lang) else {
                continue;
            };
            if by_day
                .get(&applies_from)
                .is_none_or(|(kept, _)| *kept < v.id)
            {
                by_day.insert(applies_from, (v.id, format!("{SITE}{}", file.url)));
            }
        }
        Ok(Listing {
            adopted: None,
            consolidations: by_day
                .into_iter()
                .map(|(applies_from, (version, xml_url))| Consolidation {
                    applies_from: applies_from.to_string(),
                    xml_url,
                    modified: None,
                    token: Some(version),
                })
                .collect(),
            abbreviation: None,
        })
    }

    /// A version's PDF as `pdftohtml -xml` renders it.
    pub async fn xml(&self, url: &str) -> Result<String> {
        let pdf = self.paced.send(|| self.paced.http.get(url)).await?;
        tokio::task::spawn_blocking(move || crate::pdf::pdftohtml_xml_extract(&pdf))
            .await
            .map_err(|e| SourceError::Invalid(format!("{url}: pdftohtml: {e}")))?
    }

    /// The acts the canton changed on `since` or later, by the portal's feed, newest first.
    pub async fn changed_since(&self, since: NaiveDate) -> Result<HashSet<u64>> {
        let mut changed = HashSet::new();
        let mut url = self.api(&format!(
            "entities/{}/recent-changes?filter_formless=false",
            self.canton.entity
        ));
        loop {
            let page: Changes = self.json(&url).await?;
            let mut older = false;
            for c in page.recent_changes {
                match day(&c.change_date) {
                    Some(d) if d >= since => {
                        changed.insert(c.text_of_law.id);
                    }
                    Some(_) => older = true,
                    None => {
                        return Err(SourceError::Invalid(format!(
                            "{url}: change day {:?}",
                            c.change_date
                        )))
                    }
                }
            }
            match page.next_batch.filter(|_| !older) {
                Some(next) => url = format!("{SITE}{next}"),
                None => return Ok(changed),
            }
        }
    }
}
