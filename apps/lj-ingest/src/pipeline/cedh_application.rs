//! The cases the CEDH communicated to France, read for the French ruling they attack.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_extract::cedh::{introduced_on, CedhFacts};
use lj_extract::chrono::ChronoSnapshot;
use lj_sources::cedh::{parse_results, CedhClient, CedhResult, PAGE_SIZE};
use lj_store::repository::{CedhApplicationRow, DecisionRepository};

use crate::config::Settings;

pub async fn ingest_cedh_applications() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = super::europe::europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let client = CedhClient::new(&settings.egress_proxy)?.with_body_cache(settings.cache_dir());
    let rows = communicated(&client).await?;
    let held = repo.cedh_application_itemids().await?;
    let snap = chrono_snapshot(&repo).await?;
    let (mut added, mut addressed, mut empty) = (0usize, 0usize, 0usize);
    for row in rows.iter().filter(|r| !held.contains(&r.itemid)) {
        let text = client
            .body_text(&row.itemid)
            .await
            .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
        if text.trim().is_empty() {
            empty += 1;
            continue;
        }
        let column = |k: &str| row.columns[k].as_str().unwrap_or("");
        let communicated_on = day(column("kpdate").get(..10).unwrap_or(""))
            .ok_or_else(|| anyhow!("cedh {}: no kpdate", row.itemid))?;
        let target = CedhFacts::read(&text, &communicated_on.to_string())
            .and_then(|f| f.address(&snap))
            .and_then(|a| Some((a.court?, day(&a.date?)?)));
        let has_target = target.is_some();
        let inserted = repo
            .insert_cedh_application(&CedhApplicationRow {
                itemid: row.itemid.clone(),
                appnos: list(column("appno")),
                name: column("docname").to_string(),
                introduced_on: introduced_on(&text).and_then(|d| day(&d)),
                communicated_on,
                articles: list(column("article")),
                text,
                target,
            })
            .await?;
        added += usize::from(inserted);
        addressed += usize::from(inserted && has_target);
    }
    tracing::info!(
        listed = rows.len(),
        added,
        addressed,
        empty,
        "cedh communicated cases"
    );
    Ok(())
}

async fn communicated(client: &CedhClient) -> Result<Vec<CedhResult>> {
    let first = client
        .communicated_france_page(0)
        .await
        .map_err(|e| anyhow!("cedh communicated start=0: {e}"))?;
    let (count, mut rows) = parse_results(&first);
    for start in (PAGE_SIZE..count as usize).step_by(PAGE_SIZE) {
        let page = client
            .communicated_france_page(start)
            .await
            .map_err(|e| anyhow!("cedh communicated start={start}: {e}"))?;
        rows.extend(parse_results(&page).1);
    }
    Ok(rows)
}

async fn chrono_snapshot(repo: &DecisionRepository<'_>) -> Result<ChronoSnapshot> {
    let closed = repo
        .jurisdiction_successors()
        .await?
        .into_iter()
        .map(|(closed, _)| closed)
        .collect();
    Ok(ChronoSnapshot::with_closed(
        repo.load_jurisdictions()
            .await?
            .into_iter()
            .filter_map(|j| Some((j.code, j.jurisdiction_type, j.city?))),
        closed,
    ))
}

fn day(iso: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").ok()
}

fn list(column: &str) -> Vec<String> {
    column
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
