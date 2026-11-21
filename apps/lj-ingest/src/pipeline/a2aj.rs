//! A2AJ's Canadian case law: each court file read from disk, each language a provenance.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use lj_core::chamber::Seat;
use lj_parse::{
    a2aj_court, a2aj_names, parse_a2aj, A2ajCourt, A2ajDecision, A2ajLanguage, A2AJ_COURTS,
};
use lj_sources::a2aj::{rows, A2ajClient};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::run_stream;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// French first: a decision a row creates takes the language of its first text as its original.
const LANGUAGES: [A2ajLanguage; 2] = [A2ajLanguage::French, A2ajLanguage::English];

fn candidate(row: &Map<String, Value>, language: A2ajLanguage, parsed: A2ajDecision) -> Candidate {
    let court = parsed.court;
    let seat = Seat {
        jurisdiction_type: court.jurisdiction_type.to_string(),
        jurisdiction_code: Some(court.code.to_string()),
        ..Default::default()
    };
    let extracted = ExtractedFields {
        date_lecture: parsed
            .decision
            .date_lecture
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
        seat_paths: seat.paths(),
        seat: Some(seat),
        jurisdiction: Some(JurisdictionRow {
            code: court.code.to_string(),
            source_code: court.code.to_string(),
            jurisdiction_type: court.jurisdiction_type.to_string(),
            city: None,
            label: court.label.to_string(),
            place: Some(court.place.to_string()),
        }),
        case_name: Some(parsed.title).filter(|t| !t.is_empty()),
        ..Default::default()
    };
    let columns: Map<String, Value> = row
        .iter()
        .filter(|(name, _)| *name == "dataset" || name.ends_with(language.suffix()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(
            &serde_json::to_vec(&columns).expect("a JSON map serialises"),
        ),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: "json".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    }
}

/// What a court's rows gave, by language and by reason refused.
#[derive(Debug, Default)]
struct Tally {
    taken: BTreeMap<&'static str, usize>,
    refused: BTreeMap<String, usize>,
}

/// Reads a court's file and sends each language a row holds to `tx`, none when `tx` is `None`.
fn read_court(
    court: &A2ajCourt,
    path: &std::path::Path,
    tx: Option<&mpsc::Sender<Candidate>>,
) -> Result<Tally> {
    let mut tally = Tally::default();
    for row in rows(path)? {
        let row = row?;
        for language in LANGUAGES {
            match parse_a2aj(&row, language) {
                Ok(Some(parsed)) => {
                    *tally.taken.entry(language.suffix()).or_default() += 1;
                    if let Some(tx) = tx {
                        if tx.blocking_send(candidate(&row, language, parsed)).is_err() {
                            bail!("a2aj: the load stopped reading {}", court.dataset);
                        }
                    }
                }
                Ok(None) => {}
                Err(why) => {
                    *tally
                        .refused
                        .entry(format!("{}:{why:?}", language.suffix()))
                        .or_default() += 1
                }
            }
        }
    }
    tracing::warn!(
        court = court.dataset,
        taken = ?tally.taken,
        refused = ?tally.refused,
        "a2aj: court file read"
    );
    Ok(tally)
}

/// Loads each court file whose checksum moved since its last load; `dry_run` opens no database.
pub async fn sync_a2aj(courts: Vec<String>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = A2ajClient::new(&settings.egress_proxy)?;
    let dir = settings.cache_dir().join("a2aj");
    std::fs::create_dir_all(&dir)?;
    let read: Vec<&'static A2ajCourt> = match courts.as_slice() {
        [] => A2AJ_COURTS.iter().collect(),
        named => named
            .iter()
            .map(|c| a2aj_court(c).ok_or_else(|| anyhow!("a2aj: no court {c} is read")))
            .collect::<Result<_>>()?,
    };
    let checksums = client.checksums().await?;
    let mut files: Vec<(&'static A2ajCourt, PathBuf, PathBuf, String)> = Vec::new();
    for court in read {
        let path = dir.join(format!("{}.parquet", court.dataset));
        let fetched = dir.join(format!("{}.sha256", court.dataset));
        let loaded = dir.join(format!("{}.loaded", court.dataset));
        let checksum = checksums
            .get(court.dataset)
            .cloned()
            .ok_or_else(|| anyhow!("a2aj: the repository holds no file for {}", court.dataset))?;
        if std::fs::read_to_string(&loaded).ok().as_deref() == Some(checksum.as_str()) {
            tracing::info!(court = court.dataset, "a2aj: court file loaded already");
            continue;
        }
        if !path.exists()
            || std::fs::read_to_string(&fetched).ok().as_deref() != Some(checksum.as_str())
        {
            let bytes = client.download(court.dataset, &path).await?;
            std::fs::write(&fetched, &checksum)?;
            tracing::info!(court = court.dataset, bytes, "a2aj: court file fetched");
        }
        files.push((court, path, loaded, checksum));
    }
    if dry_run {
        return tokio::task::spawn_blocking(move || {
            for (court, path, _, _) in &files {
                read_court(court, path, None)?;
            }
            Ok(())
        })
        .await?;
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let marks: Vec<(PathBuf, String)> = files
        .iter()
        .map(|(_, _, loaded, checksum)| (loaded.clone(), checksum.clone()))
        .collect();
    let counts = run_stream(
        &conn,
        &write_conn,
        IngestMode::MissingHash,
        |tx| async move {
            tokio::task::spawn_blocking(move || {
                for (court, path, _, _) in &files {
                    read_court(court, path, Some(&tx))?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
            Ok(IngestCounts::default())
        },
    )
    .await?;
    for (loaded, checksum) in marks {
        std::fs::write(loaded, checksum)?;
    }
    let named = DecisionRepository::new(&write_conn)
        .write_jurisdiction_names(&a2aj_names())
        .await?;
    tracing::warn!(
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        named,
        "a2aj sync done"
    );
    Ok(())
}
