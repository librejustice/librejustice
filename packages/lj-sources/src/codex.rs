//! The Vlaamse Codex open data API: its listing of Flemish acts, and an act's text on a day.

use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const API: &str = "https://codex.opendata.api.vlaanderen.be/api/WetgevingDocument";
const PAGE: usize = 1000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListedAct {
    pub id: u64,
    pub opschrift: String,
    pub datum: Option<String>,
    pub wetgeving_document_type: String,
    pub heeft_inhoud: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Page {
    totaal_aantal: usize,
    resultaten_lijst: Vec<ListedAct>,
}

pub struct CodexClient {
    paced: PacedClient,
}

impl CodexClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "codex.opendata.api.vlaanderen.be")?,
        })
    }

    /// The act's page on the Codex.
    pub fn page_url(id: u64) -> String {
        format!("https://codex.vlaanderen.be/Zoeken/Document.aspx?DID={id}&param=inhoud")
    }

    async fn pages(&self, filter: &str) -> Result<Vec<ListedAct>> {
        let mut acts = Vec::new();
        loop {
            let url = format!("{API}?skip={}&take={PAGE}{filter}", acts.len());
            let body = self
                .paced
                .send(|| {
                    self.paced
                        .http
                        .get(&url)
                        .header("Accept", "application/json")
                })
                .await?;
            let page: Page = serde_json::from_slice(&body)
                .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
            let done = page.resultaten_lijst.is_empty();
            acts.extend(page.resultaten_lijst);
            if done || acts.len() >= page.totaal_aantal {
                return Ok(acts);
            }
        }
    }

    /// Every act the Codex lists, in its order.
    pub async fn listing(&self) -> Result<Vec<ListedAct>> {
        self.pages("").await
    }

    /// The acts the Codex changed on or after `day` (`YYYY-MM-DD`).
    pub async fn changed_since(&self, day: &str) -> Result<Vec<ListedAct>> {
        self.pages(&format!("&gewijzigdSinds={day}")).await
    }

    /// The act `id` as in force on `day`, or today.
    pub async fn state(&self, id: u64, day: Option<&str>) -> Result<String> {
        let url = match day {
            Some(day) => format!("{API}/{id}/VolledigDocument?datum={day}"),
            None => format!("{API}/{id}/VolledigDocument"),
        };
        let body = self
            .paced
            .send(|| {
                self.paced
                    .http
                    .get(&url)
                    .header("Accept", "application/json")
            })
            .await?;
        String::from_utf8(body)
            .map_err(|e| SourceError::Invalid(format!("{url} is not UTF-8: {e}")))
    }
}
