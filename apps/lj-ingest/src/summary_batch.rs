//! Summaries through the Message Batches API. A batch in flight is a row of
//! `summary_batch` until its results are written, so a killed run loses nothing.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Result};
use lj_llm::anthropic::{AnthropicBatches, Batch, BatchRequest, Outcome, Usage};
use lj_store::repository::{DecisionRepository, MissingSummaryRow, SummaryBatchRow};
use lj_summary::{
    assemble_rewrite, body_text_from_full_text, wants_second_pass, SummaryEngine,
    COLLAGE_PROSE_MIN, JURISDICTION_TIERS, SUMMARY_PROMPT_VERSION,
};

use crate::config::Settings;

const ENGINE: SummaryEngine = SummaryEngine::Haiku55BlockBatchV11;

/// The API caps a batch's requests and bytes, which these bounds stay under.
const BATCH_REQUESTS_MAX: usize = 10_000;
const BATCH_BYTES_MAX: usize = 200_000_000;

/// Batch prices of `claude-haiku-5-5` per token: input, one-hour cache write,
/// cache read, output.
const USD_PER_TOKEN: [f64; 4] = [0.05e-6, 0.10e-6, 0.005e-6, 0.25e-6];

const POLL_S: u64 = 60;

fn usd(u: &Usage) -> f64 {
    u.input_tokens as f64 * USD_PER_TOKEN[0]
        + u.cache_creation_input_tokens as f64 * USD_PER_TOKEN[1]
        + u.cache_read_input_tokens as f64 * USD_PER_TOKEN[2]
        + u.output_tokens as f64 * USD_PER_TOKEN[3]
}

pub async fn run(limit: Option<usize>, reverse: bool, wait: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let Some(key) = repo.pool_api_keys("anthropic").await?.into_iter().next() else {
        bail!("no anthropic key in llm_credential: lj-ingest credential import anthropic <VAR>");
    };
    let api = AnthropicBatches::new(key)?;

    collect(&repo, &api).await?;
    if let Some(limit) = limit.filter(|l| *l > 0) {
        submit_stale(&repo, &api, limit, reverse).await?;
    }
    while wait && !repo.open_summary_batches().await?.is_empty() {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(POLL_S)) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::warn!("summary_batch_wait_interrupted");
                return Ok(());
            }
        }
        collect(&repo, &api).await?;
    }
    Ok(())
}

async fn collect(repo: &DecisionRepository<'_>, api: &AnthropicBatches) -> Result<()> {
    for open in repo.open_summary_batches().await? {
        let batch = api.retrieve(&open.id).await?;
        if !batch.ended() {
            tracing::info!(
                batch = %open.id,
                processing = batch.request_counts.processing,
                succeeded = batch.request_counts.succeeded,
                "summary_batch_pending"
            );
            continue;
        }
        let redo = write_results(repo, api, &open, &batch).await?;
        // Deleted before the second pass goes out: a run killed in between leaves
        // those decisions stale for a later first pass, never in two batches.
        repo.delete_summary_batch(&open.id).await?;
        if !redo.is_empty() {
            let second = ENGINE
                .second_pass()
                .and_then(|t| t.wire())
                .expect("the batch engine names its second pass");
            let rows = repo.summary_rows(&redo).await?;
            submit(repo, api, &rows, second).await?;
        }
    }
    Ok(())
}

/// Returns the decisions of a first pass that want a second.
async fn write_results(
    repo: &DecisionRepository<'_>,
    api: &AnthropicBatches,
    open: &SummaryBatchRow,
    batch: &Batch,
) -> Result<Vec<i64>> {
    let engine = SummaryEngine::from_code(open.engine)
        .ok_or_else(|| anyhow!("summary_batch {} names engine {}", open.id, open.engine))?;
    let first_pass = Some(open.effort.as_str()) == engine.thinking().wire();
    let (mut written, mut taken, mut failed, mut spent) = (0usize, 0usize, 0usize, 0.0f64);
    let mut redo = Vec::new();
    for result in api.results(batch).await? {
        let decision_id: i64 = result.custom_id.parse()?;
        let answer = match result.outcome {
            Outcome::Succeeded(answer) => answer,
            other => {
                failed += 1;
                tracing::warn!(decision_id, outcome = ?other, "summary_batch_failed");
                continue;
            }
        };
        spent += usd(&answer.usage);
        let text = answer.text.trim();
        let thinking = answer.usage.output_tokens_details.thinking_tokens;
        let answered = assemble_rewrite(text).filter(|_| answer.stop_reason == "end_turn");
        if first_pass
            && answered
                .as_ref()
                .is_none_or(|w| wants_second_pass(&w.summary, thinking))
        {
            redo.push(decision_id);
            continue;
        }
        let Some(stored) = answered else {
            failed += 1;
            tracing::warn!(decision_id, stop = %answer.stop_reason, "summary_batch_empty");
            continue;
        };
        if repo
            .set_stale_summary(
                decision_id,
                &stored.summary,
                Some(stored.body),
                engine.prompt_version(),
                engine.code(),
            )
            .await?
        {
            written += 1;
        } else {
            taken += 1;
        }
    }
    tracing::info!(
        batch = %open.id,
        effort = %open.effort,
        written,
        second_pass = redo.len(),
        written_elsewhere = taken,
        failed,
        usd = spent,
        "summary_batch_collected"
    );
    Ok(redo)
}

async fn submit_stale(
    repo: &DecisionRepository<'_>,
    api: &AnthropicBatches,
    limit: usize,
    reverse: bool,
) -> Result<()> {
    let in_flight: HashSet<i64> = repo
        .open_summary_batches()
        .await?
        .into_iter()
        .flat_map(|b| b.decision_ids)
        .collect();
    let (types, tiers): (Vec<&str>, Vec<i32>) = JURISDICTION_TIERS.iter().copied().unzip();
    let wanted = (limit + in_flight.len()) as i64;
    let candidates: Vec<MissingSummaryRow> = repo
        .iter_summaries_by_jurisdiction(
            SUMMARY_PROMPT_VERSION,
            wanted,
            Some(wanted),
            true,
            &types,
            &tiers,
            reverse,
            &[],
            false,
        )
        .await?
        .into_iter()
        .flatten()
        .filter(|row| !in_flight.contains(&row.decision_id))
        .collect();
    let mut submitted = 0usize;
    for group in candidates.chunks(BATCH_REQUESTS_MAX) {
        let ids: Vec<i64> = group.iter().map(|r| r.decision_id).collect();
        let commented: HashSet<i64> = repo
            .fetch_collage_rows(&ids, COLLAGE_PROSE_MIN as i32)
            .await?
            .into_iter()
            .map(|r| r.decision_id)
            .collect();
        let rows: Vec<MissingSummaryRow> = group
            .iter()
            .filter(|r| !commented.contains(&r.decision_id))
            .take(limit - submitted)
            .cloned()
            .collect();
        let first = ENGINE
            .thinking()
            .wire()
            .expect("the batch engine names its level");
        submitted += submit(repo, api, &rows, first).await?;
        if submitted >= limit {
            break;
        }
    }
    tracing::info!(
        submitted,
        in_flight = in_flight.len(),
        "summary_batch_submitted"
    );
    Ok(())
}

/// Returns how many requests went out.
async fn submit(
    repo: &DecisionRepository<'_>,
    api: &AnthropicBatches,
    rows: &[MissingSummaryRow],
    effort: &str,
) -> Result<usize> {
    let ids: Vec<i64> = rows.iter().map(|r| r.decision_id).collect();
    let mut texts: HashMap<i64, String> = repo.fetch_full_texts(&ids).await?.into_iter().collect();
    let mut headers = crate::summary::headers(repo, rows).await?;
    let mut requests = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(body) = texts
            .remove(&row.decision_id)
            .and_then(|t| body_text_from_full_text(&t))
        else {
            tracing::warn!(decision_id = row.decision_id, "summary_no_body");
            continue;
        };
        let header = headers
            .remove(&row.decision_id)
            .expect("one header per row");
        requests.push(BatchRequest {
            custom_id: row.decision_id.to_string(),
            model: ENGINE.model(),
            effort,
            system: ENGINE.prompt(),
            user: header.rewrite_input(&body),
        });
    }
    let mut sent = 0usize;
    let mut rest = requests.as_slice();
    while !rest.is_empty() {
        let mut bytes = 0usize;
        let take = rest
            .iter()
            .take(BATCH_REQUESTS_MAX)
            .take_while(|r| {
                bytes += r.wire_len();
                bytes <= BATCH_BYTES_MAX
            })
            .count()
            .max(1);
        let (chunk, tail) = rest.split_at(take);
        let batch = api.create(chunk).await?;
        let ids: Vec<i64> = chunk
            .iter()
            .map(|r| r.custom_id.parse())
            .collect::<std::result::Result<_, _>>()?;
        repo.insert_summary_batch(&batch.id, ENGINE.code(), effort, &ids)
            .await?;
        tracing::info!(batch = %batch.id, effort, requests = chunk.len(), "summary_batch_created");
        sent += chunk.len();
        rest = tail;
    }
    Ok(sent)
}
