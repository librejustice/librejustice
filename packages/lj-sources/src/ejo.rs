//! Djibouti's electronic Journal officiel, read through its WordPress API.

use std::collections::HashMap;

use lj_parse::{EjoIssue, EjoText};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const API: &str = "https://www.journalofficiel.dj/wp-json/wp/v2";
const PAGE: usize = 100;
const LISTED: &str =
    "id,date,modified,link,title,nature-dun-texte,institution,acf.reference,acf.journal_officiel";

pub struct EjoClient {
    paced: PacedClient,
}

fn ids(ids: &[u64]) -> String {
    ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

impl EjoClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "www.journalofficiel.dj")?,
        })
    }

    async fn read<T>(
        &self,
        url: &str,
        parse: impl Fn(&str) -> std::result::Result<Vec<T>, String>,
    ) -> Result<Vec<T>> {
        parse(&self.paced.text(url).await?).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// Every page of `path` by offset; past the last one the API answers an empty list.
    async fn pages<T>(
        &self,
        path: &str,
        query: &str,
        parse: impl Fn(&str) -> std::result::Result<Vec<T>, String>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        loop {
            let url = format!(
                "{API}/{path}?per_page={PAGE}&offset={}&orderby=id&order=asc{query}",
                out.len()
            );
            let page = self.read(&url, &parse).await?;
            let done = page.len() < PAGE;
            out.extend(page);
            if done {
                return Ok(out);
            }
        }
    }

    /// The texts without their content, those changed after `since` (`YYYY-MM-DDTHH:MM:SS`) if given.
    pub async fn listing(&self, since: Option<&str>) -> Result<Vec<EjoText>> {
        let query = match since {
            Some(since) => format!("&_fields={LISTED}&modified_after={since}"),
            None => format!("&_fields={LISTED}"),
        };
        self.pages("texte-juridique", &query, lj_parse::ejo_texts)
            .await
    }

    /// At most a hundred texts, with their content.
    pub async fn contents(&self, texts: &[u64]) -> Result<Vec<EjoText>> {
        let url = format!(
            "{API}/texte-juridique?per_page={PAGE}&include={}&_fields={LISTED},content,acf.visas,acf.signature",
            ids(texts)
        );
        self.read(&url, lj_parse::ejo_texts).await
    }

    /// At most a hundred issues.
    pub async fn issues(&self, issues: &[u64]) -> Result<Vec<EjoIssue>> {
        let url = format!(
            "{API}/journal-officiel?per_page={PAGE}&include={}&_fields=id,link,acf",
            ids(issues)
        );
        self.read(&url, lj_parse::ejo_issues).await
    }

    /// The name of each institution a text may be filed under.
    pub async fn institutions(&self) -> Result<HashMap<u64, String>> {
        Ok(self
            .pages("institution", "&_fields=id,name", lj_parse::ejo_terms)
            .await?
            .into_iter()
            .collect())
    }
}
