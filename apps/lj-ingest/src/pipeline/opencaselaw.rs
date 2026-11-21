//! OpenCaseLaw's dataset: the Obwalden entries, the file read whole each run, each entry
//! loaded by the standard upsert path, the unchanged ones skipped by their checksum.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use serde_json::Value;

use lj_core::chamber::Seat;
use lj_parse::{obwalden_names, parse_opencaselaw, OpenCaseLawParsed, OpenCaseLawRefused};
use lj_sources::opencaselaw::OpenCaseLawClient;
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::run_stream;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode};

/// The court files read, by their name in the dataset.
const COURTS: [&str; 1] = ["ow_gerichte"];

fn candidate(row: &serde_json::Map<String, Value>, parsed: OpenCaseLawParsed) -> Candidate {
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
        ..Default::default()
    };
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(
            &serde_json::to_vec(row).expect("a JSON map serialises"),
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

/// Reads each court file of [`COURTS`] and loads the entries [`parse_opencaselaw`] takes;
/// `dry_run` prints what it would load and refuse, by body and by reason, without the database.
pub async fn sync_opencaselaw(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = OpenCaseLawClient::new(&settings.egress_proxy)?;
    let mut rows = Vec::new();
    for court in COURTS {
        let file = client.court_rows(court).await?;
        tracing::info!(court, rows = file.len(), "opencaselaw: court file read");
        rows.extend(file);
    }
    let mut taken = Vec::new();
    let mut by_court: BTreeMap<&str, usize> = BTreeMap::new();
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    for row in &rows {
        match parse_opencaselaw(row) {
            Ok(parsed) => {
                *by_court.entry(parsed.court.id).or_default() += 1;
                taken.push(candidate(row, parsed));
            }
            Err(why) => {
                let why = match why {
                    OpenCaseLawRefused::BodyNotTaken(issuer) => format!("body: {issuer}"),
                    other => format!("{other:?}"),
                };
                *refused.entry(why).or_default() += 1;
            }
        }
    }
    tracing::warn!(
        rows = rows.len(),
        taken = taken.len(),
        by_court = ?by_court,
        refused = ?refused,
        "opencaselaw: entries read"
    );
    if dry_run {
        return Ok(());
    }
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let write_conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let named = DecisionRepository::new(&write_conn)
        .write_jurisdiction_names(&obwalden_names())
        .await?;
    tracing::info!(named, "opencaselaw: court names written");
    let counts = run_stream(
        &conn,
        &write_conn,
        IngestMode::MissingHash,
        |tx| async move {
            for c in taken {
                if tx.send(c).await.is_err() {
                    break;
                }
            }
            Ok(IngestCounts::default())
        },
    )
    .await?;
    tracing::warn!(
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        "opencaselaw sync done"
    );
    Ok(())
}
