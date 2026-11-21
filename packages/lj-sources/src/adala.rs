//! The Adala portal of Morocco's Justice ministry, read through its folder pages.

use std::collections::HashSet;

use lj_parse::AdalaFile;

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://adala.justice.gov.ma";
/// Constitutions, laws by matter, organic laws, judicial organisation, legal professions.
const FRENCH_ROOTS: [u64; 5] = [752, 411, 754, 422, 426];
/// Constitutions, national laws, organic laws, constitutional bodies, the justice system.
const ARABIC_ROOTS: [u64; 5] = [568, 12, 896, 569, 2];

pub struct AdalaClient {
    paced: PacedClient,
}

impl AdalaClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, "adala.justice.gov.ma")?,
        })
    }

    /// Every file in `language`, `FR` or `AR`, under the national texts' folders, each once.
    pub async fn files(&self, language: &str) -> Result<Vec<AdalaFile>> {
        let (roots, segment) = match language {
            "AR" => (ARABIC_ROOTS, "ar"),
            _ => (FRENCH_ROOTS, "fr"),
        };
        let mut queue: Vec<u64> = roots.to_vec();
        let mut walked: HashSet<u64> = HashSet::new();
        let mut seen: HashSet<u64> = HashSet::new();
        let mut files = Vec::new();
        while let Some(id) = queue.pop() {
            if !walked.insert(id) {
                continue;
            }
            let url = format!("{SITE}/{segment}/resources/{id}");
            let html = self.paced.text(&url).await?;
            let folder = lj_parse::adala_folder(&html, language)
                .map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
            queue.extend(folder.folders);
            files.extend(folder.files.into_iter().filter(|f| seen.insert(f.id)));
        }
        Ok(files)
    }

    pub fn pdf_url(path: &str) -> String {
        format!("{SITE}/api/{path}")
    }

    pub async fn pdf(&self, url: &str) -> Result<Vec<u8>> {
        self.paced.send(|| self.paced.http.get(url)).await
    }
}
