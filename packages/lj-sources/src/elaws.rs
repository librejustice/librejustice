//! Ontario's e-Laws: its sitemaps, then each version's page, one second apart as its `robots.txt` asks.

use std::time::Duration;

use lj_parse::{ElawsKind, ElawsListed};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const ROOT: &str = "https://www.ontario.ca";
/// The English sitemaps, which date each version, and the French one, which lists its pages.
const ENGLISH: [&str; 4] = [
    "statute-current",
    "statute-historical",
    "regulation-current",
    "regulation-historical",
];

pub struct ElawsClient {
    site: PacedClient,
}

impl ElawsClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            site: PacedClient::new(proxy, "ontario.ca")?.paced(Duration::from_secs(1)),
        })
    }

    /// `https://www.ontario.ca/laws/statute/92c06/v3`, `https://www.ontario.ca/lois/loi/92c06`.
    pub fn page_url(kind: ElawsKind, key: &str, version: Option<u32>, french: bool) -> String {
        let (root, segment) = match french {
            true => ("lois", kind.french()),
            false => ("laws", kind.english()),
        };
        match version {
            Some(v) => format!("{ROOT}/{root}/{segment}/{key}/v{v}"),
            None => format!("{ROOT}/{root}/{segment}/{key}"),
        }
    }

    async fn sitemap(&self, url: &str) -> Result<Vec<ElawsListed>> {
        let xml = self.site.text(url).await?;
        lj_parse::elaws_sitemap(&xml).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// The pages of the English sitemaps, then those of the French one.
    pub async fn listing(&self) -> Result<(Vec<ElawsListed>, Vec<ElawsListed>)> {
        let mut english = Vec::new();
        for name in ENGLISH {
            let url = format!("{ROOT}/laws/sitemap/sitemap-law-extended-{name}-en.xml");
            english.extend(self.sitemap(&url).await?);
        }
        let french = format!("{ROOT}/laws/sitemap/sitemap-law-fr.xml");
        Ok((english, self.sitemap(&french).await?))
    }

    pub async fn html(&self, url: &str) -> Result<String> {
        self.site.text(url).await
    }
}
