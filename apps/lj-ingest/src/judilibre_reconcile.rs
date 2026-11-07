//! Judilibre against the base, one court and one decision-date slice at a
//! time: a slice whose count matches the base's is skipped, a year that does
//! not is counted month by month, and only the slices that still differ are
//! listed. What the base lacks is fetched into the cache and what Judilibre
//! answers 404 for is tombstoned there, both for the next `ingest`. An id
//! fetched once is not fetched again, only reported.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use lj_sources::downloader::{
    court_total, list_slices, month_slices, reconcile_fetch, reconcile_tombstone, slice_totals,
    year_slices, Manifest, Slice,
};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// A year holding more decisions than this is counted month by month.
const SLICE_SPLIT: usize = 20_000;

/// The base's live ids of one court, sorted by decision date.
struct Court {
    dated: Vec<(String, String)>,
    undated: usize,
}

impl Court {
    fn in_slice(&self, (start, end): &Slice) -> &[(String, String)] {
        let lo = self
            .dated
            .partition_point(|(d, _)| d.as_str() < start.as_str());
        let hi = self
            .dated
            .partition_point(|(d, _)| d.as_str() <= end.as_str());
        &self.dated[lo..hi]
    }
}

pub async fn run(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = crate::judilibre_client(&settings)?;
    let cache_dir = settings.cache_dir();
    let source_dir = settings.paths().judilibre();

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut live: HashSet<String> = HashSet::new();
    let mut tombstoned: HashSet<String> = HashSet::new();
    let mut courts: HashMap<String, Court> = HashMap::new();
    for p in DecisionRepository::new(&conn)
        .judilibre_provenances()
        .await?
    {
        let id = p
            .source_uid
            .strip_prefix("judilibre/")
            .ok_or_else(|| anyhow!("judilibre provenance without its prefix: {}", p.source_uid))?
            .to_string();
        if p.tombstoned {
            tombstoned.insert(id);
            continue;
        }
        let jurisdiction = p
            .jurisdiction
            .ok_or_else(|| anyhow!("judilibre provenance without a jurisdiction: {id}"))?;
        let court = courts.entry(jurisdiction).or_insert_with(|| Court {
            dated: Vec::new(),
            undated: 0,
        });
        match p.decision_date {
            Some(date) => court.dated.push((date, id.clone())),
            None => court.undated += 1,
        }
        live.insert(id);
    }
    for court in courts.values_mut() {
        court.dated.sort_unstable();
    }

    let manifest =
        Manifest::load(&source_dir.join("manifest.json")).map_err(|e| anyhow!("manifest: {e}"))?;
    let empty = Court {
        dated: Vec::new(),
        undated: 0,
    };
    let mut missing: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    let mut fetched_before = 0usize;
    let mut revived = 0usize;
    for jurisdiction in client.jurisdictions().await? {
        let ours = courts.get(&jurisdiction).unwrap_or(&empty);
        let total = court_total(&client, &jurisdiction)
            .await
            .map_err(|e| anyhow!("{jurisdiction}: {e}"))?;

        let mut differing: Vec<Slice> = Vec::new();
        let mut by_month: Vec<Slice> = Vec::new();
        let mut dated_total = 0usize;
        for (slice, n) in slice_totals(&client, &jurisdiction, year_slices())
            .await
            .map_err(|e| anyhow!("{jurisdiction}: {e}"))?
        {
            dated_total += n;
            if n == ours.in_slice(&slice).len() {
                continue;
            }
            if n <= SLICE_SPLIT || slice.0.starts_with("0001") {
                differing.push(slice);
            } else {
                by_month.extend(month_slices(&slice));
            }
        }
        for (slice, n) in slice_totals(&client, &jurisdiction, by_month)
            .await
            .map_err(|e| anyhow!("{jurisdiction}: {e}"))?
        {
            if n != ours.in_slice(&slice).len() {
                differing.push(slice);
            }
        }

        let listed: HashSet<String> = list_slices(&client, &jurisdiction, differing.clone())
            .await
            .map_err(|e| anyhow!("{jurisdiction}: {e}"))?
            .into_iter()
            .collect();
        let mut court_missing = 0usize;
        for id in &listed {
            if live.contains(id) || manifest.history_failed.contains(id) {
                continue;
            }
            if manifest.reconciled.contains(id) {
                fetched_before += 1;
                continue;
            }
            if tombstoned.contains(id) {
                revived += 1;
            }
            missing.push(id.clone());
            court_missing += 1;
        }
        let mut court_extra = 0usize;
        for slice in &differing {
            for (_, id) in ours.in_slice(slice) {
                if !listed.contains(id) {
                    extra.push(id.clone());
                    court_extra += 1;
                }
            }
        }
        tracing::info!(
            jurisdiction,
            total,
            ours = ours.dated.len() + ours.undated,
            undated_theirs = total.saturating_sub(dated_total),
            undated_ours = ours.undated,
            slices_listed = differing.len(),
            listed = listed.len(),
            missing = court_missing,
            extra = court_extra,
            "judilibre_reconcile_court"
        );
    }

    missing.sort();
    tracing::warn!(
        missing = missing.len(),
        revived,
        fetched_before_still_missing = fetched_before,
        extra = extra.len(),
        apply,
        "judilibre_reconcile"
    );
    if !apply || (missing.is_empty() && extra.is_empty()) {
        return Ok(());
    }

    let fetched = reconcile_fetch(&client, &cache_dir, &missing)
        .await
        .map_err(|e| anyhow!("fetch: {e}"))?;
    let marked = reconcile_tombstone(&client, &cache_dir, &extra)
        .await
        .map_err(|e| anyhow!("tombstone: {e}"))?;
    tracing::warn!(
        written = fetched.written,
        gone = fetched.gone.len(),
        failed = fetched.failed.len(),
        tombstoned = marked.len(),
        served_unlisted = extra.len() - marked.len(),
        "judilibre_reconcile_repaired"
    );
    Ok(())
}
