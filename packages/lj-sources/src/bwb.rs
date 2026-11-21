//! The BWB: the `wetten.overheid.nl` sitemap, then KOOP's manifests and consolidated states.

use lj_parse::{BwbListed, BwbState, BwbToestand};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITEMAP: &str = "https://wetten.overheid.nl/sitemap.xml";
const REPOSITORY: &str = "https://repository.officiele-overheidspublicaties.nl/bwb";

pub struct BwbClient {
    site: PacedClient,
    repository: PacedClient,
}

impl BwbClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            site: PacedClient::new(proxy, "wetten.overheid.nl")?,
            repository: PacedClient::new(proxy, "repository.officiele-overheidspublicaties.nl")?,
        })
    }

    pub fn page_url(id: &str) -> String {
        format!("https://wetten.overheid.nl/{id}/")
    }

    pub async fn listing(&self) -> Result<Vec<BwbListed>> {
        let xml = self.site.text(SITEMAP).await?;
        lj_parse::bwb_sitemap(&xml).map_err(|e| SourceError::Invalid(format!("{SITEMAP}: {e}")))
    }

    pub async fn states(&self, id: &str) -> Result<Vec<BwbState>> {
        let url = format!("{REPOSITORY}/{id}/manifest.xml");
        let xml = self.repository.text(&url).await?;
        lj_parse::bwb_manifest(&xml).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// The abbreviations the regeling's WTI lists, from the file's first 32 KB: the whole file
    /// also lists every related regeling, 23 MB for the Algemene wet bestuursrecht.
    pub async fn abbreviations(&self, id: &str) -> Result<Vec<String>> {
        let url = format!("{REPOSITORY}/{id}/{id}.WTI");
        let head = self
            .repository
            .send(|| {
                self.repository
                    .http
                    .get(&url)
                    .header(reqwest::header::RANGE, "bytes=0-32767")
            })
            .await?;
        lj_parse::bwb_abbreviations(&String::from_utf8_lossy(&head))
            .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// A state and the size of its XML.
    pub async fn state(&self, id: &str, state: &BwbState) -> Result<(BwbToestand, usize)> {
        let url = format!("{REPOSITORY}/{id}/{}", state.path);
        let xml = self.repository.text(&url).await?;
        let toestand = lj_parse::bwb_toestand(&xml)
            .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
        Ok((toestand, xml.len()))
    }
}
