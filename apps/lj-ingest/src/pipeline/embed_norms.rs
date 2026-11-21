//! Cuts and embeds the article versions the norms search reads whose passages are missing or were cut from another version of their text.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::stream::{self, StreamExt};
use rayon::prelude::*;

use lj_chunk::{norm_passages, reaches_context, Chunk, EMBED_CONTEXT_TOKENS};
use lj_llm::backend::{cut_dims, AnyEmbedder, Embedder, NORM_EMBEDDING_DIM};
use lj_llm::openai_http::NORM_PASSAGE_PRIORITY;
use lj_store::repository::{DecisionRepository, NormPassageInput, NormPassageWrite};

use super::embed::{build_embedder, build_local_strict, get_tokenizer};
use crate::config::Settings;

/// Article versions a request carries: about 70,000 tokens.
const BATCH: usize = 128;
/// Batches in flight per host.
const IN_FLIGHT: usize = 2;

/// Embeds the queue of [`DecisionRepository::norm_embed_queue`], one transaction
/// per batch, `limit` bounding the versions visited. `stock` embeds on the local
/// hosts alone, never on Cloudflare. Past `stop_after`, no batch is started and
/// the run ends with success.
pub async fn embed_norms(
    limit: Option<usize>,
    stock: bool,
    stop_after: Option<Duration>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let embedder = if stock {
        build_local_strict(&settings).await?
    } else {
        build_embedder(&settings).await?
    }
    .with_passage_priority(NORM_PASSAGE_PRIORITY);
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

    let mut queue = repo.norm_embed_queue().await?;
    queue.truncate(limit.unwrap_or(usize::MAX));
    tracing::info!(queued = queue.len(), "embed-norms: start");

    let stop_at = stop_after.map(|after| Instant::now() + after);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (conn, queue, embedder, reader) = (&conn, &queue, &embedder, &reader);
    let produce = async move {
        let mut batches = stream::iter(queue.chunks(BATCH))
            .take_while(|_| std::future::ready(stop_at.is_none_or(|at| Instant::now() < at)))
            .map(|ids| embed_batch(reader, embedder, ids))
            .buffered(IN_FLIGHT * embedder.width());
        while let Some(batch) = batches.next().await {
            if tx.send(batch).await.is_err() {
                break;
            }
        }
    };
    let write = async move {
        let (mut visited, mut passages, mut blank) = (0usize, 0u64, 0usize);
        while let Some(batch) = rx.recv().await {
            let batch = batch?;
            visited += batch.visited;
            blank += batch.blank;
            if batch.writes.is_empty() {
                continue;
            }
            conn.batch_execute("BEGIN").await?;
            match repo.replace_norm_passages(&batch.writes).await {
                Ok(n) => {
                    conn.batch_execute("COMMIT").await?;
                    passages += n;
                }
                Err(e) => {
                    let _ = conn.batch_execute("ROLLBACK").await;
                    return Err(anyhow!("embed-norms write: {e}"));
                }
            }
            tracing::info!(visited, passages, "embed-norms progress");
        }
        tracing::warn!(
            visited,
            passages,
            blank,
            left = queue.len() - visited,
            "embed-norms"
        );
        Ok(())
    };
    let ((), written) = tokio::join!(produce, write);
    written
}

/// One version's passages, cut.
struct Cut {
    input: NormPassageInput,
    chunks: Vec<Chunk>,
}

/// One batch of the queue: the versions written, and those whose text cuts into no passage.
struct Batch {
    visited: usize,
    writes: Vec<NormPassageWrite>,
    blank: usize,
}

async fn embed_batch(
    repo: &DecisionRepository<'_>,
    embedder: &AnyEmbedder,
    ids: &[i64],
) -> Result<Batch> {
    let inputs = repo.norm_embed_inputs(ids).await?;
    let cuts = tokio::task::spawn_blocking(move || cut(inputs))
        .await
        .map_err(|e| anyhow!("embed-norms cut: {e}"))??;
    let found = cuts.len();
    let cuts: Vec<Cut> = cuts.into_iter().filter(|c| !c.chunks.is_empty()).collect();
    let blank = found - cuts.len();
    if cuts.is_empty() {
        return Ok(Batch {
            visited: ids.len(),
            writes: Vec::new(),
            blank,
        });
    }
    let texts: Vec<String> = cuts
        .iter()
        .flat_map(|c| c.chunks.iter().map(Chunk::embedding_text))
        .collect();
    let vectors = embedder
        .embed_passages(&texts)
        .await
        .map_err(|e| anyhow!("embed_passages: {e}"))?;
    let mut rows = vectors.outer_iter();
    let writes = cuts
        .into_iter()
        .map(|c| NormPassageWrite {
            article_id: c.input.article_id,
            content_checksum: c.input.content_checksum,
            embeddings: rows
                .by_ref()
                .take(c.chunks.len())
                .map(|v| cut_dims(&v.to_vec(), NORM_EMBEDDING_DIM))
                .collect(),
        })
        .collect();
    Ok(Batch {
        visited: ids.len(),
        writes,
        blank,
    })
}

/// Cuts each version in char mode, and again in exact BPE one whose passage the
/// char estimate leaves at the host's context or above.
fn cut(inputs: Vec<NormPassageInput>) -> Result<Vec<Cut>> {
    inputs
        .into_par_iter()
        .map(|input| {
            let passages = |tokenizer| {
                norm_passages(
                    &input.title,
                    &input.title_path,
                    &input.num,
                    &input.texte,
                    tokenizer,
                )
                .map_err(|e| anyhow!("norm_passages {}: {e}", input.article_id))
            };
            let mut chunks = passages(None)?;
            // A token holds a byte at least, and the tokenizer adds two at most.
            let long = chunks
                .iter()
                .any(|c| c.prefix.len() + c.body.len() + 2 >= EMBED_CONTEXT_TOKENS);
            if long && reaches_context(std::slice::from_ref(&chunks), get_tokenizer())? {
                chunks = passages(Some(get_tokenizer()))?;
            }
            Ok(Cut { input, chunks })
        })
        .collect()
}
