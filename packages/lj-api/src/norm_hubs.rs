//! Hubs du catalogue des normes (ADR 0255) : catalogue `/api/normes`, hub
//! `/api/normes/{fond}`, page année `/api/normes/{fond}/{annee}` (`annee` =
//! année ou `sans-date`). Chemins de crawl SSR vers les textes — réponses
//! stables (le corpus bouge à l'ingest quotidien), cache CDN long côté routes.

use crate::error::{ApiError, Result};
use crate::state::AppState;
use lj_core::referential_labels::{norm_fond_label, NORM_FONDS};
use lj_dtos::{
    NormCatalogueResponse, NormFondEntry, NormFondResponse, NormHubText, NormYearCount,
    NormYearResponse,
};
use lj_store::repository::DecisionRepository;
use std::sync::Arc;

/// Textes par page hub fond×année.
pub const HUB_PAGE_SIZE: u32 = 100;

/// Token d'URL du bucket des textes sans date de parcours.
pub const UNDATED_TOKEN: &str = "sans-date";

/// Catalogue des fonds, dans l'ordre éditorial de `NORM_FONDS`. Une seule
/// entrée de cache (clé `()`), TTL 12 h — l'agrégat sur 1 M de lignes ne se
/// recalcule jamais par requête.
pub async fn catalogue(state: &AppState) -> Result<Arc<NormCatalogueResponse>> {
    state
        .norm_catalogue_cache
        .try_get_with((), load_catalogue(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue normes: {e}")))
}

async fn load_catalogue(state: &AppState) -> Result<Arc<NormCatalogueResponse>> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let counts = DecisionRepository::new(&conn).norm_catalogue().await?;

    let fonds = NORM_FONDS
        .iter()
        .filter_map(|fond| {
            let count = counts.iter().find(|(f, _)| f == fond).map(|(_, c)| *c)?;
            Some(NormFondEntry {
                fond: (*fond).to_string(),
                label: norm_fond_label(fond).unwrap_or(fond).to_string(),
                text_count: count,
            })
        })
        .collect();
    Ok(Arc::new(NormCatalogueResponse { fonds }))
}

/// Hub d'un fond : années couvertes + volume. 404 si le fond est inconnu de
/// la taxonomie, vide, ou `codes` (son catalogue est `/codes`).
pub async fn hub(state: &AppState, fond: &str) -> Result<NormFondResponse> {
    let label = browsable_fond_label(fond)?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let years = DecisionRepository::new(&conn).norm_fond_years(fond).await?;
    if years.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(NormFondResponse {
        fond: fond.to_string(),
        label,
        text_count: years.iter().map(|(_, c)| c).sum(),
        years: years
            .into_iter()
            .map(|(year, count)| NormYearCount { year, count })
            .collect(),
    })
}

/// Page paginée des textes d'un fond×année (`year = None` = bucket
/// « sans date »). 404 si le couple est vide ; la page hors bornes renvoie
/// une liste vide (total exact fourni).
pub async fn year_page(
    state: &AppState,
    fond: &str,
    year: Option<i32>,
    page: u32,
) -> Result<NormYearResponse> {
    let label = browsable_fond_label(fond)?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let total = repo
        .norm_fond_years(fond)
        .await?
        .into_iter()
        .find(|(y, _)| *y == year)
        .map(|(_, c)| c)
        .ok_or(ApiError::NotFound)?;

    let page = page.max(1);
    let offset = i64::from(page - 1) * i64::from(HUB_PAGE_SIZE);
    let rows = repo
        .norm_fond_year_texts(fond, year, i64::from(HUB_PAGE_SIZE), offset)
        .await?;
    Ok(NormYearResponse {
        fond: fond.to_string(),
        label,
        year,
        total,
        page,
        page_size: HUB_PAGE_SIZE,
        texts: rows
            .into_iter()
            .map(|r| NormHubText {
                slug: r.slug,
                title: r.title,
                date: r.date,
            })
            .collect(),
    })
}

/// Libellé d'un fond navigable par hub année — 404 sur un fond hors
/// taxonomie et sur `codes` (couvert par le catalogue `/codes`).
fn browsable_fond_label(fond: &str) -> Result<String> {
    if fond == "codes" || !NORM_FONDS.contains(&fond) {
        return Err(ApiError::NotFound);
    }
    Ok(norm_fond_label(fond).unwrap_or(fond).to_string())
}
