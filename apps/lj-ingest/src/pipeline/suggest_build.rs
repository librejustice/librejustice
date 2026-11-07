//! Autocomplete FST from a tenth of the decisions and articles, plus the `legal_text` titles.

use anyhow::{anyhow, Result};

use lj_store::repository::{DecisionRepository, SUGGEST_FST_KEY};
use lj_suggest::SuggestBuilder;

use crate::config::Settings;

/// One decision and one article in `SAMPLE_MODULO`: the suggest ranks by relative
/// frequency within a domain.
const SAMPLE_MODULO: i64 = 10;
const BATCH: i64 = 500;

/// Commande `build-suggest` : comptage → coupe → FST → blob `suggest_index`.
pub async fn build_suggest() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut builder = SuggestBuilder::new();

    // 1. Décisions échantillonnées (df côté jurisprudence).
    let mut last_id = 0i64;
    let mut docs = 0u64;
    loop {
        let batch = repo
            .suggest_decision_texts_batch(last_id, SAMPLE_MODULO, BATCH)
            .await?;
        let Some(&(max_id, _)) = batch.last() else {
            break;
        };
        last_id = max_id;
        docs += batch.len() as u64;
        let texts: Vec<String> = batch.into_iter().map(|(_, text)| text).collect();
        builder.count_decisions(&texts);
        if docs.is_multiple_of(20_000) {
            tracing::info!(
                docs,
                entries = builder.entries(),
                "build-suggest: decisions"
            );
        }
    }
    tracing::info!(docs, entries = builder.entries(), "decisions counted");

    // 2. Sampled articles (texts-side df).
    let mut last_id = 0i64;
    let mut arts = 0u64;
    loop {
        let batch = repo
            .suggest_article_texts_batch(last_id, SAMPLE_MODULO, BATCH)
            .await?;
        let Some(&(max_id, _)) = batch.last() else {
            break;
        };
        last_id = max_id;
        arts += batch.len() as u64;
        let texts: Vec<String> = batch.into_iter().map(|(_, text)| text).collect();
        builder.count_articles(&texts);
        if arts.is_multiple_of(100_000) {
            tracing::info!(arts, entries = builder.entries(), "build-suggest: articles");
        }
    }
    tracing::info!(arts, entries = builder.entries(), "articles counted");

    // 3. Titres legal_text injectés entiers, avec boost côté textes.
    let mut titles_kept = 0u64;
    for title in repo.suggest_text_titles().await? {
        if builder.inject_title(&title) {
            titles_kept += 1;
        }
    }
    tracing::info!(titles_kept, "titles injected");

    // 4. Coupe et FST → blob.
    let bytes = builder.build()?;
    repo.upsert_suggest_fst(SUGGEST_FST_KEY, &bytes).await?;
    tracing::info!("build-suggest done");
    Ok(())
}
