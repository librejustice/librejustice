//! Purge des citations procédurales stockées (ADR 0211) — depuis l'ADR 0250
//! le stock est fidèle (le filtre à la persistance a disparu) : cette passe
//! n'est conservée que comme INVERSE exact du backfill 0250 (rollback),
//! à supprimer après validation. DELETE des spans par paire denylist
//! (`lj_core::procedural`) puis resync des composites décision/chunks.
//! Idempotent (#7) : une deuxième passe ne supprime rien.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

pub async fn purge_procedural_citations() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Passe de maintenance : DELETE de masse + resync, hors borne interactive.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let pairs = repo.procedural_ref_pairs().await?;
    tracing::info!(paires = pairs.len(), "denylist résolue contre le catalogue");
    let supprimees = repo.delete_procedural_citations(&pairs).await?;
    tracing::info!(
        supprimees,
        "décisions legal_citation réécrites (spans procéduraux retirés)"
    );

    let Some((_, max_id)) = repo.decision_id_bounds().await? else {
        return Ok(());
    };
    const BATCH: i64 = 20_000;
    let (mut d_total, mut c_total) = (0i64, 0i64);
    let mut lo = 0i64;
    while lo <= max_id {
        let (d, c) = repo.resync_legal_arrays_range(lo, lo + BATCH).await?;
        d_total += d;
        c_total += c;
        lo += BATCH;
        if (lo / BATCH) % 25 == 0 {
            tracing::info!(lo, max_id, d_total, c_total, "resync composites");
        }
    }
    tracing::info!(d_total, c_total, "composites resynchronisés");
    Ok(())
}
