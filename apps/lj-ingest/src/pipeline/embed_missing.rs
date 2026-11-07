//! Cuts and embeds, from their stored texts and the read text's summary, the decisions whose passages are missing, lack a vector, or predate `EMBED_VERSION` or their summary.

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_chunk::{
    chunk_aligned, embedding_prefix, reaches_context, Chunk, DEFAULT_CHUNK_TOKENS, EMBED_VERSION,
};
use lj_core::decision::Decision;
use lj_llm::backend::{AnyEmbedder, Embedder};
use lj_llm::error::EmbedError;
use lj_parse::FromSourceFields;
use lj_store::repository::{ChunkWrite, DecisionRepository, EmbedInput, SummaryStamp};

use super::embed::{build_embedder, build_local_strict, get_tokenizer};
use super::BATCH_SIZE;
use crate::config::Settings;

/// A decision's passages, cut from each of its texts behind the read text's
/// header and summary.
struct Cut {
    decision_id: i64,
    decision: Decision,
    /// The read text's language.
    lang: String,
    /// The cleaned texts of its other languages the passages read, each with its language.
    others: Vec<(String, String)>,
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
pub async fn embed_missing(limit: Option<usize>, stock: bool, shard: (i64, i64)) -> Result<()> {
    let settings = Settings::from_env()?;
    let embedder = if stock {
        build_local_strict(&settings).await?
    } else {
        build_embedder(&settings).await?
    };
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut queue = repo.embed_queue(EMBED_VERSION, stock, shard).await?;
    queue.truncate(limit.unwrap_or(usize::MAX));
    tracing::info!(queued = queue.len(), "embed-missing: start");
    let mut visited = 0usize;
    let mut unbuilt = 0usize;
    let mut empty = 0usize;
    let mut embedded = 0usize;
    let mut chunks = 0usize;
    for ids in queue.chunks(BATCH_SIZE) {
        let mut inputs = Vec::with_capacity(ids.len());
        for (id, input) in repo.embed_inputs(ids).await? {
            visited += 1;
            match input {
                Some(input) => inputs.push((id, input)),
                None => {
                    tracing::warn!(decision_id = id, "embed-missing: no provenance or text");
                    unbuilt += 1;
                }
            }
        }
        let built = inputs.len();
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
        empty += built - cuts.len();
        let vectors = embed_cuts(&embedder, &mut cuts).await?;

        conn.batch_execute("BEGIN").await?;
        let done = async {
            let mut n = 0usize;
            for (cut, vectors) in cuts.iter().zip(vectors) {
                n += repo
                    .replace_chunks(
                        cut.decision_id,
                        &cut.decision,
                        &chunk_writes(cut, vectors),
                        cut.summary.unwrap_or((None, None)),
                    )
                    .await?;
            }
            repo.set_embed_version(ids, EMBED_VERSION).await?;
            Ok::<_, lj_store::error::StoreError>(n)
        }
        .await;
        match done {
            Ok(n) => {
                conn.batch_execute("COMMIT").await?;
                embedded += cuts.len();
                chunks += n;
            }
            Err(e) => {
                let _ = conn.batch_execute("ROLLBACK").await;
                return Err(anyhow!("embed-missing write: {e}"));
            }
        }
        tracing::info!(visited, embedded, chunks, "embed-missing progress");
    }
    tracing::warn!(visited, embedded, chunks, unbuilt, empty, "embed-missing");
    Ok(())
}

/// Cuts a decision's texts at the same points behind the read text's header
/// and summary, in char mode: `None` for a read text empty once cleaned.
fn cut(decision_id: i64, input: EmbedInput) -> Result<Option<Cut>> {
    let decision =
        Decision::from_source_fields(&input.full_text, &input.source_fields, &input.source_uid);
    if decision.texte_integral_clean.is_empty() {
        return Ok(None);
    }
    let prefix = embedding_prefix(
        &decision.metadata_header,
        input.summary.as_deref().map(lj_summary::summary_head),
    );
    let others: Vec<(String, String)> = input
        .others
        .into_iter()
        .map(|o| {
            let text = Decision::from_source_fields(&o.full_text, &o.source_fields, &o.source_uid)
                .texte_integral_clean;
            (text, o.lang)
        })
        .filter(|(text, _)| !text.is_empty())
        .collect();
    let chunks = chunk_aligned(
        &texts(&decision, &input.lang, &others),
        &prefix,
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
        others,
        chunks,
        at_context,
        summary: input.summary.map(|_| input.stamp),
    }))
}

fn texts<'a>(
    decision: &'a Decision,
    lang: &'a str,
    others: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    std::iter::once((decision.texte_integral_clean.as_str(), lang))
        .chain(
            others
                .iter()
                .map(|(text, lang)| (text.as_str(), lang.as_str())),
        )
        .collect()
}

/// The vector of each passage of each of `cuts`, the mean of its texts'. When
/// the char estimate falls short and a passage reaches or overflows the
/// embedder's context, the batch is cut again in exact BPE and sent once more.
async fn embed_cuts(embedder: &AnyEmbedder, cuts: &mut [Cut]) -> Result<Vec<Vec<Vec<f32>>>> {
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
            let tokenizer = get_tokenizer();
            for c in cuts.iter_mut() {
                let prefix = c.chunks[0][0].prefix.clone();
                c.chunks = chunk_aligned(
                    &texts(&c.decision, &c.lang, &c.others),
                    &prefix,
                    DEFAULT_CHUNK_TOKENS,
                    Some(tokenizer),
                    None,
                )
                .map_err(|e| anyhow!("chunk_aligned {}: {e}", c.decision.source_uid))?;
            }
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
