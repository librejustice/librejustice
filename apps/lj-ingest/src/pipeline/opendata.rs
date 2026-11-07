//! Pipelines opendata (XML) et Judilibre (JSON) + refetch ciblé.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use lj_pg::db::Connection;
use lj_sources::downloader::Manifest;
use lj_sources::judilibre::JudilibreClient;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::{drain_batch, run_stream};
use super::files::{collect_jsonl_gz, collect_zip_paths};
use super::prepare::{classify_judilibre, classify_xml};
use super::{Candidate, IngestCounts, IngestMode};

/// Ingests the opendata ZIPs under `data_dir`: each XML parsed, triaged by batch,
/// extracted and upserted.
pub async fn ingest_opendata(data_dir: &Path, mode: IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    // Connexion dédiée aux écritures : la phase amont du lot suivant (lecture
    // seule) recouvre l'écriture du lot courant.
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let zip_paths = collect_zip_paths(data_dir)?;
    if zip_paths.is_empty() {
        tracing::info!(dir = %data_dir.display(), "no opendata ZIP found");
        return Ok(());
    }

    // Manifeste downloader : fast-skip des ZIPs déjà entièrement ingérés. Sans
    // manifeste (chemin explicite hors `<cache>/opendata_conseil_etat`), aucun
    // skip — full scan.
    let manifest_path = data_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;
    let key_by_path: HashMap<PathBuf, String> = manifest
        .entries
        .iter()
        .map(|(key, entry)| (data_dir.join(&entry.path), key.clone()))
        .collect();

    let mut total = IngestCounts::default();
    let mut manifest_skipped = 0usize;
    for zip_path in &zip_paths {
        let manifest_key = key_by_path.get(zip_path);

        // Fast skip : ZIP déjà entièrement ingéré selon le manifeste (taille
        // identique au download). `fully_ingested` est invalidé par le downloader à
        // chaque re-download. Mode ALL : le fast-skip est bypassé.
        if let Some(entry) = manifest_key
            .filter(|_| mode != IngestMode::All)
            .and_then(|k| manifest.entries.get(k))
        {
            if entry.fully_ingested
                && entry.size.is_some()
                && entry.size == Some(zip_path.metadata()?.len())
            {
                manifest_skipped += 1;
                tracing::info!(source = %zip_path.display(), "ingest_zip_manifest_skip");
                continue;
            }
        }

        let zip_counts = drain_zip(&conn, &write_conn, zip_path, mode).await?;

        tracing::info!(
            source = %zip_path.display(),
            created = zip_counts.created,
            updated = zip_counts.updated,
            skipped = zip_counts.skipped,
            errors = zip_counts.errors,
            dropped = zip_counts.passages_dropped,
            kept = zip_counts.passages_kept,
            "ingest_zip"
        );
        total.merge(&zip_counts);

        // Marque le ZIP entièrement ingéré dans le manifeste, écrit immédiatement.
        if zip_counts.errors == 0 {
            if let Some(entry) = manifest_key.and_then(|k| manifest.entries.get_mut(k)) {
                entry.fully_ingested = true;
                manifest
                    .save(&manifest_path)
                    .map_err(|e| anyhow!("manifest save: {e}"))?;
            }
        }
    }

    tracing::info!(
        zips = zip_paths.len(),
        manifest_skipped,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        errors = total.errors,
        dropped = total.passages_dropped,
        kept = total.passages_kept,
        "ingest_total"
    );
    Ok(())
}

/// Reads an opendata ZIP, classifies each XML member and drains them by batch.
async fn drain_zip(
    conn: &Connection,
    write_conn: &Connection,
    zip_path: &Path,
    mode: IngestMode,
) -> Result<IngestCounts> {
    let archive_name = zip_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    let members = lj_sources::zip_reader::iter_decisions(zip_path)
        .map_err(|e| anyhow!("lecture {}: {e}", zip_path.display()))?;

    run_stream(conn, write_conn, mode, |tx| async move {
        let mut counts = IngestCounts::default();
        for (member, raw) in members {
            if raw.is_empty() {
                counts.skipped += 1;
                counts.empty_skipped += 1;
                continue;
            }
            match classify_xml(raw, &member, &archive_name) {
                Some(cand) => {
                    if tx.send(cand).await.is_err() {
                        break;
                    }
                }
                None => counts.errors += 1,
            }
        }
        Ok(counts)
    })
    .await
}

/// Ingère les archives Judilibre d'un dossier (JSON → DB).
///
/// Itère les `*/*.jsonl.gz` sous `data_dir`, parse chaque ligne JSON, même
/// triage/extract/upsert que l'opendata.
pub async fn ingest_judilibre(data_dir: &Path, mode: IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    // Propage les suppressions Judilibre (`tombstones.jsonl`) vers la base avant
    // tout ingest : une décision retirée par la Cour de cassation doit cesser
    // d'être servie même si le manifeste fast-skip court-circuite la boucle
    // d'ingest.
    crate::tombstones::prune_tombstones(data_dir, &DecisionRepository::new(&conn)).await?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let files = collect_jsonl_gz(data_dir)?;
    if files.is_empty() {
        tracing::info!(dir = %data_dir.display(), "no Judilibre file found");
        return Ok(());
    }

    // Manifeste downloader : fast-skip + resume incrémental. Mapping fichier →
    // MonthState : `<jur>/<key>.jsonl.gz` (`key` = YYYYMM ou `archive`).
    let manifest_path = data_dir.join("manifest.json");
    let mut manifest = Manifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut manifest_skipped = 0usize;
    let mut untyped: BTreeMap<String, usize> = BTreeMap::new();
    for path in &files {
        let jur = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let month_key = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .trim_end_matches(".jsonl.gz")
            .to_string();
        let file_size = path.metadata()?.len();
        // Mode ALL : le manifeste est ignoré (ni fast-skip ni resume), re-scan total.
        let month_state = manifest
            .jurisdictions
            .get(&jur)
            .and_then(|j| j.months.get(&month_key))
            .filter(|_| mode != IngestMode::All);

        // Fast skip : `.jsonl.gz` non modifié (taille identique) et déjà ingéré.
        if let Some(state) = month_state {
            if state.fully_ingested && state.ingested_size == Some(file_size) {
                manifest_skipped += 1;
                tracing::info!(source = %path.display(), "ingest_judilibre_manifest_skip");
                continue;
            }
        }

        // The `.jsonl.gz` is append-only: the lines already ingested are skipped
        // before any parse.
        let skip_first_lines =
            month_state.map_or(0, |state| state.ingested_lines.unwrap_or(0).max(0) as usize);

        let lines = lj_sources::downloader::read_jsonl_gz_lines(path)?;
        let lines_total = lines.len();
        if skip_first_lines > 0 {
            tracing::info!(
                source = %path.display(),
                skip_first_lines,
                lines_total,
                "ingest_judilibre_resume"
            );
        }
        let source = path.display().to_string();
        let mut untyped_lines = 0usize;
        let untyped_in_file = &mut untyped_lines;
        let file_counts = run_stream(&conn, &write_conn, mode, |tx| async move {
            let mut counts = IngestCounts::default();
            for line in lines.into_iter().skip(skip_first_lines) {
                if line.is_empty() {
                    continue;
                }
                match classify_judilibre(line) {
                    Ok(Some(cand)) => {
                        if tx.send(cand).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        *untyped_in_file += 1;
                        counts.errors += 1;
                    }
                    Err(e) => {
                        tracing::error!(source = %source, error = %e, "Judilibre parse failed");
                        counts.errors += 1;
                    }
                }
            }
            Ok(counts)
        })
        .await?;

        tracing::info!(
            source = %path.display(),
            created = file_counts.created,
            updated = file_counts.updated,
            skipped = file_counts.skipped,
            errors = file_counts.errors,
            dropped = file_counts.passages_dropped,
            kept = file_counts.passages_kept,
            "ingest_judilibre"
        );
        total.merge(&file_counts);
        if untyped_lines > 0 {
            *untyped.entry(jur.clone()).or_default() += untyped_lines;
        }

        // Marque le mois entièrement ingéré dans le manifeste, écrit immédiatement.
        if file_counts.errors == 0 {
            if let Some(state) = manifest
                .jurisdictions
                .get_mut(&jur)
                .and_then(|j| j.months.get_mut(&month_key))
            {
                state.fully_ingested = true;
                state.ingested_size = Some(file_size);
                state.ingested_lines = Some(lines_total as i64);
                manifest
                    .save(&manifest_path)
                    .map_err(|e| anyhow!("manifest save: {e}"))?;
            }
        }
    }

    tracing::info!(
        files = files.len(),
        manifest_skipped,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        errors = total.errors,
        dropped = total.passages_dropped,
        kept = total.passages_kept,
        "ingest_judilibre_total"
    );
    if !untyped.is_empty() {
        let listed: Vec<String> = untyped
            .iter()
            .map(|(jur, n)| format!("{jur} ({n})"))
            .collect();
        bail!(
            "Judilibre jurisdictions lj-parse does not type, left out: {}",
            listed.join(", ")
        );
    }
    Ok(())
}

/// Re-fetch ciblé de décisions Judilibre par id (`/decision`) puis ingest.
///
/// Répare une poignée de décisions désynchronisées (ex. résurrection :
/// supprimée puis re-créée côté Judilibre) **sans reculer le
/// `history_watermark` global**. Réutilise le chemin d'ingest normal (parse →
/// extract → upsert), idempotent par `content_checksum` (`MissingHash` :
/// un id déjà à jour est skippé).
pub async fn refetch_judilibre(client: &JudilibreClient, ids: &[String]) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let mut candidates: Vec<Candidate> = Vec::new();
    for id in ids {
        let payload = client
            .decision(id)
            .await
            .map_err(|e| anyhow!("refetch {id}: {e}"))?;
        let line = serde_json::to_vec(&payload).context("refetch: sérialisation payload")?;
        match classify_judilibre(line)? {
            Some(candidate) => {
                tracing::info!(id = %id, uid = %candidate.decision.source_uid, "refetch_judilibre_fetched");
                candidates.push(candidate);
            }
            None => tracing::warn!(id = %id, "refetch: decision not recognised, skip"),
        }
    }

    let mut counts = IngestCounts::default();
    drain_batch(&conn, candidates, IngestMode::MissingHash, &mut counts).await?;
    tracing::info!(
        requested = ids.len(),
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        dropped = counts.passages_dropped,
        kept = counts.passages_kept,
        "refetch_judilibre_total"
    );
    Ok(())
}
