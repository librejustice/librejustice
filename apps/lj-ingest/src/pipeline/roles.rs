//! Rôles des textes publiés : backfill rejouable de `legal_text.role`
//! (classification conservatrice par motifs de titre + arêtes `legal_link`) et
//! alignement de `legal_link.verb` sur le repli verbe/nom courant. Exposé en
//! commande `backfill-text-roles`.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Commande `backfill-text-roles` : pool + migrations + les deux passes.
pub async fn backfill_text_roles() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Passes pleine-table sur l'ensemble des titres : au-delà du
    // statement_timeout de 30 s du pool.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let (individuel, habilitation, vehicule, modificatif) = repo.backfill_text_roles().await?;
    let verbs = repo.normalize_link_verbs().await?;
    // `legal_article.searchable` dépend du rôle : un rôle reclassé doit
    // re-dénormaliser les articles du texte.
    let denorm = repo.refresh_article_denorm(None).await?;
    tracing::info!(
        individuel,
        habilitation,
        vehicule,
        modificatif,
        verbs_alignes = verbs,
        denorm,
        "text roles backfilled"
    );
    Ok(())
}
