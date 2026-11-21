//! The body of a European act CELLAR serves in no Formex: its French HTML, else its PDF's text layer.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;

use lj_norm::text_identity::text_uid_celex;
use lj_sources::cjue::CjueClient;
use lj_store::repository::{BodyWrite, DecisionRepository};

use crate::config::Settings;

/// The floor under which `empty_legal_text_uids` reads a body as absent.
const BODY_FLOOR: usize = 300;
const LOT: usize = 200;
const FAILURE_STREAK: usize = 20;

/// Gives their CELLAR text to the `EU/REG` and `EU/DIR` texts with neither body nor article, newest first;
/// with `refresh`, to every one with no numbered article, over its body.
pub async fn fill_eu_bodies(
    limit: Option<usize>,
    refresh: Option<NaiveDate>,
    dry_run: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let visited_path = settings.paths().eu_law().join("bodies-visited.json");
    let mut visited: BTreeMap<String, String> = match std::fs::read_to_string(&visited_path) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| anyhow!("{}: {e}", visited_path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => bail!("{}: {e}", visited_path.display()),
    };
    let uids: Vec<String> = if let Some(since) = refresh {
        let done = repo
            .eu_bodies_written_since(since)
            .await
            .map_err(|e| anyhow!("eu_bodies_written_since: {e}"))?;
        repo.eu_texts_without_articles()
            .await
            .map_err(|e| anyhow!("eu_texts_without_articles: {e}"))?
            .into_iter()
            .map(|(uid, _)| uid)
            .filter(|uid| !done.contains(uid))
            .collect()
    } else {
        repo.eu_texts_without_body()
            .await
            .map_err(|e| anyhow!("eu_texts_without_body: {e}"))?
    };
    let queue: Vec<(String, String)> = uids
        .into_iter()
        .filter_map(|uid| Some((text_uid_celex(&uid)?, uid)))
        .filter(|(celex, _)| refresh.is_some() || !visited.contains_key(celex))
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let mode = if refresh.is_some() {
        BodyWrite::Replace
    } else {
        BodyWrite::IfAbsent
    };
    tracing::info!(queue = queue.len(), refresh = ?refresh, dry_run, "eu-bodies: queue");

    let client = CjueClient::new();
    let today = chrono::Utc::now().date_naive().to_string();
    let (mut lot, mut written, mut empty, mut failed, mut streak) = (Vec::new(), 0u64, 0, 0, 0);
    for (celex, uid) in &queue {
        let text = match client.resource_text_in(celex, "fra").await {
            Ok(text) => {
                streak = 0;
                text
            }
            Err(e) => {
                failed += 1;
                streak += 1;
                tracing::warn!(celex, error = %e, "eu-bodies: CELLAR failed");
                if streak == FAILURE_STREAK {
                    break;
                }
                continue;
            }
        };
        let body = text
            .map(|t| lj_core::normalizer::normalize_chars(lj_parse::eurlex_act_text(&t)))
            .filter(|b| b.chars().count() >= BODY_FLOOR);
        match body {
            Some(body) if dry_run => {
                let head: String = body.chars().take(160).collect();
                println!(
                    "{celex}\t{}\t{}",
                    body.chars().count(),
                    head.replace('\n', " | ")
                );
            }
            Some(body) => lot.push((uid.clone(), body)),
            None => {
                empty += 1;
                visited.insert(celex.clone(), today.clone());
            }
        }
        if lot.len() == LOT {
            written += write_lot(&repo, &mut lot, mode, &visited, &visited_path).await?;
        }
    }
    if !dry_run {
        written += write_lot(&repo, &mut lot, mode, &visited, &visited_path).await?;
    }
    tracing::info!(
        queue = queue.len(),
        written,
        empty,
        failed,
        "eu-bodies: CELLAR texts written"
    );
    if streak == FAILURE_STREAK {
        bail!("eu-bodies: CELLAR failed {FAILURE_STREAK} times in a row");
    }
    Ok(())
}

async fn write_lot(
    repo: &DecisionRepository<'_>,
    lot: &mut Vec<(String, String)>,
    mode: BodyWrite,
    visited: &BTreeMap<String, String>,
    visited_path: &std::path::Path,
) -> Result<u64> {
    let n = repo
        .write_text_bodies("eu-law", lot, mode)
        .await
        .map_err(|e| anyhow!("write_text_bodies: {e}"))?
        .len() as u64;
    lot.clear();
    std::fs::write(visited_path, serde_json::to_vec(visited)?)
        .map_err(|e| anyhow!("{}: {e}", visited_path.display()))?;
    Ok(n)
}
