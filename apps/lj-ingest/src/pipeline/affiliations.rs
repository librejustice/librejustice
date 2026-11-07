//! Counsel to law firm affiliations derived from the decisions.
//!
//! The pairing is read when the actors are built (`firm_ord` in the actor
//! document); only the pairs resolved at both ends are kept. Each run recomputes
//! its edges in full and replaces the previous ones. Dry run by default,
//! `--apply` to write.

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Provenance des arêtes tirées du corpus.
const SRC: &str = "decision";

/// Support minimal : nombre de décisions distinctes attestant le couple.
const SUPPORT: i64 = 2;

pub async fn derive_affiliations(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let derived = repo.affiliation_from_decisions(SUPPORT).await?;
    let avocats = derived
        .iter()
        .map(|(e, _)| e.entity_uid.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let structures = derived
        .iter()
        .map(|(e, _)| e.structure_uid.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let support_max = derived.iter().map(|(_, n)| *n).max().unwrap_or(0);

    println!(
        "affiliations dérivées (support ≥ {SUPPORT}) : {} arêtes, {avocats} avocats, \
         {structures} structures, support max {support_max}",
        derived.len()
    );
    if !apply {
        println!("(dry-run, re-run with --apply to write)");
        return Ok(());
    }

    let edges: Vec<_> = derived.into_iter().map(|(e, _)| e).collect();
    conn.batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
        .await?;
    let dropped = repo.affiliation_source_clear(SRC).await?;
    let mut written = 0u64;
    for batch in edges.chunks(20_000) {
        written += repo.affiliation_insert_bulk(SRC, batch).await?;
    }
    conn.batch_execute("COMMIT").await?;
    conn.batch_execute("ANALYZE entity_affiliation").await?;

    tracing::info!(
        edges = written,
        dropped,
        avocats,
        structures,
        "affiliations derived from the decisions"
    );
    println!("applied: {written} edges written, {dropped} replaced");
    Ok(())
}
