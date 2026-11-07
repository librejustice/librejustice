//! OAI-PMH harvest of the law journals and blogs that comment cassation
//! decisions, one request a second per host, under each host's robots.txt.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use lj_parse::{
    cairn_label_title, cairn_note, extract_dockets, openedition_note, page_dc_title,
    parse_oai_page, prairial_note, prairial_page_url, robots_allows, DoctrineNote, OaiRecord,
};
use url::Url;

use crate::error::{Result, SourceError};
use crate::hal::USER_AGENT;

pub const THROTTLE: Duration = Duration::from_millis(1100);

const ROBOTS_AGENT: &str = "LibreJusticeBot";

const CAIRN: &str = "https://oai.cairn.info/oai.php";
const OPENEDITION: &str = "https://oai.openedition.org/";
const PRAIRIAL: &str = "https://publications-prairial.fr/oai/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Cairn,
    OpenEdition,
    Prairial,
}

/// A journal or a blog, under the name its publisher writes.
#[derive(Debug)]
pub struct Journal {
    pub key: &'static str,
    pub name: &'static str,
    pub platform: Platform,
    pub endpoint: &'static str,
    pub set: &'static str,
}

pub const JOURNALS: [Journal; 5] = [
    Journal {
        key: "drou",
        name: "Le Droit Ouvrier",
        platform: Platform::Cairn,
        endpoint: CAIRN,
        set: "DROU",
    },
    Journal {
        key: "rcdip",
        name: "Revue critique de droit international privé",
        platform: Platform::Cairn,
        endpoint: CAIRN,
        set: "RCDIP",
    },
    Journal {
        key: "dsf",
        name: "Droit du travail et de la protection sociale",
        platform: Platform::OpenEdition,
        endpoint: OPENEDITION,
        set: "blogs:dsf",
    },
    Journal {
        key: "m2dsplille",
        name: "Droit de la protection sociale",
        platform: Platform::OpenEdition,
        endpoint: OPENEDITION,
        set: "blogs:m2dsplille",
    },
    Journal {
        key: "ajdc",
        name: "Actualité juridique du dommage corporel",
        platform: Platform::Prairial,
        endpoint: PRAIRIAL,
        set: "journals:ajdc",
    },
];

pub struct Harvest {
    /// Records listed, the articles of the issues on Cairn.
    pub records: usize,
    pub notes: Vec<DoctrineNote>,
}

pub struct Harvester {
    client: reqwest::Client,
    last: HashMap<String, Instant>,
    /// By host, `None` when it serves no robots.txt.
    robots: HashMap<String, Option<String>>,
}

impl Harvester {
    /// An empty `proxy` goes out from the host's own address.
    pub fn new(proxy: &str) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120));
        if !proxy.is_empty() {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        Ok(Self {
            client: builder.build()?,
            last: HashMap::new(),
            robots: HashMap::new(),
        })
    }

    /// The notes a journal published or touched since `from` (`YYYY-MM-DD`),
    /// all of them when `None`.
    pub async fn harvest(&mut self, journal: &Journal, from: Option<&str>) -> Result<Harvest> {
        match journal.platform {
            Platform::Cairn => self.cairn(journal, from).await,
            Platform::OpenEdition => {
                let records = self
                    .list_records(journal.endpoint, "oai_dc", journal.set, from)
                    .await?;
                Ok(Harvest {
                    records: records.len(),
                    notes: records
                        .iter()
                        .filter_map(|r| openedition_note(r, journal.name))
                        .collect(),
                })
            }
            Platform::Prairial => self.prairial(journal, from).await,
        }
    }

    /// The METS issue records carry each article's full title, which `oai_dc`
    /// cuts before the citation, and `oai_dc` carries its landing page and
    /// access: only the articles whose title writes a docket are joined.
    async fn cairn(&mut self, journal: &Journal, from: Option<&str>) -> Result<Harvest> {
        let mut dc: HashMap<String, OaiRecord> = HashMap::new();
        for r in self
            .list_records(journal.endpoint, "oai_dc", journal.set, from)
            .await?
        {
            let id = r.identifier.rsplit(':').next().unwrap_or_default();
            dc.insert(id.to_string(), r);
        }
        // Cairn's METS answers every issue whatever `from` says:
        // the window is read on the issue's datestamp, or on the article's own.
        let issues = self
            .list_records(journal.endpoint, "mets", journal.set, None)
            .await?;
        let touched = &dc;
        let articles: Vec<(String, String)> = issues
            .into_iter()
            .flat_map(|issue| {
                let fresh = from.is_none_or(|f| issue.datestamp.as_str() >= f);
                issue
                    .articles
                    .into_iter()
                    .filter(move |(id, _)| fresh || touched.contains_key(id))
            })
            .map(|(id, label)| (id, cairn_label_title(&label)))
            .collect();
        let citing: Vec<&(String, String)> = articles
            .iter()
            .filter(|(_, title)| !extract_dockets(title).is_empty())
            .collect();
        let mut notes = Vec::new();
        for (id, title) in citing {
            let record = match dc.remove(id) {
                Some(r) => Some(r),
                None => {
                    self.get_record(journal.endpoint, "oai_dc", &format!("oai:cairn.info:{id}"))
                        .await?
                }
            };
            match record.and_then(|r| cairn_note(id, title, &r, journal.name)) {
                Some(note) => notes.push(note),
                None => {
                    tracing::warn!(journal = journal.key, article = %id, "Cairn article without a landing page")
                }
            }
        }
        Ok(Harvest {
            records: articles.len(),
            notes,
        })
    }

    /// The OAI title drops the subtitle that names the decision: each article's
    /// page is read for its `DC.title`.
    async fn prairial(&mut self, journal: &Journal, from: Option<&str>) -> Result<Harvest> {
        let records = self
            .list_records(journal.endpoint, "oai_dc", journal.set, from)
            .await?;
        let mut notes = Vec::new();
        for r in &records {
            let Some(page) = prairial_page_url(r) else {
                continue;
            };
            let url =
                Url::parse(&page).map_err(|e| SourceError::Invalid(format!("{page}: {e}")))?;
            let title = match self.get(url).await {
                Ok(html) => page_dc_title(&html),
                Err(e) => {
                    tracing::warn!(journal = journal.key, page = %page, error = %e, "article page unread");
                    continue;
                }
            };
            match title.and_then(|t| prairial_note(r, &t, journal.name)) {
                Some(note) => notes.push(note),
                None => {
                    tracing::warn!(journal = journal.key, page = %page, "article page without a title")
                }
            }
        }
        Ok(Harvest {
            records: records.len(),
            notes,
        })
    }

    async fn list_records(
        &mut self,
        endpoint: &str,
        prefix: &str,
        set: &str,
        from: Option<&str>,
    ) -> Result<Vec<OaiRecord>> {
        let mut params = vec![
            ("verb", "ListRecords"),
            ("metadataPrefix", prefix),
            ("set", set),
        ];
        params.extend(from.map(|f| ("from", f)));
        let mut url = oai_url(endpoint, &params)?;
        let mut out = Vec::new();
        loop {
            let page = parse_oai_page(&self.get(url).await?)
                .map_err(|e| SourceError::Invalid(format!("{endpoint} {set} {prefix}: {e}")))?;
            out.extend(page.records.into_iter().filter(|r| !r.deleted));
            let Some(token) = page.resumption_token else {
                break;
            };
            url = oai_url(
                endpoint,
                &[("verb", "ListRecords"), ("resumptionToken", token.as_str())],
            )?;
        }
        Ok(out)
    }

    async fn get_record(
        &mut self,
        endpoint: &str,
        prefix: &str,
        identifier: &str,
    ) -> Result<Option<OaiRecord>> {
        let url = oai_url(
            endpoint,
            &[
                ("verb", "GetRecord"),
                ("metadataPrefix", prefix),
                ("identifier", identifier),
            ],
        )?;
        let page = parse_oai_page(&self.get(url).await?)
            .map_err(|e| SourceError::Invalid(format!("{endpoint} {identifier}: {e}")))?;
        Ok(page.records.into_iter().find(|r| !r.deleted))
    }

    async fn get(&mut self, url: Url) -> Result<String> {
        let host = url.host_str().unwrap_or_default().to_string();
        if !self.robots.contains_key(&host) {
            let robots = url
                .join("/robots.txt")
                .map_err(|e| SourceError::Invalid(format!("{host}: {e}")))?;
            let body = match self.fetch(robots).await? {
                (200..=299, body) => Some(body),
                // RFC 9309 2.3.1.3: an unavailable robots.txt restricts nothing.
                (400..=499, _) => None,
                (status, _) => {
                    return Err(SourceError::Invalid(format!(
                        "{host}/robots.txt: status {status}"
                    )))
                }
            };
            self.robots.insert(host.clone(), body);
        }
        if let Some(Some(robots)) = self.robots.get(&host) {
            if !robots_allows(robots, ROBOTS_AGENT, url.path()) {
                return Err(SourceError::Invalid(format!(
                    "{host}/robots.txt refuses {}",
                    url.path()
                )));
            }
        }
        let shown = url.to_string();
        match self.fetch(url).await? {
            (200, body) => Ok(body),
            (status, _) => Err(SourceError::Invalid(format!("{shown}: status {status}"))),
        }
    }

    async fn fetch(&mut self, url: Url) -> Result<(u16, String)> {
        let host = url.host_str().unwrap_or_default().to_string();
        if let Some(since) = self.last.get(&host).map(Instant::elapsed) {
            if since < THROTTLE {
                tokio::time::sleep(THROTTLE - since).await;
            }
        }
        let sent = self.client.get(url).send().await;
        self.last.insert(host, Instant::now());
        let resp = sent?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await?))
    }
}

fn oai_url(endpoint: &str, params: &[(&str, &str)]) -> Result<Url> {
    Url::parse_with_params(endpoint, params)
        .map_err(|e| SourceError::Invalid(format!("{endpoint}: {e}")))
}
