//! DGLex's German texts of the German-speaking Community's acts, hung on their versions in force.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::Utc;
use lj_parse::{ConsolidatedArticle, DglexListed, JustelListed};
use lj_sources::dglex::DglexClient;
use lj_store::repository::{ArticleLanguageWrite, DecisionRepository};
use regex::Regex;

use super::{read_listing, refused, NormsScan, RELIST_AFTER_DAYS};
use crate::config::Settings;

const SOURCE: &str = "dglex";
/// The Community's decrees and orders, authentic in German.
const COMMUNITY: [&str; 2] = ["569", "570"];
/// The federal texts held that DGLex gives in German: its kind, the adoption day, the text.
const FEDERAL: [(&str, &str, &str); 1] = [("571", "1994-02-17", "BE/CONSTITUTION-1994")];
/// The least share of a document's numbers its French text holds, and its lead over the next.
const MATCH: f64 = 0.5;
const LEAD: f64 = 0.15;

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

/// An act's kind as both languages name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Decree,
    Order,
    Ministerial,
    Other,
}

/// `Programmdekret 2025`, `ERB zur …`, `Ministerieller Erlass …`, `EM zur …`.
fn german_kind(title: &str) -> Kind {
    let folded = lj_fold::fold(title);
    let first = folded.split_whitespace().next().unwrap_or_default();
    match first {
        _ if first.ends_with("dekret") || first == "d." => Kind::Decree,
        "ministerieller" | "em" | "emb" => Kind::Ministerial,
        "erlass" | "er" | "erb" | "erk" => Kind::Order,
        _ => Kind::Other,
    }
}

/// `Décret …`, `Arrêté ministériel …`, `[Arrêté du Gouvernement …]`.
fn french_kind(heading: &str) -> Kind {
    let folded = lj_fold::fold(heading);
    let folded = folded.trim_start_matches(['[', '(', ' ']);
    match folded {
        _ if folded.starts_with("decret") => Kind::Decree,
        _ if folded.starts_with("arrete ministeriel") => Kind::Ministerial,
        _ if folded.starts_with("arrete") => Kind::Order,
        _ => Kind::Other,
    }
}

/// The be-cde acts of `listed`'s day and kind, those of its Moniteur day when there are any.
fn candidates<'a>(listed: &DglexListed, acts: &'a [JustelListed]) -> Vec<&'a JustelListed> {
    let kind = german_kind(&listed.title);
    let same: Vec<&JustelListed> = acts
        .iter()
        .filter(|a| a.adopted.is_some() && a.adopted == listed.adopted)
        .filter(|a| french_kind(&a.heading) == kind)
        .collect();
    let published: Vec<&JustelListed> = same
        .iter()
        .copied()
        .filter(|a| a.published.is_some() && a.published == listed.published)
        .collect();
    if published.is_empty() {
        same
    } else {
        published
    }
}

/// How many times each number appears in `texts`, leading zeros dropped.
fn numbers<'a>(texts: impl Iterator<Item = &'a str>) -> HashMap<&'a str, u32> {
    let mut counts = HashMap::new();
    for text in texts {
        for m in NUMBER.find_iter(text) {
            let n = m.as_str().trim_start_matches('0');
            *counts
                .entry(if n.is_empty() { "0" } else { n })
                .or_default() += 1;
        }
    }
    counts
}

/// The weighted Jaccard of two counts of numbers.
fn overlap(a: &HashMap<&str, u32>, b: &HashMap<&str, u32>) -> f64 {
    let (mut shared, mut all) = (0u32, 0u32);
    for (n, &x) in a {
        let y = b.get(n).copied().unwrap_or(0);
        shared += x.min(y);
        all += x.max(y);
    }
    all += b
        .iter()
        .filter(|(n, _)| !a.contains_key(*n))
        .map(|(_, &y)| y)
        .sum::<u32>();
    if all == 0 {
        0.0
    } else {
        f64::from(shared) / f64::from(all)
    }
}

/// The candidate whose French text holds the document's numbers, clear of the others.
fn chosen(scores: &[f64]) -> Option<usize> {
    let mut ranked: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, score) = *ranked.first()?;
    let next = ranked.get(1).map_or(0.0, |r| r.1);
    (score >= MATCH && score - next >= LEAD).then_some(best)
}

fn cached_path(cache: &Path, document: &str) -> PathBuf {
    cache.join("dglex").join(format!("{document}.xml.zst"))
}

fn cached(cache: &Path, document: &str) -> Result<Option<String>> {
    match std::fs::read(cached_path(cache, document)) {
        Ok(bytes) => Ok(Some(String::from_utf8(zstd::decode_all(&bytes[..])?)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn cache(cache: &Path, document: &str, xml: &str) -> Result<()> {
    let path = cached_path(cache, document);
    std::fs::create_dir_all(path.parent().expect("a cached document has a folder"))?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, zstd::encode_all(xml.as_bytes(), 3)?)?;
    std::fs::rename(&partial, &path)?;
    Ok(())
}

/// A document matched to its text: the standing of its German, its articles, its file's checksum.
struct Matched {
    listed: DglexListed,
    text_uid: String,
    status: &'static str,
    articles: Vec<ConsolidatedArticle>,
    checksum: u64,
}

pub(super) async fn sync(
    scan: NormsScan,
    limit: Option<usize>,
    fresh: bool,
    minutes: Option<u64>,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let deadline = minutes.map(|m| Instant::now() + Duration::from_secs(m * 60));
    let cache_dir = settings.cache_dir();
    let (_, acts) = read_listing(&cache_dir.join("justel").join("BE-CDE.tsv"))?
        .ok_or_else(|| anyhow!("no Justel listing of BE-CDE: sync-norms be-cde lists it"))?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let visits = match fresh {
        true => HashMap::new(),
        false => repo
            .norm_visits(SOURCE)
            .await
            .map_err(|e| anyhow!("norm_visits {SOURCE}: {e}"))?,
    };
    let refetch_before = Utc::now() - chrono::Duration::days(RELIST_AFTER_DAYS);
    let client = DglexClient::new(&settings.egress_proxy)?;

    let mut listed: Vec<(&str, DglexListed)> = Vec::new();
    for kind in COMMUNITY.into_iter().chain(FEDERAL.map(|(k, _, _)| k)) {
        if listed.iter().any(|(k, _)| *k == kind) {
            continue;
        }
        let documents = client.listing(kind).await?;
        tracing::info!(kind, documents = documents.len(), "dglex kind listed");
        listed.extend(documents.into_iter().map(|d| (kind, d)));
    }

    let (mut matched, mut unmatched, mut fetched, mut left) = (Vec::new(), 0usize, 0usize, 0usize);
    let (mut unserved, mut no_article) = (0usize, 0usize);
    let mut failed = Vec::new();
    for (kind, document) in listed {
        let texts: Vec<String> = if COMMUNITY.contains(&kind) {
            candidates(&document, &acts)
                .iter()
                .map(|a| format!("BE-CDE/{}", a.numac))
                .collect()
        } else {
            FEDERAL
                .iter()
                .filter(|(k, day, _)| *k == kind && document.adopted.as_deref() == Some(*day))
                .map(|(_, _, uid)| uid.to_string())
                .collect()
        };
        if texts.is_empty() {
            tracing::debug!(title = document.title, adopted = ?document.adopted, "dglex document has no candidate text");
            unmatched += 1;
            continue;
        }
        let refetch = visits
            .get(&document.document)
            .is_some_and(|v| scan == NormsScan::Relist && v.visited_at < refetch_before);
        let xml = match cached(&cache_dir, &document.document)? {
            Some(xml) if !refetch => xml,
            _ => {
                if deadline.is_some_and(|d| Instant::now() > d)
                    || limit.is_some_and(|l| fetched >= l)
                {
                    left += 1;
                    continue;
                }
                fetched += 1;
                match client.xml(&document).await {
                    Ok(Some(xml)) => {
                        cache(&cache_dir, &document.document, &xml)?;
                        xml
                    }
                    Ok(None) => {
                        tracing::warn!(
                            document = document.document,
                            title = document.title,
                            "dglex serves no pdf for the document"
                        );
                        unserved += 1;
                        continue;
                    }
                    Err(e) => {
                        let e = anyhow::Error::from(e);
                        tracing::error!(document = document.document, error = %e, "dglex document failed");
                        failed.push(document.document.clone());
                        if refused(&e) {
                            break;
                        }
                        continue;
                    }
                }
            }
        };
        let parsed = match lj_parse::dglex_articles(&xml) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::info!(document = document.document, title = document.title, error = %e, "dglex document shows no article");
                no_article += 1;
                continue;
            }
        };
        let german = numbers(parsed.iter().map(|a| a.texte.as_str()));
        let mut scores = Vec::with_capacity(texts.len());
        for uid in &texts {
            let french = repo
                .stored_article_versions(uid)
                .await
                .map_err(|e| anyhow!("stored_article_versions {uid}: {e}"))?;
            let french = numbers(
                french
                    .iter()
                    .filter(|v| v.date_fin.is_none())
                    .map(|v| v.texte.as_str()),
            );
            scores.push(overlap(&german, &french));
        }
        match chosen(&scores) {
            Some(i) => matched.push(Matched {
                checksum: xxhash_rust::xxh3::xxh3_64(xml.as_bytes()),
                text_uid: texts[i].clone(),
                listed: document,
                status: "authentic",
                articles: parsed,
            }),
            None => {
                tracing::debug!(
                    title = document.title,
                    ?texts,
                    ?scores,
                    "dglex document matches no text clearly"
                );
                unmatched += 1;
            }
        }
    }

    let mut by_text: HashMap<String, usize> = HashMap::new();
    for m in &matched {
        *by_text.entry(m.text_uid.clone()).or_default() += 1;
    }
    let (mut written, mut unchanged, mut hung, mut twice) = (0usize, 0usize, 0usize, 0usize);
    for m in &matched {
        if by_text[&m.text_uid] > 1 {
            tracing::warn!(
                text_uid = m.text_uid,
                title = m.listed.title,
                "two dglex documents match one text"
            );
            twice += 1;
            continue;
        }
        let versions = repo
            .article_version_ends(&m.text_uid)
            .await
            .map_err(|e| anyhow!("article_version_ends {}: {e}", m.text_uid))?;
        let articles: HashMap<String, &ConsolidatedArticle> = m
            .articles
            .iter()
            .map(|a| (lj_norm::article_key::identity_key(&a.num), a))
            .collect();
        let mut rows: Vec<ArticleLanguageWrite> = versions
            .iter()
            .filter(|(_, _, fin)| fin.is_none())
            .filter_map(|(id, num, _)| {
                let a = articles.get(&lj_norm::article_key::identity_key(num))?;
                Some(ArticleLanguageWrite {
                    article_id: *id,
                    text_status: m.status.to_string(),
                    texte: a.texte.clone(),
                    title_path: a.title_path.clone(),
                    content_checksum: i64::from_ne_bytes(m.checksum.to_ne_bytes()),
                })
            })
            .collect();
        rows.sort_by_key(|r| r.article_id);
        let ids: Vec<i64> = rows.iter().map(|r| r.article_id).collect();
        let visit =
            xxhash_rust::xxh3::xxh3_64(format!("{}{}{ids:?}", m.checksum, m.text_uid).as_bytes());
        if visits
            .get(&m.listed.document)
            .is_some_and(|v| v.content_checksum == visit)
        {
            unchanged += 1;
            continue;
        }
        hung += rows.len();
        written += 1;
        if dry_run {
            continue;
        }
        repo.replace_language_texts(&m.text_uid, "de", &rows)
            .await
            .map_err(|e| anyhow!("replace_language_texts {}: {e}", m.text_uid))?;
        repo.put_norm_visits(SOURCE, &[(m.listed.document.clone(), visit)])
            .await
            .map_err(|e| anyhow!("put_norm_visits {SOURCE}: {e}"))?;
    }
    tracing::info!(
        ?scan,
        matched = matched.len(),
        unmatched,
        twice,
        unserved,
        no_article,
        written,
        unchanged,
        hung,
        fetched,
        left,
        failed = failed.len(),
        dry_run,
        "sync-norms be-dg finished"
    );
    if !failed.is_empty() {
        anyhow::bail!(
            "{} dglex documents failed: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(numac: &str, heading: &str, adopted: &str, published: &str) -> JustelListed {
        JustelListed {
            numac: numac.into(),
            issuer: "Communauté germanophone".into(),
            heading: heading.into(),
            adopted: Some(adopted.into()),
            published: Some(published.into()),
        }
    }

    fn listed(title: &str, adopted: &str, published: &str) -> DglexListed {
        DglexListed {
            document: "d".into(),
            file: "d".into(),
            title: title.into(),
            adopted: Some(adopted.into()),
            published: Some(published.into()),
        }
    }

    #[test]
    fn a_document_is_offered_the_acts_of_its_day_and_kind_then_of_its_moniteur_day() {
        let acts = [
            act(
                "1",
                "Décret portant assentiment",
                "2025-03-24",
                "2025-04-10",
            ),
            act(
                "2",
                "[Arrêté du Gouvernement portant nomination]",
                "2025-03-24",
                "2025-04-10",
            ),
            act(
                "3",
                "Arrêté ministériel portant agrément",
                "2025-03-24",
                "2025-04-10",
            ),
            act(
                "4",
                "Arrêté du Gouvernement fixant",
                "2025-03-24",
                "2025-05-02",
            ),
            act(
                "5",
                "Arrêté du Gouvernement modifiant",
                "2025-03-25",
                "2025-04-10",
            ),
        ];
        let numacs = |l: &DglexListed| -> Vec<String> {
            candidates(l, &acts)
                .iter()
                .map(|a| a.numac.clone())
                .collect()
        };
        assert_eq!(
            numacs(&listed("Programmdekret 2025", "2025-03-24", "2025-04-10")),
            ["1"]
        );
        assert_eq!(
            numacs(&listed("ERB zur Bestellung", "2025-03-24", "2025-04-10")),
            ["2"]
        );
        assert_eq!(
            numacs(&listed("Erlass zur Festlegung", "2025-03-24", "2025-06-01")),
            ["2", "4"]
        );
        assert_eq!(
            numacs(&listed("EM zur Anerkennung", "2025-03-24", "2025-04-10")),
            ["3"]
        );
        assert!(numacs(&listed("GSO Förderausschuss", "2025-03-24", "2025-04-10")).is_empty());
    }

    #[test]
    fn the_text_holding_the_documents_numbers_clear_of_the_others_is_chosen() {
        let german = numbers(["Artikel 1 des Dekrets vom 06. Juni 2008, 1.250 Euro"].into_iter());
        let same = numbers(["L'article 1er du décret du 6 juin 2008, 1.250 euros"].into_iter());
        let other = numbers(["Le montant est fixé à 300 euros au 1er janvier 2009"].into_iter());
        assert_eq!(overlap(&german, &same), 1.0);
        assert!(overlap(&german, &other) < MATCH);
        assert_eq!(
            chosen(&[overlap(&german, &other), overlap(&german, &same)]),
            Some(1)
        );
        assert_eq!(chosen(&[0.9, 0.8]), None);
        assert_eq!(chosen(&[0.4]), None);
        assert_eq!(chosen(&[0.6]), Some(0));
    }
}
