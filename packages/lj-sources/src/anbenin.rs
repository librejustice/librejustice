//! The documentation centre of Benin's National Assembly, read through its Omeka S API.

use std::collections::HashMap;

use lj_parse::AnbeninLaw;

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://documentation-anbenin.org";
const PAGE: usize = 100;

pub struct AnbeninClient {
    paced: PacedClient,
}

impl AnbeninClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "documentation-anbenin.org")?,
        })
    }

    async fn pages<T>(
        &self,
        kind: &str,
        parse: impl Fn(&str) -> std::result::Result<Vec<T>, String>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        for page in 1.. {
            let url =
                format!("{SITE}/api/{kind}?per_page={PAGE}&page={page}&sort_by=id&sort_order=asc");
            let json = self.paced.text(&url).await?;
            let read = serde_json::from_str::<Vec<serde_json::Value>>(&json)
                .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?
                .len();
            out.extend(parse(&json).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?);
            if read < PAGE {
                break;
            }
        }
        Ok(out)
    }

    /// Every law of the collection, by item id.
    pub async fn laws(&self) -> Result<Vec<AnbeninLaw>> {
        self.pages("items", lj_parse::anbenin_laws).await
    }

    /// The PDF of every media, by media id.
    pub async fn pdfs(&self) -> Result<HashMap<u64, String>> {
        Ok(self
            .pages("media", lj_parse::anbenin_pdfs)
            .await?
            .into_iter()
            .collect())
    }

    pub async fn pdf(&self, url: &str) -> Result<Vec<u8>> {
        self.paced.send(|| self.paced.http.get(url)).await
    }
}
