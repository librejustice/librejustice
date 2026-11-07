//! Orchestration d'un batch : write (I/O DB, tokio) + drain (triage → process).

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_pg::db::Connection;
use lj_store::repository::{
    BulkDecisionWrite, DecisionRepository, ExistingDecisionState, UpsertStatus,
};

use super::prepare::{
    extract_write, keep_unchanged_passages, prepare_write, to_bulk_write, triage_candidates,
};
use super::{
    Candidate, ExtractCtx, IngestCounts, IngestMode, PreparedDecision, WriteMode, BATCH_SIZE,
};

/// Écrit un batch en DB. Une transaction par batch.
async fn write_writes(conn: &Connection, writes: Vec<BulkDecisionWrite>) -> Result<IngestCounts> {
    let mut counts = IngestCounts::default();
    let repo = DecisionRepository::new(conn);
    conn.batch_execute("BEGIN").await?;

    let result = write_writes_inner(&repo, writes, &mut counts).await;
    match &result {
        Ok(()) => {
            conn.batch_execute("COMMIT").await?;
        }
        Err(_) => {
            let _ = conn.batch_execute("ROLLBACK").await;
        }
    }
    result.map(|()| counts)
}

async fn write_writes_inner(
    repo: &DecisionRepository<'_>,
    writes: Vec<BulkDecisionWrite>,
    counts: &mut IngestCounts,
) -> Result<()> {
    // Décisions écrites (créées/mises à jour) : cibles potentielles de
    // pendants plus anciens — sonde inversée en fin de lot.
    let mut fresh_ids: Vec<i64> = Vec::new();
    for write in writes {
        if write.write_mode == WriteMode::SourceXmlOnly.as_str() {
            let decision_id = write
                .decision_id
                .ok_or_else(|| anyhow!("backfill source_xml sans decision_id"))?;
            // Backfill du `public_id` (+ format) à contenu inchangé :
            // `source_uid`/`source_fields` vivent sur la provenance, qui existe déjà
            // ici. On complète seulement l'identifiant manquant.
            repo.set_public_id(decision_id, &write.public_id).await?;
            repo.set_payload_format(decision_id, &write.payload_format)
                .await?;
            counts.updated += 1;
            continue;
        }
        if let Some(e) = &write.extracted {
            if repo.is_withdrawn(e).await? {
                tracing::warn!(uid = %write.decision.source_uid, "record at a withdrawn address, not written");
                continue;
            }
        }

        let decision_id = match write.decision_id {
            None => {
                let result = repo
                    .upsert(
                        &write.decision,
                        &write.content_checksum,
                        &write.public_id,
                        write.extracted.as_ref(),
                        write.canonical_ref.as_deref(),
                        &write.source_fields,
                        &write.payload_format,
                        write.distinct,
                    )
                    .await?;
                match result.status {
                    UpsertStatus::Created => counts.created += 1,
                    UpsertStatus::Updated => counts.updated += 1,
                    UpsertStatus::Skipped => {
                        counts.skipped += 1;
                        continue;
                    }
                    UpsertStatus::Copy => {
                        counts.copies += 1;
                        continue;
                    }
                    UpsertStatus::Held => {
                        tracing::warn!(uid = %write.decision.source_uid, decision_id = result.id, "record held: neither the copy of that decision nor another one");
                        counts.held += 1;
                        continue;
                    }
                }
                result.id
            }
            Some(id) => {
                repo.update_existing(
                    id,
                    &write.decision,
                    &write.content_checksum,
                    &write.public_id,
                    write.extracted.as_ref(),
                    write.canonical_ref.as_deref(),
                    &write.source_fields,
                    &write.payload_format,
                )
                .await?;
                counts.updated += 1;
                id
            }
        };
        if write.drop_passages
            && repo
                .writes_passage_text(decision_id, &write.decision.source_uid)
                .await?
        {
            counts.passages_dropped += repo.drop_passages(&[decision_id]).await? as usize;
        }
        repo.set_payload_format(decision_id, &write.payload_format)
            .await?;
        fresh_ids.push(decision_id);
    }
    super::reextract::fuse_written(repo, &fresh_ids).await?;
    // Sonde inversée : les décisions du lot attirent les citations/liens
    // pendants qui les visaient. Dernier statement avant le COMMIT de
    // l'appelant : fenêtre minimale du verrou advisory de l'attract.
    super::attract::attract_pending_for(repo, &fresh_ids).await?;
    Ok(())
}

/// Stage **1** of a batch: state read (DB), triage, clean (CPU). Writes and
/// extracts nothing, and needs no extraction context.
async fn stage_prepare(
    conn: &Connection,
    candidates: Vec<Candidate>,
    mode: IngestMode,
) -> Result<(Vec<PreparedDecision>, IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let started = Instant::now();
    // Les tours de vidange du pipeline passent un lot vide : rien à demander.
    if candidates.is_empty() {
        return Ok((Vec::new(), counts, Duration::ZERO));
    }
    let uids: Vec<String> = candidates
        .iter()
        .map(|c| c.decision.source_uid.clone())
        .collect();
    let existing = fetch_states(conn, &uids).await?;
    let (survivors, skipped, deduped) = triage_candidates(candidates, &existing, mode);
    counts.skipped += skipped;
    counts.dedup_in_batch += deduped;
    if survivors.is_empty() {
        return Ok((Vec::new(), counts, started.elapsed()));
    }

    let rewritten: Vec<String> = survivors
        .iter()
        .filter(|c| c.decision_id.is_some() && c.write_mode == WriteMode::Full && c.passages)
        .map(|c| c.decision.source_uid.clone())
        .collect();
    let stored = DecisionRepository::new(conn)
        .passage_inputs(&rewritten)
        .await?;

    // On a blocking thread: `into_par_iter` holds its caller to the end.
    let prepared: Vec<Result<Option<PreparedDecision>>> = tokio::task::spawn_blocking(move || {
        survivors
            .into_par_iter()
            .map(|cand| {
                let stored = stored.get(&cand.decision.source_uid);
                Ok(prepare_write(cand)?.map(|p| match stored {
                    Some(stored) => keep_unchanged_passages(p, stored),
                    None => p,
                }))
            })
            .collect()
    })
    .await?;

    let mut writes = Vec::with_capacity(prepared.len());
    for item in prepared {
        match item? {
            Some(p) => {
                if p.write_mode == WriteMode::KeepPassages {
                    counts.passages_kept += 1;
                }
                writes.push(p);
            }
            None => counts.empty_skipped += 1,
        }
    }
    Ok((writes, counts, started.elapsed()))
}

/// Stage **2** of a batch: structured extraction (CPU) and the writes. The
/// only stage that needs the extraction context (linker, vocabulary).
async fn stage_extract(
    ctx: &'static ExtractCtx,
    items: Vec<PreparedDecision>,
) -> Result<(Vec<BulkDecisionWrite>, Duration)> {
    if items.is_empty() {
        return Ok((Vec::new(), Duration::ZERO));
    }
    let started = Instant::now();
    let extracted: Vec<Result<BulkDecisionWrite>> = tokio::task::spawn_blocking(move || {
        items
            .into_par_iter()
            .map(|prepared| extract_write(prepared, ctx).map(to_bulk_write))
            .collect()
    })
    .await?;
    let mut bulk = Vec::with_capacity(extracted.len());
    for item in extracted {
        bulk.push(item?);
    }
    Ok((bulk, started.elapsed()))
}

/// Précheck DB groupé des `source_uid` d'un batch.
async fn fetch_states(
    conn: &Connection,
    uids: &[String],
) -> Result<HashMap<String, ExistingDecisionState>> {
    let repo = DecisionRepository::new(conn);
    Ok(repo.find_ingest_states(uids).await?)
}

/// Triage and process one batch of candidates (states, triage, prepare,
/// extract, write), one transaction, without overlap: the path of the
/// low-volume sources and of the maintenance commands.
pub(super) async fn drain_batch(
    conn: &Connection,
    candidates: Vec<Candidate>,
    mode: IngestMode,
    counts: &mut IngestCounts,
) -> Result<()> {
    let (writes, staged, _) = stage_prepare(conn, candidates, mode).await?;
    counts.merge(&staged);
    if writes.is_empty() {
        return Ok(());
    }
    let ctx = super::extract_ctx(conn).await?;
    let (bulk, _) = stage_extract(ctx, writes).await?;
    let written = write_writes(conn, bulk).await?;
    counts.merge(&written);
    Ok(())
}

/// Time spent by kind of work over a [`run_stream`].
#[derive(Default)]
struct Phases {
    /// State read, triage, clean (DB read, then CPU).
    prepare: Duration,
    /// Structured extraction and the writes (CPU).
    extract: Duration,
    /// Write transactions.
    write: Duration,
}

/// Depth of the channels between stages, in batches.
const STAGE_QUEUE: usize = 1;

/// Depth of the prepare → extract channel, in batches: the extraction stage
/// consumes nothing while its context hydrates, and the prepare stage runs
/// ahead meanwhile.
const EXTRACT_QUEUE: usize = 4;

/// Ingest **as a stream**, for any source that emits candidates.
///
/// The stages are independent loops joined by bounded channels: each consumes
/// as soon as it has something, and the run costs the **max of the sums**.
///
/// ```text
/// produce ─→ [candidates] ─→ prepare ─→ [prepared] ─→ extract ─→ [bulk] ─→ write
/// ```
///
/// `produce` is the only point a source adapts.
///
/// **Writes stay sequential and in order.** One stage writes, on one
/// connection: the upsert resolves identity (`source_uid`, then the decision's
/// address) with a non-atomic read-then-write.
///
/// `prepare` thus reads the state from before the batches in flight commit: a
/// `source_uid` present twice in that window escapes the triage, and the upsert
/// catches it, returning `Skipped`/`Updated`.
pub(super) async fn run_stream<P, Fut>(
    read_conn: &Connection,
    write_conn: &Connection,
    mode: IngestMode,
    produce: P,
) -> Result<IngestCounts>
where
    P: FnOnce(mpsc::Sender<Candidate>) -> Fut,
    Fut: std::future::Future<Output = Result<IngestCounts>>,
{
    let (cand_tx, cand_rx) = mpsc::channel::<Candidate>(BATCH_SIZE * (STAGE_QUEUE + 1));
    let (prep_tx, prep_rx) = mpsc::channel::<Vec<PreparedDecision>>(EXTRACT_QUEUE);
    let (bulk_tx, bulk_rx) = mpsc::channel::<Vec<BulkDecisionWrite>>(STAGE_QUEUE);

    let started = Instant::now();
    let mut phases = Phases::default();

    let (source_counts, prepare_out, extract_out, write_out) = tokio::try_join!(
        async {
            let counts = produce(cand_tx).await?;
            Result::<_>::Ok(counts)
        },
        prepare_loop(read_conn, cand_rx, prep_tx, mode),
        extract_loop(write_conn, prep_rx, bulk_tx),
        write_loop(write_conn, bulk_rx),
    )?;

    let mut counts = source_counts;
    counts.merge(&prepare_out.0);
    counts.merge(&write_out.0);
    phases.prepare = prepare_out.1;
    phases.extract = extract_out;
    phases.write = write_out.1;

    tracing::info!(
        wall_s = started.elapsed().as_secs_f64().round() as u64,
        prepare_s = phases.prepare.as_secs_f64().round() as u64,
        extract_s = phases.extract.as_secs_f64().round() as u64,
        write_s = phases.write.as_secs_f64().round() as u64,
        "pipeline_phases"
    );
    Ok(counts)
}

/// Stage 1: gathers the candidates into batches and prepares them.
async fn prepare_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Candidate>,
    tx: mpsc::Sender<Vec<PreparedDecision>>,
    mode: IngestMode,
) -> Result<(IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let mut elapsed = Duration::ZERO;
    let mut buffer: Vec<Candidate> = Vec::with_capacity(BATCH_SIZE);
    let mut closed = false;
    loop {
        // `recv_many` returns as soon as **one** candidate is there: this loop
        // fills the batch before `fetch_states`.
        while !closed && buffer.len() < BATCH_SIZE {
            let room = BATCH_SIZE - buffer.len();
            if rx.recv_many(&mut buffer, room).await == 0 {
                closed = true;
            }
        }
        if buffer.is_empty() {
            break;
        }
        let batch = std::mem::take(&mut buffer);
        let (writes, batch_counts, took) = stage_prepare(conn, batch, mode).await?;
        counts.merge(&batch_counts);
        elapsed += took;
        // Récepteur tombé = un étage aval a échoué ; `try_join!` porte l'erreur.
        if !writes.is_empty() && tx.send(writes).await.is_err() {
            break;
        }
    }
    Ok((counts, elapsed))
}

/// Stage 2: structured extraction and the writes.
///
/// Hydrates the extraction context **before** consuming. The connection is the
/// writer's, idle while nothing is extracted; the hydration (read only, once
/// per process) is over before the first transaction.
async fn extract_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Vec<PreparedDecision>>,
    tx: mpsc::Sender<Vec<BulkDecisionWrite>>,
) -> Result<Duration> {
    let ctx = super::extract_ctx(conn).await?;
    let mut elapsed = Duration::ZERO;
    while let Some(items) = rx.recv().await {
        let (bulk, took) = stage_extract(ctx, items).await?;
        elapsed += took;
        if !bulk.is_empty() && tx.send(bulk).await.is_err() {
            break;
        }
    }
    Ok(elapsed)
}

/// Stage 3: one transaction per batch, sequential and in order.
async fn write_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Vec<BulkDecisionWrite>>,
) -> Result<(IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let mut elapsed = Duration::ZERO;
    while let Some(bulk) = rx.recv().await {
        let started = Instant::now();
        let written = write_writes(conn, bulk).await?;
        elapsed += started.elapsed();
        counts.merge(&written);
    }
    Ok((counts, elapsed))
}
