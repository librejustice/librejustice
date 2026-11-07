//! A treaty record takes its articles from the decree that publishes it.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use lj_norm::treaty_text::{cut, is_decree_paragraph, title_lines, Entry, TreatyArticle};
use lj_store::repository::{DecisionRepository, Fraicheur, LegalArticleRow, TreatyCutCandidate};
use serde_json::Value;

use crate::config::Settings;

#[derive(Default, Debug)]
pub(super) struct TreatyCounts {
    treaties: usize,
    articles: usize,
    entries: usize,
    nothing: usize,
}

struct Listed {
    id: String,
    num: Option<String>,
    titles: Vec<String>,
}

fn walk(
    items: &Value,
    sections: &Value,
    out: &mut Vec<Listed>,
    seen: &mut HashSet<String>,
    titles: &mut Vec<String>,
) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(id) = item["article"].as_str() {
            if seen.insert(id.to_string()) {
                out.push(Listed {
                    id: id.to_string(),
                    num: item["num"].as_str().map(str::to_string),
                    titles: std::mem::take(titles),
                });
            }
        } else if let Some(id) = item["section"].as_str() {
            if seen.insert(id.to_string()) {
                let section = &sections[id];
                titles.extend(
                    title_lines(section["title"].as_str().unwrap_or_default())
                        .into_iter()
                        .map(str::to_string),
                );
                walk(&section["items"], sections, out, seen, titles);
            }
        }
    }
}

/// In reading order, a section's title before its first article, then what
/// the structure does not list, by id.
fn treaty_entries(c: &TreatyCutCandidate) -> Vec<(String, String)> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    walk(
        &c.structure,
        &c.sections,
        &mut order,
        &mut seen,
        &mut Vec::new(),
    );
    let mut rest: Vec<&String> = c
        .articles
        .as_object()
        .into_iter()
        .flat_map(|m| m.keys())
        .filter(|id| !seen.contains(*id))
        .collect();
    rest.sort();
    order.extend(rest.into_iter().map(|id| Listed {
        id: id.clone(),
        num: None,
        titles: Vec::new(),
    }));

    let mut titles: Vec<String> = Vec::new();
    order
        .into_iter()
        .filter_map(|l| {
            titles.extend(l.titles);
            let entry = c.articles.get(&l.id)?;
            if entry.get("held").is_some() {
                return None;
            }
            let text = entry["text"].as_str()?;
            let text = match l.num {
                Some(n) if n.bytes().all(|b| b.is_ascii_digit()) => return None,
                Some(n) => format!("{}\n{text}", n.to_uppercase()),
                None if is_decree_paragraph(text) => return None,
                None => text.to_string(),
            };
            titles.push(text);
            Some((l.id, std::mem::take(&mut titles).join("\n")))
        })
        .collect()
}

fn cut_entries(entries: &[(String, String)]) -> Vec<TreatyArticle> {
    cut(&entries
        .iter()
        .map(|(id, text)| Entry { id, text })
        .collect::<Vec<_>>())
}

async fn write(
    repo: &DecisionRepository<'_>,
    c: &TreatyCutCandidate,
    entries: &[(String, String)],
    articles: &[TreatyArticle],
) -> Result<()> {
    let today = chrono::Utc::now().date_naive();
    for (i, a) in articles.iter().enumerate() {
        let row = LegalArticleRow {
            text_uid: c.instrument_uid.clone(),
            num: a.num.clone(),
            num_key: a.num_key.clone(),
            position: Some(i as i32 + 1),
            title_path: a.title_path.clone(),
            status: String::new(),
            date_debut: c.date_publi,
            date_fin: None,
            texte: Some(a.text.clone()),
            texte_original: None,
            lang_original: None,
            translation: "officiel".to_string(),
            nota: None,
            content_checksum: xxhash_rust::xxh3::xxh3_64(a.text.as_bytes()),
            source: "jorf".to_string(),
            source_uid: a.source.clone(),
            source_url: None,
            source_asof: Some(today),
            source_upstream_url: None,
        };
        repo.upsert_legal_article(&row, Fraicheur::Autoritaire)
            .await
            .map_err(|e| {
                anyhow!(
                    "upsert_legal_article {} {}: {e}",
                    c.instrument_uid,
                    a.num_key
                )
            })?;
    }
    let ids: Vec<&str> = entries.iter().map(|(id, _)| id.as_str()).collect();
    repo.hold_publication_articles(&c.vehicle_uid, &ids, &c.instrument_uid)
        .await
        .map_err(|e| anyhow!("hold_publication_articles {}: {e}", c.vehicle_uid))?;
    Ok(())
}

/// Cuts the treaties `vehicles` publish, every treaty when `None`.
pub(super) async fn cut_treaty_articles(
    repo: &DecisionRepository<'_>,
    vehicles: Option<&[String]>,
) -> Result<TreatyCounts> {
    let candidates = repo
        .treaty_cut_candidates(vehicles)
        .await
        .map_err(|e| anyhow!("treaty_cut_candidates: {e}"))?;
    let mut counts = TreatyCounts::default();
    for c in &candidates {
        let entries = treaty_entries(c);
        let articles = cut_entries(&entries);
        if articles.is_empty() {
            counts.nothing += 1;
            continue;
        }
        repo.conn.batch_execute("BEGIN").await?;
        match write(repo, c, &entries, &articles).await {
            Ok(()) => repo.conn.batch_execute("COMMIT").await?,
            Err(e) => {
                repo.conn.batch_execute("ROLLBACK").await?;
                return Err(e);
            }
        }
        counts.treaties += 1;
        counts.articles += articles.len();
        counts.entries += entries.len();
    }
    Ok(counts)
}

/// Every treaty record still empty, then the finishers of the rows written.
/// `dry_run` prints what each treaty would get and writes nothing.
pub async fn treaty_articles(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if dry_run {
        for c in repo
            .treaty_cut_candidates(None)
            .await
            .map_err(|e| anyhow!("treaty_cut_candidates: {e}"))?
        {
            let entries = treaty_entries(&c);
            let articles = cut_entries(&entries);
            let chars: usize = articles.iter().map(|a| a.text.chars().count()).sum();
            let keys: Vec<&str> = articles.iter().map(|a| a.num_key.as_str()).collect();
            println!(
                "{}\t{}\t{}\t{}\t{chars}\t{}",
                c.instrument_uid,
                c.vehicle_uid,
                entries.len(),
                articles.len(),
                keys.join(",")
            );
        }
        return Ok(());
    }
    let counts = cut_treaty_articles(&repo, None).await?;
    tracing::warn!(?counts, "treaty articles: cut");
    repo.refresh_article_denorm(None)
        .await
        .map_err(|e| anyhow!("refresh_article_denorm: {e}"))?;
    repo.refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    Ok(())
}
