//! Rôles des textes publiés (ADR 0246) : backfill rejouable de
//! `legal_text.role` (classification v1 conservatrice par motifs de titre +
//! arêtes `legal_link`) et alignement de `legal_link.verb` sur le repli
//! verbe/nom courant. Exposé en commande `backfill-text-roles`.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Commande `backfill-text-roles` : pool + migrations + les deux passes.
pub async fn backfill_text_roles() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Passes pleine-table sur 1,4 M de titres : au-delà du statement_timeout
    // de 30 s du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let (individuel, habilitation, vehicule) = repo.backfill_text_roles().await?;
    let verbs = repo.normalize_link_verbs().await?;
    tracing::info!(
        individuel,
        habilitation,
        vehicule,
        verbs_alignes = verbs,
        "rôles des textes backfillés"
    );
    Ok(())
}
