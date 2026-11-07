//! Job `usage-terms` : matérialise `legal_article_usage` — sacs de n-grammes
//! des contextes de citation (signal d'usage). Toutes les identités passent par le
//! graphe `legal_citation`, citations procédurales comprises.
//!
//! Le coût est dominé par la lecture TOAST des `full_text` échantillonnés (I/O) :
//! les chunks, indépendants par construction, sont répartis sur `workers`
//! connexions.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::config::Settings;

pub async fn run(chunks: usize, min_citations: i64, workers: usize) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, workers).map_err(|e| anyhow!("build_pool: {e}"))?;

    let identities = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let identities = repo.usage_terms_pool(min_citations).await?;
        println!(
            "pool: {} identities (>={min_citations} citations), {chunks} chunks, {workers} workers",
            identities.len()
        );
        Arc::new(identities)
    };

    let next = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..workers {
        let pool = pool.clone();
        let identities = Arc::clone(&identities);
        let next = Arc::clone(&next);
        let total = Arc::clone(&total);
        set.spawn(async move {
            let per_chunk = identities.len().div_ceil(chunks);
            loop {
                let chunk = next.fetch_add(1, Ordering::Relaxed);
                if chunk >= chunks {
                    return Ok::<(), anyhow::Error>(());
                }
                let start = chunk * per_chunk;
                let end = (start + per_chunk).min(identities.len());
                let (uids, nums): (Vec<String>, Vec<String>) =
                    identities[start..end].iter().cloned().unzip();
                let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
                let repo = lj_store::repository::DecisionRepository::new(&conn);
                let n = repo.usage_terms_fill_batch(&uids, &nums).await?;
                total.fetch_add(n, Ordering::Relaxed);
                println!("chunk {chunk}/{chunks}: {n} bags");
            }
        });
    }
    while let Some(joined) = set.join_next().await {
        joined.map_err(|e| anyhow!("worker: {e}"))??;
    }
    println!("bags materialised: {}", total.load(Ordering::Relaxed));

    let (uids, nums): (Vec<String>, Vec<String>) = identities.iter().cloned().unzip();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let pruned = lj_store::repository::DecisionRepository::new(&conn)
        .usage_terms_prune(&uids, &nums)
        .await?;
    println!("bags pruned: {pruned}");

    Ok(())
}
