//! Derives `formation_paths` from the stored seat paths and bench axes, without
//! parsing. A dry run prints the decisions each path would hold.

use std::collections::BTreeMap;
use std::time::Instant;

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const PAGE: i64 = 20_000;
const WRITE_BATCH: usize = 1_000;

pub async fn resolve_formations(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0; SET synchronous_commit = off")
        .await?;
    let repo = DecisionRepository::new(&conn);

    let started = Instant::now();
    let mut after = 0i64;
    let (mut lues, mut changees, mut ecrites) = (0usize, 0usize, 0u64);
    let mut pending: Vec<(i64, Vec<String>)> = Vec::new();
    let mut paths_count: BTreeMap<String, i64> = BTreeMap::new();
    loop {
        let rows = repo.formation_rows_after(after, PAGE).await?;
        let Some((last, ..)) = rows.last() else {
            break;
        };
        after = *last;
        lues += rows.len();
        for (id, seat_paths, formation, office, procedure, stored) in rows {
            let paths = lj_core::formation::formation_paths(
                &seat_paths,
                formation.as_deref(),
                office.as_deref(),
                procedure.as_deref(),
            );
            for p in &paths {
                *paths_count.entry(p.clone()).or_default() += 1;
            }
            if paths != stored {
                changees += 1;
                pending.push((id, paths));
            }
        }
        if pending.len() >= WRITE_BATCH {
            ecrites += flush(&repo, &mut pending, dry_run).await?;
        }
        if lues % 500_000 == 0 {
            tracing::info!(lues, changees, ecrites, "resolve-formations: running");
        }
    }
    ecrites += flush(&repo, &mut pending, dry_run).await?;
    tracing::info!(
        lues,
        changees,
        ecrites,
        dry_run,
        secondes = started.elapsed().as_secs(),
        "resolve-formations: done"
    );
    if dry_run {
        for (path, n) in &paths_count {
            println!("{path}\t{n}");
        }
    }
    Ok(())
}

async fn flush(
    repo: &DecisionRepository<'_>,
    pending: &mut Vec<(i64, Vec<String>)>,
    dry_run: bool,
) -> Result<u64> {
    let written = if dry_run || pending.is_empty() {
        0
    } else {
        repo.put_formation_paths(pending).await?
    };
    pending.clear();
    Ok(written)
}
