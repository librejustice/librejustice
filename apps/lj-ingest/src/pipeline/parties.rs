//! A decision's actors: the full resolution passes. The write itself happens
//! with the extraction (`update_extracted_fields`, ingest).

use crate::config::Settings;
use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

/// Resolution of pending actors. Default (weekly): the normalised-firm pass alone,
/// the only one without an at-write twin, then the directory recount. `--full`:
/// every full pass, once after a SIRENE/RNA reload.
pub async fn relink_parties(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    relink_with(&DecisionRepository::new(&conn), full).await
}

pub(crate) async fn relink_with(repo: &DecisionRepository<'_>, full: bool) -> Result<()> {
    let linked = if full {
        repo.resolve_pending_parties().await?
    } else {
        repo.resolve_pending_cabinets().await?
    };
    let (total, resolved, siren, rna, cnb, oacc) = repo.actor_stats().await?;
    tracing::info!(
        linked,
        total,
        resolved,
        siren,
        rna,
        cnb,
        oacc,
        "parties: resolution"
    );
    println!(
        "actors: {total}, {resolved} linked \
         ({siren} siren:, {rna} rna:, {cnb} cnb:, {oacc} oacc:), {linked} by this pass."
    );
    let annuaire = repo.refresh_annuaire().await?;
    tracing::info!(annuaire, "annuaire: counters refreshed");
    println!("annuaire: {annuaire} entities with litigation.");
    Ok(())
}
