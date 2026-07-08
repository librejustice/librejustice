//! Re-embed ciblé des décisions orphelines : chunks présents mais `embedding`
//! NULL (#39, opendata principalement). Op de maintenance idempotente.
//!
//! Reconstruit chaque décision depuis `(full_text, source_fields)` (ADR 0085) en
//! un [`Candidate`] fidèle (même `source_uid`, même `content_checksum`, même
//! `public_id`) puis le passe au pipeline normal via [`drain_batch`] en
//! `require_embeddings = true` : le triage voit `same_hash` + `!has_embeddings`
//! → branche re-embed (ré-chunk identique par parité #37 + embed + `replace_chunks`).
//! Aucune création ni changement d'identité — uniquement le remplissage des
//! embeddings manquants.
//!
//! Embedder **vLLM strict** ([`build_vllm_strict`]) : jamais de repli Cloudflare
//! (coût) — erreur franche si vLLM est injoignable.

use anyhow::{anyhow, Result};

use lj_core::decision::Decision;
use lj_store::repository::DecisionRepository;

use super::batch::drain_batch;
use super::embed::build_vllm_strict;
use super::{Candidate, IngestCounts, IngestMode, WriteMode, BATCH_SIZE};
use crate::config::Settings;

/// Re-embed les décisions ayant au moins un chunk sans embedding. `limit` borne
/// le nombre de décisions traitées (None = toutes). Keyset par `decision_id`,
/// reprise transparente.
pub async fn embed_missing(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    // Embedder vLLM strict : refuse Cloudflare (coût) et erreur si vLLM down.
    let embedder = build_vllm_strict(&settings).await?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Op de maintenance : lève la borne API (build_pool pose statement_timeout=30s)
    // — le scan de frontière du keyset au resume peut être long.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut last_id: i64 = 0;
    let mut counts = IngestCounts::default();
    let mut scanned = 0usize;
    let batch_limit = BATCH_SIZE as i64;

    loop {
        if let Some(max) = limit {
            if scanned >= max {
                break;
            }
        }
        let rows = repo
            .fetch_missing_embedding_batch(last_id, batch_limit)
            .await?;
        let Some(&(max_batch_id, _)) = rows.last() else {
            break;
        };
        last_id = max_batch_id;

        let mut candidates: Vec<Candidate> = Vec::with_capacity(rows.len());
        for (id, complete) in rows {
            let Some((public_id, payload_format, full_text, source_fields, source_uid, checksum)) =
                complete
            else {
                tracing::warn!(decision_id = id, "orphelin sans provenance/full_text, skip");
                continue;
            };
            // Reconstruction canonique (ADR 0085) : decision portée par
            // (full_text, source_fields). `source_fields` est passé en
            // `prebuilt_source_fields` car `raw_payload` n'existe plus.
            let decision = Decision::from_source_fields(&full_text, &source_fields, &source_uid);
            candidates.push(Candidate {
                decision_id: Some(id),
                public_id,
                decision,
                content_checksum: checksum,
                raw_payload: Vec::new(),
                payload_format,
                write_mode: WriteMode::Full,
                dila_fond: None,
                prebuilt_source_fields: Some(source_fields),
                prebuilt_extracted: None,
            });
        }

        scanned += candidates.len();
        if candidates.is_empty() {
            continue;
        }

        // require_embeddings = true : la branche re-embed du triage se déclenche
        // sur les décisions à hash identique dont les embeddings manquent.
        drain_batch(
            &conn,
            Some(&embedder),
            candidates,
            true,
            IngestMode::MissingHash,
            &mut counts,
        )
        .await?;

        tracing::info!(
            last_id,
            updated = counts.updated,
            chunks = counts.chunks_created,
            skipped = counts.skipped,
            "embed-missing progress"
        );
    }

    tracing::info!(
        updated = counts.updated,
        chunks = counts.chunks_created,
        skipped = counts.skipped,
        empty_skipped = counts.empty_skipped,
        "Re-embed des orphelins terminé"
    );
    Ok(())
}
