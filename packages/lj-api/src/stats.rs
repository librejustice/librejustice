//! Compteurs globaux du corpus pour la page d'accueil (`GET /api/corpus-stats`).
//!
//! Cache moka mono-entrée (TTL 12 h, `AppState::corpus_stats_cache`) sur le même
//! patron que le référentiel : le corpus ne bouge qu'à l'ingest quotidien, donc
//! la DB n'est relue qu'à l'expiration — jamais par requête. Les comptes sont
//! donc exacts (un seq scan 2×/jour est négligeable) : décisions **actives**
//! (non soft-deleted), et codes/articles depuis le catalogue navigable (`/codes`),
//! source unique de « ce qui compte comme un code ».

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
    // Réutilise la définition du catalogue (`list_legal_texts`, ADR 0133) plutôt
    // que de dupliquer son filtre « navigable » : codes ET articles suivent le
    // contenu de `/codes`. Payload minuscule (quelques dizaines de lignes) : le
    // compte de codes et la somme des articles en vigueur sortent du même appel.
    let catalog = repo.list_legal_texts().await.map_err(ApiError::Store)?;
    let codes_count = catalog.len() as i64;
    let articles_count = catalog.iter().map(|t| t.article_count).sum();
    Ok(Arc::new(CorpusStatsResponse {
        decisions_count,
        codes_count,
        articles_count,
    }))
}
