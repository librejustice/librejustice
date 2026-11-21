//! Cuts and embeds, from their stored texts, each behind its own summary, the decisions whose passages are missing, lack a vector, or predate `EMBED_VERSION` or their summary.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::stream::{self, StreamExt};
use rayon::prelude::*;

use lj_chunk::{
    chunk_aligned, embedding_prefix, reaches_context, Chunk, DEFAULT_CHUNK_TOKENS, EMBED_VERSION,
};
use lj_core::decision::Decision;
use lj_llm::backend::{AnyEmbedder, Embedder};
use lj_llm::error::EmbedError;
use lj_parse::FromSourceFields;
use lj_store::repository::{ChunkWrite, DecisionRepository, EmbedInput, EmbedScope, SummaryStamp};

use super::embed::{build_embedder, build_local_strict, get_tokenizer};
use super::BATCH_SIZE;
use crate::config::Settings;

/// Batches cut and embedded at once per host, beside the write of the one before.
/// On one GPU, two requests in flight embed 10 % more than one, four no more.
const IN_FLIGHT: usize = 2;

/// A decision's passages, cut from each of its texts behind the read text's
/// header and the text's own summary.
struct Cut {
    decision_id: i64,
    decision: Decision,
    /// The read text's language and prefix.
    lang: String,
    prefix: String,
    /// The cleaned texts of its other languages the passages read, each with its
    /// language and prefix.
    others: Vec<(String, String, String)>,
    /// The passages of each text, the read text's first, as many in each.
    chunks: Vec<Vec<Chunk>>,
    /// A passage counts the embedding host's context or more.
    at_context: bool,
    /// The summary the passages are embedded behind, `None` without one.
    summary: Option<SummaryStamp>,
}

/// Replaces the passages of each decision of [`DecisionRepository::embed_queue`]
/// and sets its `embed_version`, one transaction per batch, `limit` bounding the
/// visits. The stock is embedded on the local hosts alone, never on Cloudflare.
/// Past `stop_after` from its start, no batch is started and the run ends with success.
pub async fn embed_missing(
    limit: Option<usize>,
    stock: bool,
    shard: (i64, i64),
    scope: EmbedScope,
    stop_after: Option<Duration>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let embedder = if stock {
        build_local_strict(&settings).await?
    } else {
        build_embedder(&settings).await?
    };
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let reader = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    for c in [&conn, &reader] {
        c.batch_execute("SET statement_timeout = 0")
            .await
            .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    }
    let repo = DecisionRepository::new(&conn);
    let reader = DecisionRepository::new(&reader);

    // From the start, queue included: the watchdog counts it too.
    let stop_at = stop_after.map(|after| Instant::now() + after);
    let mut queue = repo.embed_queue(EMBED_VERSION, stock, shard, scope).await?;
    queue.truncate(limit.unwrap_or(usize::MAX));
    tracing::info!(queued = queue.len(), "embed-missing: start");

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (conn, queue, embedder, reader) = (&conn, &queue, &embedder, &reader);
    let produce = async move {
        let mut batches = stream::iter(queue.chunks(BATCH_SIZE))
            .take_while(|_| std::future::ready(stop_at.is_none_or(|at| Instant::now() < at)))
            .map(|ids| embed_batch(reader, embedder, ids, scope))
            .buffered(IN_FLIGHT * embedder.width());
        while let Some(batch) = batches.next().await {
            if tx.send(batch).await.is_err() {
                break;
            }
        }
    };
    let write = async move {
        let mut visited = 0usize;
        let mut waiting = 0usize;
        let mut unbuilt = 0usize;
        let mut empty = 0usize;
        let mut embedded = 0usize;
        let mut chunks = 0usize;
        while let Some(batch) = rx.recv().await {
            let batch = batch?;
            let written = Instant::now();
            visited += batch.ids.len();
            waiting += batch.waiting;
            unbuilt += batch.unbuilt;
            empty += batch.empty;
            conn.batch_execute("BEGIN").await?;
            let done = async {
                let mut n = 0usize;
                for (cut, vectors) in batch.cuts.iter().zip(batch.vectors) {
                    n += repo
                        .replace_chunks(
                            cut.decision_id,
                            &cut.decision,
                            &chunk_writes(cut, vectors),
                            cut.summary.unwrap_or((None, None)),
                        )
                        .await?;
                }
                repo.set_embed_version(&batch.ids, EMBED_VERSION).await?;
                Ok::<_, lj_store::error::StoreError>(n)
            }
            .await;
            match done {
                Ok(n) => {
                    conn.batch_execute("COMMIT").await?;
                    embedded += batch.cuts.len();
                    chunks += n;
                }
                Err(e) => {
                    let _ = conn.batch_execute("ROLLBACK").await;
                    return Err(anyhow!("embed-missing write: {e}"));
                }
            }
            tracing::info!(
                visited,
                embedded,
                chunks,
                read_ms = batch.read.as_millis() as u64,
                cut_ms = batch.cut.as_millis() as u64,
                embed_ms = batch.embed.as_millis() as u64,
                write_ms = written.elapsed().as_millis() as u64,
                "embed-missing progress"
            );
        }
        tracing::warn!(
            visited,
            embedded,
            chunks,
            waiting,
            unbuilt,
            empty,
            left = queue.len() - visited - waiting,
            "embed-missing"
        );
        Ok(())
    };
    let ((), written) = tokio::join!(produce, write);
    written
}

/// A batch cut and embedded, waiting for its write.
struct Embedded {
    /// The decisions of the batch the scope takes.
    ids: Vec<i64>,
    /// Those it leaves, a summary missing.
    waiting: usize,
    cuts: Vec<Cut>,
    vectors: Vec<Vec<Vec<f32>>>,
    /// Decisions without provenance or text.
    unbuilt: usize,
    /// Decisions whose read text is empty once cleaned.
    empty: usize,
    /// Time spent reading the inputs, cutting them, and embedding the passages.
    read: Duration,
    cut: Duration,
    embed: Duration,
}

async fn embed_batch(
    repo: &DecisionRepository<'_>,
    embedder: &AnyEmbedder,
    queued: &[i64],
    scope: EmbedScope,
) -> Result<Embedded> {
    let started = Instant::now();
    let ids = repo.embed_scope(queued, scope).await?;
    let waiting = queued.len() - ids.len();
    let mut inputs = Vec::with_capacity(ids.len());
    let mut unbuilt = 0usize;
    for (id, input) in repo.embed_inputs(&ids).await? {
        match input {
            Some(input) => inputs.push((id, input)),
            None => {
                tracing::warn!(decision_id = id, "embed-missing: no provenance or text");
                unbuilt += 1;
            }
        }
    }
    let built = inputs.len();
    let read = started.elapsed();
    let mut cuts: Vec<Cut> = tokio::task::spawn_blocking(move || {
        inputs
            .into_par_iter()
            .map(|(id, input)| cut(id, input))
            .collect::<Result<Vec<_>>>()
    })
    .await
    .map_err(|e| anyhow!("embed-missing cut: {e}"))??
    .into_iter()
    .flatten()
    .collect();
    let empty = built - cuts.len();
    let cut = started.elapsed() - read;
    let vectors = if cuts.is_empty() {
        Vec::new()
    } else {
        embed_cuts(embedder, &mut cuts).await?
    };
    Ok(Embedded {
        ids,
        waiting,
        cuts,
        vectors,
        unbuilt,
        empty,
        read,
        cut,
        embed: started.elapsed() - read - cut,
    })
}

/// Cuts a decision's texts at the same points, each behind the read text's
/// header and its own summary, in char mode: `None` for a read text empty once
/// cleaned.
fn cut(decision_id: i64, input: EmbedInput) -> Result<Option<Cut>> {
    let decision =
        Decision::from_source_fields(&input.full_text, &input.source_fields, &input.source_uid);
    if decision.texte_integral_clean.is_empty() {
        return Ok(None);
    }
    let prefix = |summary: Option<&str>| {
        embedding_prefix(
            &decision.metadata_header,
            summary.map(lj_summary::summary_head),
        )
    };
    let read_prefix = prefix(input.summary.as_deref());
    let others: Vec<(String, String, String)> = input
        .others
        .into_iter()
        .map(|o| {
            let text = Decision::from_source_fields(&o.full_text, &o.source_fields, &o.source_uid)
                .texte_integral_clean;
            (text, o.lang, prefix(o.summary.as_deref()))
        })
        .filter(|(text, _, _)| !text.is_empty())
        .collect();
    let chunks = chunk_aligned(
        &texts(&decision, &input.lang, &read_prefix, &others),
        DEFAULT_CHUNK_TOKENS,
        None,
        None,
    )
    .map_err(|e| anyhow!("chunk_aligned {}: {e}", input.source_uid))?;
    // The host answers an overflow above its context, but a passage of exactly
    // that length can stall it instead of answering.
    let at_context = reaches_context(&chunks, get_tokenizer())?;
    Ok(Some(Cut {
        decision_id,
        decision,
        lang: input.lang,
        prefix: read_prefix,
        others,
        chunks,
        at_context,
        summary: input.summary.map(|_| input.stamp),
    }))
}

fn texts<'a>(
    decision: &'a Decision,
    lang: &'a str,
    prefix: &'a str,
    others: &'a [(String, String, String)],
) -> Vec<(&'a str, &'a str, &'a str)> {
    std::iter::once((decision.texte_integral_clean.as_str(), lang, prefix))
        .chain(
            others
                .iter()
                .map(|(text, lang, prefix)| (text.as_str(), lang.as_str(), prefix.as_str())),
        )
        .collect()
}

/// The vector of each passage of each of `cuts`, the mean of its texts'. When
/// the char estimate falls short and a passage reaches or overflows the
/// embedder's context, the batch is cut again in exact BPE and sent once more.
async fn embed_cuts(embedder: &AnyEmbedder, cuts: &mut Vec<Cut>) -> Result<Vec<Vec<Vec<f32>>>> {
    fn inputs(cuts: &[Cut]) -> Vec<String> {
        cuts.iter()
            .flat_map(|c| c.chunks.iter().flatten().map(Chunk::embedding_text))
            .collect()
    }
    let sent = if cuts.iter().any(|c| c.at_context) {
        Err(EmbedError::InputTooLong)
    } else {
        embedder.embed_passages(&inputs(cuts)).await
    };
    let vecs = match sent {
        Ok(v) => v,
        Err(EmbedError::InputTooLong) => {
            tracing::warn!(
                batch = cuts.len(),
                "embed-missing: context overflow in char mode, cut again in exact BPE"
            );
            let mut owned = std::mem::take(cuts);
            *cuts = tokio::task::spawn_blocking(move || {
                owned.par_iter_mut().try_for_each(|c| {
                    c.chunks = chunk_aligned(
                        &texts(&c.decision, &c.lang, &c.prefix, &c.others),
                        DEFAULT_CHUNK_TOKENS,
                        Some(get_tokenizer()),
                        None,
                    )
                    .map_err(|e| anyhow!("chunk_aligned {}: {e}", c.decision.source_uid))?;
                    Ok::<_, anyhow::Error>(())
                })?;
                Ok::<_, anyhow::Error>(owned)
            })
            .await
            .map_err(|e| anyhow!("embed-missing BPE cut: {e}"))??;
            embedder
                .embed_passages(&inputs(cuts))
                .await
                .map_err(|e| anyhow!("embed_passages after the BPE cut: {e}"))?
        }
        Err(e) => return Err(anyhow!("embed_passages: {e}")),
    };
    let mut offset = 0usize;
    Ok(cuts
        .iter()
        .map(|c| {
            let (langs, passages) = (c.chunks.len(), c.chunks[0].len());
            let rows = (0..passages)
                .map(|i| {
                    let mut mean = vecs.row(offset + i).to_owned();
                    for l in 1..langs {
                        mean += &vecs.row(offset + l * passages + i);
                    }
                    (mean / langs as f32).to_vec()
                })
                .collect();
            offset += langs * passages;
            rows
        })
        .collect())
}

/// The rows of a cut's passages, spanned on the read text and tagged with what
/// each holds.
fn chunk_writes(cut: &Cut, vectors: Vec<Vec<f32>>) -> Vec<ChunkWrite> {
    let annex_start = lj_core::annexes::start(&cut.decision.texte_integral_clean);
    cut.chunks[0]
        .iter()
        .zip(vectors)
        .map(|(chunk, embedding)| ChunkWrite {
            chunk_index: chunk.chunk_index as i32,
            char_start: chunk.char_start as i32,
            char_end: chunk.char_end as i32,
            embedding,
            zones: chunk.zones(cut.summary.is_some(), annex_start),
        })
        .collect()
}

/// Parses `k/n`, `0 <= k < n`.
pub fn parse_shard(s: &str) -> Result<(i64, i64), String> {
    let (k, n) = s.split_once('/').ok_or("expected k/n")?;
    let k: i64 = k.parse().map_err(|e| format!("k: {e}"))?;
    let n: i64 = n.parse().map_err(|e| format!("n: {e}"))?;
    if n < 1 || !(0..n).contains(&k) {
        return Err(format!("expected 0 <= k < n, got {k}/{n}"));
    }
    Ok((k, n))
}
