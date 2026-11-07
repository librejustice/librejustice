//! The decisions of the Autorité de la concurrence and of the Conseil de la concurrence, from data.gouv.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use lj_core::decision::Decision;
use lj_parse::{adlc_is_decision, adlc_number, adlc_source_fields, adlc_text, FromSourceFields};
use lj_sources::adlc::{fetch_latest, prune_older, read_records, written_marker};
use serde_json::Value;

use crate::config::Settings;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{
    content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode, BATCH_SIZE,
};

/// A record holding less text than this is a page with no decision on it.
const MIN_TEXT_CHARS: usize = 300;

/// Writes the decisions of a publication not yet written, of the file held with `IngestMode::All`.
pub async fn sync_adlc(mode: IngestMode) -> Result<()> {
    let dir = Settings::from_env()?.paths().adlc();
    let publication = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || fetch_latest(&dir)).await??
    };
    let marker = written_marker(&publication);
    if mode == IngestMode::MissingHash && marker.exists() {
        tracing::info!(file = %publication.display(), "adlc: publication already written");
        return Ok(());
    }
    let records = read_records(&publication)?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let mut counts = IngestCounts::default();
    let mut batch = Vec::new();
    let mut seen = HashSet::new();
    let (mut repeated, mut without_text) = (0usize, 0usize);
    for record in records.iter().filter(|r| adlc_is_decision(r)) {
        let number = adlc_number(record).ok_or_else(|| anyhow!("adlc record without a number"))?;
        let text = adlc_text(record).unwrap_or_default();
        if text.chars().count() < MIN_TEXT_CHARS {
            without_text += 1;
            continue;
        }
        if !seen.insert(number) {
            repeated += 1;
            continue;
        }
        batch.push(candidate(number, record, text)?);
        if batch.len() >= BATCH_SIZE {
            drain_batch(&conn, std::mem::take(&mut batch), mode, &mut counts).await?;
        }
    }
    if !batch.is_empty() {
        drain_batch(&conn, batch, mode, &mut counts).await?;
    }
    std::fs::write(&marker, "")?;
    prune_older(&dir, &publication)?;
    tracing::info!(
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        repeated,
        without_text,
        "adlc"
    );
    Ok(())
}

fn candidate(number: &str, record: &Value, text: &str) -> Result<Candidate> {
    let uid = format!("adlc/{number}");
    let source_fields = adlc_source_fields(record);
    Ok(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: content_checksum(&serde_json::to_vec(record)?),
        decision: Decision::from_source_fields(text, &source_fields, &uid),
        prebuilt_source_fields: Some(source_fields),
        prebuilt_extracted: None,
        raw_payload: Vec::new(),
        payload_format: "json".to_string(),
        write_mode: super::WriteMode::Full,
        passages: true,
        distinct: false,
    })
}
