//! Légimonaco: the decisions of Monaco's courts its index serves, each page loaded by the standard
//! upsert path then kept under `<state_dir>/ingest/legimonaco/pages/`.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use lj_core::chamber::Seat;
use lj_parse::{parse_legimonaco, LegimonacoParsed, MONACO};
use lj_sources::legimonaco::{Hit, Key, LegimonacoClient, PAGE};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::run_stream;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The daily run starts no page past this, the rest waiting for the next run.
const BUDGET: Duration = Duration::from_secs(1_200);

/// Pages the cursor trails the fetch by: the stream holds at most 1,280 candidates, 6.4 pages, a
/// crash can leave unwritten.
const CURSOR_LAG: usize = 8;

fn dir(settings: &Settings) -> PathBuf {
    settings.state_dir.join("ingest/legimonaco")
}

/// The last decision of the walk loaded, by the day the portal deployed it.
#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    after: Option<Key>,
}

impl Cursor {
    fn read(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(raw) => Ok(serde_json::from_slice(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn write(&self, path: &Path) -> Result<()> {
        let part = path.with_extension("part");
        std::fs::write(&part, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&part, path)?;
        Ok(())
    }
}

fn parse(hit: &Hit) -> Result<LegimonacoParsed> {
    Ok(parse_legimonaco(
        hit.source.get(),
        hit.abstract_body.as_deref(),
        hit.summary_body.as_deref(),
    )?)
}

/// The decision and its editorial sections, without its `deployDate`, which a redeployment of an
/// unchanged decision moves.
fn checksummed(hit: &Hit) -> Result<Vec<u8>> {
    let mut doc: Value = serde_json::from_str(hit.source.get())?;
    if let Some(doc) = doc.as_object_mut() {
        doc.remove("deployDate");
    }
    Ok(serde_json::to_vec(&json!([
        doc,
        hit.abstract_body,
        hit.summary_body
    ]))?)
}

fn candidate(hit: &Hit) -> Result<Candidate> {
    let parsed = parse(hit)?;
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
            place: Some(MONACO.to_string()),
        }),
        ..Default::default()
    };
    Ok(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(&checksummed(hit)?),
        prebuilt_source_fields: Some(parsed.source_fields),
        prebuilt_extracted: Some(extracted),
        decision: parsed.decision,
        raw_payload: Vec::new(),
        payload_format: "json".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
    })
}

/// The page as JSON lines, each decision with its editorial sections.
fn page_lines(hits: &[Hit]) -> Result<String> {
    let mut lines = String::new();
    for hit in hits {
        let source: Value = serde_json::from_str(hit.source.get())?;
        lines.push_str(&serde_json::to_string(&json!({
            "source": source,
            "abstract": hit.abstract_body,
            "summary": hit.summary_body,
        }))?);
        lines.push('\n');
    }
    Ok(lines)
}

fn page_size(limit: Option<usize>, read: usize) -> usize {
    limit.map_or(PAGE, |l| PAGE.min(l.saturating_sub(read)))
}

#[derive(Default)]
struct Progress {
    read: usize,
    refused: usize,
    ended: bool,
}

/// Reads the decisions the portal deployed since the cursor, the whole stock on the first run.
/// `full` spends no time budget; `limit` stops after that many documents; `dry_run` fetches and
/// parses from the start, counts the decisions by court, and loads nothing.
pub async fn sync_legimonaco(full: bool, limit: Option<usize>, dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = LegimonacoClient::new(&settings.egress_proxy)?;
    if dry_run {
        return parse_only(&client, limit).await;
    }
    let dir = dir(&settings);
    let pages = dir.join("pages");
    std::fs::create_dir_all(&pages)?;
    let cursor_path = dir.join("cursor.json");
    let mut cursor = Cursor::read(&cursor_path)?;

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    if !DecisionRepository::new(&conn).legimonaco_lock().await? {
        tracing::info!("legimonaco sync: another run holds the lock");
        return Ok(());
    }
    tracing::info!(after = ?cursor.after, "legimonaco sync: start");

    let mut progress = Progress::default();
    let mut last = cursor.after.clone();
    let (progress_ref, last_ref) = (&mut progress, &mut last);
    let (client, pages, cursor_ref, cursor_path) = (&client, &pages, &mut cursor, &cursor_path);
    let counts = run_stream(&conn, &write_conn, IngestMode::MissingHash, |tx| async move {
        let started = Instant::now();
        let mut trail: VecDeque<Key> = VecDeque::new();
        while limit.is_none_or(|l| progress_ref.read < l) && (full || started.elapsed() < BUDGET) {
            let hits = client
                .page(last_ref.as_ref(), page_size(limit, progress_ref.read))
                .await?;
            let Some(tail) = hits.last() else {
                progress_ref.ended = true;
                break;
            };
            for hit in &hits {
                match candidate(hit) {
                    Ok(c) => {
                        if tx.send(c).await.is_err() {
                            return Ok(IngestCounts::default());
                        }
                    }
                    Err(e) => {
                        tracing::debug!(id = %hit.key.content_id, error = %e, "legimonaco document refused");
                        progress_ref.refused += 1;
                    }
                }
            }
            let name = format!("{}-{}.jsonl", tail.key.deployed, tail.key.content_id);
            std::fs::write(pages.join(name), page_lines(&hits)?)?;
            progress_ref.read += hits.len();
            *last_ref = Some(tail.key.clone());
            trail.push_back(tail.key.clone());
            if trail.len() > CURSOR_LAG {
                cursor_ref.after = trail.pop_front();
                cursor_ref.write(cursor_path)?;
            }
            tracing::info!(
                read = progress_ref.read,
                refused = progress_ref.refused,
                "legimonaco sync"
            );
        }
        Ok(IngestCounts::default())
    })
    .await?;
    cursor.after = last;
    cursor.write(cursor_path)?;
    if !progress.ended {
        tracing::warn!(
            read = progress.read,
            "legimonaco sync: stopped before the end, the next run resumes"
        );
    }
    tracing::warn!(
        read = progress.read,
        refused = progress.refused,
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        "legimonaco sync done"
    );
    Ok(())
}

async fn parse_only(client: &LegimonacoClient, limit: Option<usize>) -> Result<()> {
    let mut by_court: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut after: Option<Key> = None;
    let (mut read, mut refused) = (0, 0);
    while limit.is_none_or(|l| read < l) {
        let hits = client.page(after.as_ref(), page_size(limit, read)).await?;
        let Some(tail) = hits.last() else {
            break;
        };
        for hit in &hits {
            match parse(hit) {
                Ok(parsed) => *by_court.entry(parsed.court.code).or_default() += 1,
                Err(e) => {
                    tracing::warn!(id = %hit.key.content_id, error = %e, "legimonaco document refused");
                    refused += 1;
                }
            }
        }
        read += hits.len();
        after = Some(tail.key.clone());
    }
    for (court, count) in &by_court {
        tracing::info!(court, count, "legimonaco dry run");
    }
    tracing::warn!(read, refused, "legimonaco dry run done");
    Ok(())
}
