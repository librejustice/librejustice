//! Hubs juridiction (ADR 0253) : catalogue `/api/juridictions`, hub
//! `/api/juridictions/{code}`, page année `/api/juridictions/{code}/{annee}`.
//! Chemins de crawl SSR vers les décisions — réponses stables (le corpus
//! bouge à l'ingest quotidien), cache CDN long côté routes.

use crate::error::{ApiError, Result};
use crate::referential;
use crate::state::AppState;
use lj_dtos::{
    JurisdictionCatalogueResponse, JurisdictionHubDecision, JurisdictionHubEntry,
    JurisdictionHubResponse, JurisdictionTypeGroup, JurisdictionYearCount,
    JurisdictionYearResponse,
};
use lj_store::repository::DecisionRepository;
use std::sync::Arc;

/// Décisions par page hub juridiction×année.
pub const HUB_PAGE_SIZE: u32 = 100;

/// Ordre éditorial des familles du catalogue : ordre administratif, ordre
/// judiciaire, juridictions spécialisées, cours européennes. Une famille
/// inconnue passe en queue, dans l'ordre du GROUP BY (alphabétique).
const TYPE_ORDER: &[&str] = &[
    "CE", "CAA", "TA", "CONSTIT", "TC", "CC", "CA", "TJ", "TCOM", "CNDA", "CNIL", "CEDH", "CJUE",
];

/// Catalogue des juridictions groupé par famille. Une seule entrée de cache
/// (clé `()`), TTL 12 h — l'agrégat sur 3,7 M de lignes ne se recalcule
/// jamais par requête.
pub async fn catalogue(state: &AppState) -> Result<Arc<JurisdictionCatalogueResponse>> {
    state
        .jurisdiction_catalogue_cache
        .try_get_with((), load_catalogue(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue juridictions: {e}")))
}

async fn load_catalogue(state: &AppState) -> Result<Arc<JurisdictionCatalogueResponse>> {
    let referential = referential::referential(state).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let rows = repo.jurisdiction_catalogue().await?;

    let mut groups: Vec<JurisdictionTypeGroup> = Vec::new();
    for row in rows {
        let group = match groups
            .iter_mut()
            .find(|g| g.jurisdiction_type == row.jurisdiction_type)
        {
            Some(g) => g,
            None => {
                groups.push(JurisdictionTypeGroup {
                    label: referential
                        .jurisdiction_type_label(&row.jurisdiction_type)
                        .unwrap_or(&row.jurisdiction_type)
                        .to_string(),
                    jurisdiction_type: row.jurisdiction_type.clone(),
                    jurisdictions: Vec::new(),
                });
                groups.last_mut().expect("groupe fraîchement poussé")
            }
        };
        group.jurisdictions.push(JurisdictionHubEntry {
            code: row.code,
            label: row.label,
            decision_count: row.decision_count,
        });
    }
    let rank = |t: &str| {
        TYPE_ORDER
            .iter()
            .position(|c| *c == t)
            .unwrap_or(TYPE_ORDER.len())
    };
    groups.sort_by_key(|g| rank(&g.jurisdiction_type));
    Ok(Arc::new(JurisdictionCatalogueResponse { groups }))
}

/// Hub d'une juridiction : années couvertes + volume. 404 si le code est
/// inconnu du référentiel ou sans décision datée.
pub async fn hub(state: &AppState, code: &str) -> Result<JurisdictionHubResponse> {
    let referential = referential::referential(state).await?;
    let entry = referential.jurisdiction(code).ok_or(ApiError::NotFound)?;
    let label = entry.label.clone();
    let jurisdiction_type = entry.jurisdiction_type.clone();
    let type_label = referential
        .jurisdiction_type_label(&jurisdiction_type)
        .unwrap_or(&jurisdiction_type)
        .to_string();

    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let years = repo.jurisdiction_years(code).await?;
    if years.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(JurisdictionHubResponse {
        code: code.to_string(),
        label,
        jurisdiction_type,
        type_label,
        decision_count: years.iter().map(|(_, c)| c).sum(),
        years: years
            .into_iter()
            .map(|(year, count)| JurisdictionYearCount { year, count })
            .collect(),
    })
}

/// Page paginée des décisions d'une juridiction×année. 404 si le couple est
/// vide ; la page hors bornes renvoie une liste vide (total exact fourni).
pub async fn year_page(
    state: &AppState,
    code: &str,
    year: i32,
    page: u32,
) -> Result<JurisdictionYearResponse> {
    let referential = referential::referential(state).await?;
    let entry = referential.jurisdiction(code).ok_or(ApiError::NotFound)?;
    let label = entry.label.clone();

    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let total = repo
        .jurisdiction_years(code)
        .await?
        .into_iter()
        .find(|(y, _)| *y == year)
        .map(|(_, c)| c)
        .ok_or(ApiError::NotFound)?;

    let page = page.max(1);
    let offset = i64::from(page - 1) * i64::from(HUB_PAGE_SIZE);
    let rows = repo
        .jurisdiction_year_decisions(code, year, i64::from(HUB_PAGE_SIZE), offset)
        .await?;
    Ok(JurisdictionYearResponse {
        code: code.to_string(),
        label,
        year,
        total,
        page,
        page_size: HUB_PAGE_SIZE,
        decisions: rows
            .into_iter()
            .map(|r| JurisdictionHubDecision {
                public_id: r.public_id,
                title: r.title,
                date_lecture: r.date_lecture,
            })
            .collect(),
    })
}
