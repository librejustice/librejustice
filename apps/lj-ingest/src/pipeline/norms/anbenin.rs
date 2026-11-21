//! Benin's laws as its National Assembly files them, one version each from its PDF's text layer.

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, Result};
use lj_parse::{AnbeninLaw, ConsolidatedArticle};
use lj_sources::anbenin::AnbeninClient;
use lj_store::repository::DecisionRepository;

use super::super::legal_corpus::{load_doc, CorpusArticle, CorpusDoc, CorpusLink};
use super::{refused, Run, Tally};

pub(super) const SOURCE: &str = "anbenin";
/// Fewer characters a page than this, and a PDF is a scan.
const NATIVE_CHARS_PER_PAGE: usize = 200;

fn base_uid(code: &str, number: &str) -> String {
    format!("BJ/{code}-{}", lj_parse::journal_number_key(number))
}

/// Each law's uid, `None` for an item whose number a later item takes.
fn identities(laws: &[AnbeninLaw]) -> HashMap<u64, Option<String>> {
    let mut latest: BTreeMap<String, u64> = BTreeMap::new();
    for law in laws {
        let uid = base_uid("LOI", &law.number);
        let item = latest.entry(uid).or_insert(law.item);
        *item = (*item).max(law.item);
    }
    let owner: HashMap<u64, String> = latest.into_iter().map(|(uid, item)| (item, uid)).collect();
    laws.iter()
        .map(|law| (law.item, owner.get(&law.item).cloned()))
        .collect()
}

fn checksum(law: &AnbeninLaw, pdf: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(
        format!(
            "{}|{}|{:?}|{}|{:?}|{pdf}",
            law.nature, law.number, law.day, law.title, law.modified
        )
        .as_bytes(),
    )
}

fn article(
    num: String,
    num_key: Option<String>,
    texte: &str,
    path: Option<String>,
) -> CorpusArticle {
    CorpusArticle {
        num,
        num_key,
        texte: Some(lj_core::normalizer::normalize_chars(texte)),
        texte_original: None,
        lang_original: None,
        translation: None,
        nota: None,
        versions: None,
        title_path: path,
    }
}

fn doc(
    law: &AnbeninLaw,
    uid: &str,
    pdf: &str,
    articles: Vec<ConsolidatedArticle>,
    body: String,
) -> CorpusDoc {
    let articles = match articles.is_empty() {
        false => articles
            .into_iter()
            .map(|a| article(a.num, None, &a.texte, a.title_path))
            .collect(),
        true => vec![article(
            String::new(),
            Some("body".to_string()),
            &body,
            None,
        )],
    };
    let links = lj_parse::ejo_effects(&law.title)
        .into_iter()
        .map(|e| CorpusLink {
            verb: e.verb.to_string(),
            direction: "outgoing".to_string(),
            target_text_uid: base_uid(e.nature.code(), &e.reference),
            target_label: e.label,
            date: None,
        })
        .filter(|l| l.target_text_uid != uid)
        .collect();
    CorpusDoc {
        text_uid: uid.to_string(),
        source: SOURCE.to_string(),
        jurisdiction: "BJ".to_string(),
        title: law.title.clone(),
        title_key: None,
        nature: law.nature.to_string(),
        translation: "officiel".to_string(),
        source_url: Some(pdf.to_string()),
        source_asof: None,
        source_upstream_url: None,
        date_texte: law.day.clone(),
        effect_date: law.day.clone(),
        detect_abrogation: false,
        num_prefix_agnostic: false,
        articles,
        links,
    }
}

impl Run<'_> {
    /// Lists the collection, then reads the laws never visited and those changed since.
    pub(super) async fn anbenin_lane(&self, client: &AnbeninClient, limit: Option<usize>) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.anbenin(client, limit, &mut tally).await {
            tracing::error!(publisher = self.source, error = %e, "Assembly collection run stopped");
            tally.failed.push("anbenin".to_string());
        }
        tally
    }

    async fn anbenin(
        &self,
        client: &AnbeninClient,
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let laws = client.laws().await?;
        let pdfs = client.pdfs().await?;
        let identities = identities(&laws);
        let mut visited: Vec<(String, u64)> = Vec::new();
        let mut to_read: Vec<(&AnbeninLaw, String, &str)> = Vec::new();
        for law in &laws {
            let pdf = law.media.and_then(|m| pdfs.get(&m));
            let item = law.item.to_string();
            let sum = checksum(law, pdf.map_or("", String::as_str));
            if self
                .visits
                .get(&item)
                .is_some_and(|v| v.content_checksum == sum)
            {
                tally.unchanged += 1;
                continue;
            }
            let Some(pdf) = pdf else {
                tracing::warn!(
                    publisher = self.source,
                    item = law.item,
                    "law without a PDF, visited"
                );
                visited.push((item, sum));
                continue;
            };
            match &identities[&law.item] {
                None => visited.push((item, sum)),
                Some(uid) => to_read.push((law, uid.clone(), pdf)),
            }
        }
        if let Some(limit) = limit {
            to_read.truncate(limit);
        }
        tracing::info!(
            publisher = self.source,
            listed = laws.len(),
            to_read = to_read.len(),
            "Assembly laws listed"
        );
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let (mut scans, mut scan_pages) = (0usize, 0u32);
        let mut left = 0usize;
        for (law, uid, pdf) in to_read {
            let item = law.item.to_string();
            if self.left_for_later(&item) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            let sum = checksum(law, pdf);
            let read = async {
                let bytes = client.pdf(pdf).await?;
                let pages =
                    lj_sources::pdf::page_count(&bytes).map_err(|e| anyhow!("{pdf}: {e}"))?;
                let text = lj_sources::pdf::pdftotext_extract(&bytes)
                    .map_err(|e| anyhow!("{pdf}: {e}"))?;
                anyhow::Ok((pages, text))
            };
            let (pages, text) = match read.await {
                Ok(read) => read,
                Err(e) => {
                    tracing::error!(publisher = self.source, key = uid, error = %e, "followed text failed");
                    tally.failed.push(uid);
                    if refused(&e) {
                        break;
                    }
                    continue;
                }
            };
            let chars: usize = text.split_whitespace().map(str::len).sum();
            if chars < NATIVE_CHARS_PER_PAGE * pages.max(1) as usize {
                scans += 1;
                scan_pages += pages;
                visited.push((item, sum));
                continue;
            }
            let doc = doc(
                law,
                &uid,
                pdf,
                lj_parse::journal_text_articles(&text),
                lj_parse::journal_text_body(&text),
            );
            if self.dry_run {
                tracing::info!(
                    publisher = self.source,
                    key = uid,
                    articles = doc.articles.len(),
                    "law read"
                );
                continue;
            }
            match load_doc(
                &repo,
                &doc,
                self.read_on,
                self.source,
                &mut tally.publie_targets,
            )
            .await
            {
                Ok(_) => {
                    tally.written += 1;
                    visited.push((item, sum));
                }
                Err(e) => {
                    tracing::error!(publisher = self.source, key = uid, error = %e, "followed text failed");
                    tally.failed.push(uid);
                }
            }
            if visited.len() >= 100 {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        if !self.dry_run {
            repo.put_norm_visits(self.source, &visited)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        self.budget_spent(self.jurisdiction, left);
        if scans > 0 {
            tracing::warn!(
                publisher = self.source,
                scans,
                scan_pages,
                "laws without a text layer are visited and not written"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn law(item: u64, number: &str) -> AnbeninLaw {
        AnbeninLaw {
            item,
            nature: "LOI",
            number: number.to_string(),
            day: None,
            title: format!("Loi n° {number}"),
            modified: None,
            media: None,
        }
    }

    #[test]
    fn a_number_entered_twice_belongs_to_its_latest_item() {
        let laws = [
            law(10, "2016-06"),
            law(12, "2016-06"),
            law(11, "91-010 bis"),
        ];
        let ids = identities(&laws);
        assert_eq!(ids[&10], None);
        assert_eq!(ids[&12].as_deref(), Some("BJ/LOI-2016-06"));
        assert_eq!(ids[&11].as_deref(), Some("BJ/LOI-91-010-BIS"));
    }
}
