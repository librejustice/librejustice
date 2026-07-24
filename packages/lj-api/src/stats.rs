//! Compteurs globaux du corpus pour la page d'accueil (`GET /api/corpus-stats`).
//!
//! Cache moka mono-entrée (TTL 12 h, `AppState::corpus_stats_cache`) sur le même
//! patron que le référentiel : le corpus ne bouge qu'à l'ingest quotidien, donc
//! la DB n'est relue qu'à l'expiration — jamais par requête. Les comptes sont
//! donc exacts (un seq scan 2×/jour est négligeable) : décisions **actives**
//! (non soft-deleted), et le corpus normatif entier (tout `legal_text` + articles
//! en vigueur, toutes natures) — pas le seul catalogue navigable `/codes`.

use std::sync::Arc;

use lj_dtos::CorpusStatsResponse;
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// Compteurs corpus depuis le cache de l'état (TTL 12 h), calculés au premier accès.
pub async fn corpus_stats(state: &AppState) -> Result<Arc<CorpusStatsResponse>> {
    state
        .corpus_stats_cache
        .try_get_with((), load(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("corpus stats load: {e}")))
}

async fn load(state: &AppState) -> Result<Arc<CorpusStatsResponse>> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let decisions_count = repo
        .count_active_decisions()
        .await
        .map_err(ApiError::Store)?;
    // Corpus normatif entier (`legal_text` toutes natures + articles en vigueur),
    // pas le seul catalogue navigable `/codes` : le règne de la Norme porte
    // ~215 k textes et ~930 k articles, d'un aller-retour derrière le cache.
    let (texts_count, articles_count) = repo
        .count_normative_corpus()
        .await
        .map_err(ApiError::Store)?;
    Ok(Arc::new(CorpusStatsResponse {
        decisions_count,
        texts_count,
        articles_count,
    }))
}
