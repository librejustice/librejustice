//! Job `usage-terms` (ADR 0248) : matérialise `legal_article_usage` — sacs de
//! n-grammes des contextes de citation (signal d'usage). Depuis l'ADR 0250
//! (citations procédurales stockées), toutes les identités passent par le
//! graphe `legal_citation` — y compris 700 CPC / L761-1.

use anyhow::{anyhow, Result};

use crate::config::Settings;

pub async fn run(chunks: i32, min_citations: i64) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);

    let pool_size = repo.usage_terms_build_pool(chunks, min_citations).await?;
    println!("pool : {pool_size} identités (≥{min_citations} citations), {chunks} chunks");
    let reset = repo.usage_terms_reset().await?;
    println!("reset : {reset} sacs remis à NULL");
    let mut total = 0u64;
    for chunk in 0..chunks {
        let n = repo.usage_terms_fill_chunk(chunk).await?;
        total += n;
        println!("chunk {chunk}/{chunks} : {n} sacs");
    }
    println!("sacs matérialisés : {total}");

    Ok(())
}
