//! Whole-corpus counters for the home page (`GET /api/corpus-stats`): three
//! rows of `corpus_counts`, the snapshot `precompute` writes every night. The
//! legal side is the whole corpus, every `legal_text` of every nature, not the
//! browsable `/codes` catalogue alone. No process cache: the snapshot is the
//! cache, and an indexed read of three rows is cheap.

use lj_dtos::CorpusStatsResponse;
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::state::AppState;

pub async fn corpus_stats(state: &AppState) -> Result<CorpusStatsResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let (decisions_count, texts_count, articles_count) =
        repo.corpus_totals().await.map_err(ApiError::Store)?;
    Ok(CorpusStatsResponse {
        decisions_count,
        texts_count,
        articles_count,
    })
}
