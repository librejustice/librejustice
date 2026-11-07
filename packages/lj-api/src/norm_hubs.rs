//! Hubs du catalogue des normes : catalogue `/api/normes`, hub
//! `/api/normes/{juris}/{fond}`, page année `/api/normes/{juris}/{fond}/{annee}`
//! (`juris` = segment d'URL, `annee` = année ou `sans-date`). Chemins de crawl SSR vers les textes, réponses
//! stables (le corpus bouge à l'ingest quotidien), cache CDN long côté routes.

use crate::error::{ApiError, Result};
use crate::state::AppState;
use lj_dtos::{
    NormCatalogueResponse, NormFondEntry, NormFondResponse, NormHubText, NormJurisdictionEntry,
    NormYearCount, NormYearResponse,
};
use lj_norm::referential_labels::{norm_fond_label, NORM_FONDS};
use lj_store::repository::DecisionRepository;
use lj_vocab::HUB_PAGE_SIZE;
use std::sync::Arc;

/// Token d'URL du bucket des textes sans date de parcours.
pub const UNDATED_TOKEN: &str = "sans-date";

/// Catalogue of each jurisdiction's fonds, in the editorial order of `NORM_FONDS`.
/// A single cache entry (key `()`), TTL 12 h: the aggregate over the whole catalogue
/// is never recomputed per request.
pub async fn catalogue(state: &AppState) -> Result<Arc<NormCatalogueResponse>> {
    state
        .norm_catalogue_cache
        .try_get_with(String::new(), load_catalogue(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue normes: {e}")))
}

async fn load_catalogue(state: &AppState) -> Result<Arc<NormCatalogueResponse>> {
    let conn = state.pool.get().await?;
    let counts = DecisionRepository::new(&conn).norm_catalogue().await?;
    // Le fond `codes` affiche le compte de son catalogue (codes + constitutions
    // avec ≥ 1 article en vigueur), pas le compte brut de la taxonomie : la
    // carte et la page disent le même nombre.
    let codes = crate::legi::code_catalogue(state, true).await?;

    let mut stored: Vec<&str> = counts.iter().map(|(j, _, _)| j.as_str()).collect();
    stored.sort_unstable();
    stored.dedup();
    let mut jurisdictions: Vec<NormJurisdictionEntry> = stored
        .into_iter()
        .map(|jurisdiction| {
            let fonds: Vec<NormFondEntry> = NORM_FONDS
                .iter()
                .filter_map(|fond| {
                    let count = if *fond == "codes" {
                        let n = codes
                            .entries
                            .iter()
                            .filter(|e| e.jurisdiction == jurisdiction)
                            .count() as i64;
                        (n > 0).then_some(n)?
                    } else {
                        counts
                            .iter()
                            .find(|(j, f, _)| j == jurisdiction && f == fond)
                            .map(|(_, _, c)| *c)?
                    };
                    Some(NormFondEntry {
                        fond: (*fond).to_string(),
                        label: norm_fond_label(fond).unwrap_or(fond).to_string(),
                        text_count: count,
                    })
                })
                .collect();
            NormJurisdictionEntry {
                jurisdiction: lj_vocab::url_segment(jurisdiction),
                text_count: fonds.iter().map(|f| f.text_count).sum(),
                fonds,
            }
        })
        .collect();
    jurisdictions.sort_by_key(|j| std::cmp::Reverse(j.text_count));
    Ok(Arc::new(NormCatalogueResponse { jurisdictions }))
}

/// A fond's hub: covered years + volume. 404 if the fond is unknown to the
/// taxonomy, empty, or `codes` (its catalogue is served by `/api/codes`).
pub async fn hub(state: &AppState, segment: &str, fond: &str) -> Result<NormFondResponse> {
    let label = browsable_fond_label(fond)?;
    let jurisdiction = lj_vocab::jurisdiction_of_segment(segment);
    let conn = state.pool.get().await?;
    let years = DecisionRepository::new(&conn)
        .norm_fond_years(&jurisdiction, fond)
        .await?;
    if years.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(NormFondResponse {
        jurisdiction: segment.to_string(),
        fond: fond.to_string(),
        label,
        search_paths: lj_norm::norm_fond::fond_search_paths(&jurisdiction, fond),
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
    segment: &str,
    fond: &str,
    year: Option<i32>,
    page: u32,
) -> Result<NormYearResponse> {
    let label = browsable_fond_label(fond)?;
    let jurisdiction = lj_vocab::jurisdiction_of_segment(segment);
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let total = repo.norm_fond_year_total(&jurisdiction, fond, year).await?;
    if total == 0 {
        return Err(ApiError::NotFound);
    }

    let page = page.max(1);
    let offset = i64::from(page - 1) * i64::from(HUB_PAGE_SIZE);
    let rows = repo
        .norm_fond_year_texts(&jurisdiction, fond, year, i64::from(HUB_PAGE_SIZE), offset)
        .await?;
    Ok(NormYearResponse {
        jurisdiction: segment.to_string(),
        fond: fond.to_string(),
        label,
        search_paths: lj_norm::norm_fond::fond_search_paths(&jurisdiction, fond),
        year,
        total,
        page,
        page_size: HUB_PAGE_SIZE,
        texts: rows
            .into_iter()
            .map(|r| NormHubText {
                slug: r.slug,
                title: r.title,
                jurisdiction: r.jurisdiction,
                date: r.date,
            })
            .collect(),
    })
}

/// Label of a fond browsable by year hub: 404 on a fond outside the taxonomy and
/// on `codes` (covered by its catalogue).
fn browsable_fond_label(fond: &str) -> Result<String> {
    if fond == "codes" || !NORM_FONDS.contains(&fond) {
        return Err(ApiError::NotFound);
    }
    Ok(norm_fond_label(fond).unwrap_or(fond).to_string())
}
