//! SenegalLII, a Peachjam site of Laws.Africa: its legislation sitemap, pages and source PDFs.

use std::time::Duration;

use lj_parse::{PeachjamListed, PeachjamPage};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://senlii.org";
/// The `Crawl-delay` its robots.txt asks.
const PACE: Duration = Duration::from_secs(5);

pub struct SenliiClient {
    paced: PacedClient,
}

impl SenliiClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "senlii.org")?.paced(PACE),
        })
    }

    /// Every work the legislation sitemap lists, by its latest expression.
    pub async fn listing(&self) -> Result<Vec<PeachjamListed>> {
        let xml = self
            .paced
            .text(&format!("{SITE}/sitemaps/legislation.xml"))
            .await?;
        Ok(lj_parse::peachjam_sitemap(&xml))
    }

    pub fn url(path: &str) -> String {
        format!("{SITE}{path}")
    }

    pub async fn page(&self, path: &str) -> Result<PeachjamPage> {
        let url = Self::url(path);
        let html = self.paced.text(&url).await?;
        lj_parse::peachjam_page(&html, path)
            .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    pub async fn pdf(&self, path: &str) -> Result<Vec<u8>> {
        let url = Self::url(path);
        self.paced.send(|| self.paced.http.get(&url)).await
    }
}
