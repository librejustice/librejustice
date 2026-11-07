//! Single pass over the whole-corpus counters. These cannot be derived at
//! write time: a count of articles in force depends on `CURRENT_DATE`, which
//! moves without a row moving. Invariant this file holds: nothing counts the
//! corpus at read time, the home page reads `corpus_counts`.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// A connection with no statement timeout: every step below is a full-table
/// pass, well past the 30 s the pool arms on each connection.
async fn open() -> Result<(deadpool_postgres::Pool, deadpool_postgres::Object)> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute(
        "SET statement_timeout = 0; SET paradedb.enable_aggregate_custom_scan = off",
    )
    .await?;
    Ok((pool, conn))
}

/// The `precompute` command: replays the two corpus snapshots. Idempotent.
pub async fn precompute_all() -> Result<()> {
    resync_jurisdiction_counts().await?;
    resync_corpus_counts().await
}

/// The counters the jurisdiction hubs read, and the sitemap's page counts.
pub async fn resync_jurisdiction_counts() -> Result<()> {
    let (_pool, conn) = open().await?;
    let repo = DecisionRepository::new(&conn);
    let couples = repo.rebuild_jurisdiction_year_counts().await?;
    println!("jurisdiction_year_counts: {couples} pair(s) upserted.");
    Ok(())
}

/// The browse-mode facets and the three home-page totals.
pub async fn resync_corpus_counts() -> Result<()> {
    let (_pool, conn) = open().await?;
    let repo = DecisionRepository::new(&conn);
    let rows = repo.rebuild_corpus_counts().await?;
    println!("corpus_counts: {rows} row(s) upserted.");
    Ok(())
}
