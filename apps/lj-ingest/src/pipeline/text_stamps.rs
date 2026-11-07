//! `stamp-texts`: dates the JORF texts without a publication date from their JO
//! issue and the undated instruments from their publishing decree, restamps every
//! text's date and jurisdiction paths, then copies them onto its articles. The
//! writers stamp at write time, so a pass that rewrites anything says the type
//! table changed or a writer skipped it.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const BATCH: i64 = 5_000;

pub async fn stamp_texts() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let published = repo.date_texts_from_publications().await?;
    if !published.is_empty() {
        tracing::warn!(
            texts = published.len(),
            "stamp-texts: texts dated from their JO issue"
        );
    }
    let dated = repo.date_instruments_from_vehicles(None).await?;
    if !dated.is_empty() {
        tracing::warn!(
            instruments = dated.len(),
            "stamp-texts: instruments dated from their publishing decree"
        );
    }
    let (mut after, mut texts, mut articles) = (0i64, 0usize, 0u64);
    // The articles follow every text read, not only the texts rewritten: a text
    // stamped by another writer leaves its articles behind.
    while let (Some(last), read, rewritten) = repo.stamp_legal_texts_after(after, BATCH).await? {
        after = last;
        texts += rewritten;
        articles += repo.refresh_article_denorm(Some(&read)).await?;
        tracing::info!(after, texts, articles, "stamp-texts: batch");
    }
    if texts > 0 || articles > 0 {
        tracing::warn!(
            texts,
            articles,
            "stamp-texts: texts restamped, articles realigned"
        );
    } else {
        tracing::info!("stamp-texts: every text and article already stamped");
    }
    Ok(())
}
