//! Orchestration d'un batch : write (I/O DB, tokio) + drain (triage → process).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_llm::backend::Embedder;
use lj_store::db::Connection;
use lj_store::repository::{
    BulkDecisionWrite, DecisionRepository, ExistingDecisionState, UpsertStatus,
};

use crate::chunking::DEFAULT_CHUNK_TOKENS;

use super::embed::embed_writes;
use super::prepare::{prepare_write, triage_candidates};
use super::{Candidate, IngestCounts, IngestMode, PreparedDecision, WriteMode};

/// Écrit un batch en DB (port de `_write_writes`). Une transaction par batch.
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
    for write in writes {
        if write.write_mode == WriteMode::SourceXmlOnly.as_str() {
            let decision_id = write
                .decision_id
                .ok_or_else(|| anyhow!("backfill source_xml sans decision_id"))?;
            // Backfill du `public_id` (+ format) à contenu inchangé. Depuis le DROP
            // des colonnes mono-source (ADR 0098 §2), `source_uid`/`source_fields`
            // vivent sur la provenance — qui existe déjà ici (le triage n'émet ce
            // mode que sur une provenance active trouvée). Rien à réécrire côté
            // `decision_sources` (et surtout pas son `source_fields` avec le `Null`
            // de ce mode) : on complète seulement l'identifiant manquant.
            repo.set_public_id(decision_id, &write.public_id).await?;
            repo.set_payload_format(decision_id, &write.payload_format)
                .await?;
            counts.updated += 1;
            continue;
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
                        write.embed_version,
                        &write.payload_format,
                    )
                    .await?;
                match result.status {
                    UpsertStatus::Created => counts.created += 1,
                    UpsertStatus::Updated => counts.updated += 1,
                    UpsertStatus::Skipped => {
                        counts.skipped += 1;
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
                    write.embed_version,
                    &write.payload_format,
                )
                .await?;
                counts.updated += 1;
                id
            }
        };
        counts.chunks_created += repo
            .replace_chunks(decision_id, &write.decision, &write.chunks)
            .await?;
        repo.set_payload_format(decision_id, &write.payload_format)
            .await?;
    }
    Ok(())
}

/// Traite un batch de candidats déjà triés (survivants) : prepare en parallèle
/// (rayon), embed, écrit. Met à jour `counts`. `own_txn = true` : ouvre sa propre
/// transaction par batch (chemin ingest normal) ; `false` : écrit **sous la
/// transaction de l'appelant** (re-split #29, qui ouvre un `BEGIN`/`COMMIT` par
/// cluster — `write_writes` nesterait un `BEGIN`/`COMMIT` qui commiterait la
/// transaction externe trop tôt).
async fn process_batch<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    survivors: Vec<Candidate>,
    chunk_tokens: usize,
    counts: &mut IngestCounts,
    own_txn: bool,
) -> Result<()> {
    // Contexte d'extraction du run (linker + vocab compilé, ADR 0145/0156) —
    // hydraté au premier batch, partagé ensuite.
    let ctx = super::extract_ctx(conn).await?;
    // Prepare CPU parallèle (sans GIL = le gain Rust vs ProcessPoolExecutor).
    let prepared: Vec<Result<Option<PreparedDecision>>> = survivors
        .into_par_iter()
        .map(|cand| prepare_write(cand, chunk_tokens, ctx))
        .collect();

    let mut writes = Vec::with_capacity(prepared.len());
    for item in prepared {
        match item? {
            Some(p) => writes.push(p),
            None => counts.empty_skipped += 1,
        }
    }
    if writes.is_empty() {
        return Ok(());
    }

    let bulk = embed_writes(embedder, writes, chunk_tokens).await?;
    if own_txn {
        let batch_counts = write_writes(conn, bulk).await?;
        counts.merge(&batch_counts);
    } else {
        let repo = DecisionRepository::new(conn);
        write_writes_inner(&repo, bulk, counts).await?;
    }
    Ok(())
}

/// Précheck DB groupé des `source_uid` d'un batch (port de `find_ingest_states`).
async fn fetch_states(
    conn: &Connection,
    uids: &[String],
) -> Result<HashMap<String, ExistingDecisionState>> {
    let repo = DecisionRepository::new(conn);
    Ok(repo.find_ingest_states(uids).await?)
}

/// Triage + traitement d'un batch de candidats (lecture states → triage →
/// process). Chemin ingest normal : une transaction par batch.
pub(super) async fn drain_batch<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    counts: &mut IngestCounts,
) -> Result<()> {
    drain_batch_impl(
        conn,
        embedder,
        candidates,
        require_embeddings,
        mode,
        counts,
        true,
    )
    .await
}

/// Variante de [`drain_batch`] écrivant **sous la transaction de l'appelant**
/// (pas de `BEGIN`/`COMMIT` propre) : le re-split #29 enveloppe chaque cluster
/// dans sa propre transaction et y enchaîne create_split + re-ingest.
pub(super) async fn drain_batch_in_txn<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    counts: &mut IngestCounts,
) -> Result<()> {
    drain_batch_impl(
        conn,
        embedder,
        candidates,
        require_embeddings,
        mode,
        counts,
        false,
    )
    .await
}

async fn drain_batch_impl<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    counts: &mut IngestCounts,
    own_txn: bool,
) -> Result<()> {
    let uids: Vec<String> = candidates
        .iter()
        .map(|c| c.decision.source_uid.clone())
        .collect();
    let existing = fetch_states(conn, &uids).await?;
    let (survivors, skipped, deduped) =
        triage_candidates(candidates, &existing, require_embeddings, mode);
    counts.skipped += skipped;
    counts.dedup_in_batch += deduped;
    if survivors.is_empty() {
        return Ok(());
    }
    process_batch(
        conn,
        embedder,
        survivors,
        DEFAULT_CHUNK_TOKENS,
        counts,
        own_txn,
    )
    .await
}
