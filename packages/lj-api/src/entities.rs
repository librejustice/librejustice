//! Fiche entité (ADR 0189) : identité registre + agrégats contentieux dérivés
//! de `decision_party`, décisions citantes paginées, et encart « Parties » d'une
//! décision.
//!
//! Fonctions pures `(&AppState, …) -> Result<Dto>` (même patron que
//! [`crate::decisions`]) ; les adaptateurs axum (path/query, headers de cache)
//! vivent dans [`crate::routes`]. Tout le SQL vit dans `lj-store`
//! (`repository::entities`, règle #2) ; ce module ne fait que résoudre les
//! libellés référentiels et assembler les DTOs.

use lj_dtos::{
    AnnuaireCategorieStatsDto, AnnuaireStatsResponse, DecisionPartiesResponse, DecisionPartyDto,
    EntityCounselDto, EntityDecisionHitDto, EntityDecisionsResponse, EntityDenominationDto,
    EntityDirectoryItemDto, EntityDirectoryResponse, EntityHeaderDto, EntityKeyCountDto,
    EntityPageResponse, EntitySearchResponse, EntityStatsDto, EntityYearCountDto,
};
use lj_store::repository::{DecisionRepository, EntityDirectoryRow, EntityJurisdictionCountRow};
use std::collections::HashMap;
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

/// Uid namespacé depuis les segments d'URL (`{ns}` / `{id}` → `ns:id`).
fn build_uid(ns: &str, id: &str) -> String {
    format!("{ns}:{id}")
}

/// Libellé d'affichage de la `forme` d'une entité : code catégorie juridique
/// INSEE résolu (`5710` → « SAS », nomenclature embarquée `lj-core`) ; toute
/// valeur hors nomenclature (« association », « avocat (paris) »…) passe
/// telle quelle — jamais `None` sur une forme présente.
fn forme_label(forme: Option<String>) -> Option<String> {
    forme.map(|f| {
        lj_core::forme_juridique::forme_juridique_label(&f)
            .map(str::to_string)
            .unwrap_or(f)
    })
}

/// Libellé d'affichage d'une juridiction : le nom du référentiel `jurisdiction`
/// (`tj_paris` → « Tribunal judiciaire de Paris »), repli sur le libellé du type
/// (`TJ` → « Tribunal judiciaire »), puis le code brut.
fn jurisdiction_label(row: &EntityJurisdictionCountRow, refs: &Referential) -> (String, String) {
    match row.jurisdiction_code.as_deref() {
        Some(code) => {
            let label = refs
                .jurisdiction(code)
                .map(|j| j.label.clone())
                .unwrap_or_else(|| {
                    refs.jurisdiction_type_label(&row.jurisdiction_type)
                        .unwrap_or(&row.jurisdiction_type)
                        .to_string()
                });
            (code.to_string(), label)
        }
        None => {
            let label = refs
                .jurisdiction_type_label(&row.jurisdiction_type)
                .unwrap_or(&row.jurisdiction_type)
                .to_string();
            (row.jurisdiction_type.clone(), label)
        }
    }
}

/// Nombre max de conseils co-occurrents affichés sur la fiche.
const TOP_COUNSEL_LIMIT: i64 = 10;

/// Fiche entité (`GET /entity/{ns}/{id}`) : en-tête registre + agrégats
/// contentieux. `None` amont (uid inconnu) → 404.
#[instrument(skip(state), fields(db.system = "postgresql", uid = %build_uid(ns, id)))]
pub async fn entity_page(state: &AppState, ns: &str, id: &str) -> Result<EntityPageResponse> {
    let uid = build_uid(ns, id);
    let refs = referential(state).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let header = repo.entity_header(&uid).await?.ok_or(ApiError::NotFound)?;
    let denominations = repo.entity_denominations(&uid).await?;
    let counts = repo.entity_contentieux_counts(&uid).await?;
    let by_year = repo.entity_by_year(&uid).await?;
    let by_jurisdiction = repo.entity_by_jurisdiction(&uid).await?;
    let top_counsel = repo.entity_top_counsel(&uid, TOP_COUNSEL_LIMIT).await?;

    let namespace = uid
        .split_once(':')
        .map(|(n, _)| n)
        .unwrap_or("")
        .to_string();

    let header = EntityHeaderDto {
        uid: header.uid,
        namespace,
        nature: header.nature,
        denomination: header.denomination,
        sigle: header.sigle,
        forme: forme_label(header.forme),
        active: header.active,
        denominations: denominations
            .into_iter()
            .map(|d| EntityDenominationDto {
                denomination: d.denomination,
                date_debut: d.date_debut,
                date_fin: d.date_fin,
            })
            .collect(),
    };

    let stats = EntityStatsDto {
        decision_count: counts.decision_count,
        as_applicant: counts.as_applicant,
        as_defendant: counts.as_defendant,
        by_year: by_year
            .into_iter()
            .map(|y| EntityYearCountDto {
                year: y.year,
                count: y.count,
            })
            .collect(),
        by_jurisdiction: by_jurisdiction
            .into_iter()
            .map(|row| {
                let (key, label) = jurisdiction_label(&row, &refs);
                EntityKeyCountDto {
                    key,
                    label,
                    count: row.count,
                }
            })
            .collect(),
        top_counsel: top_counsel
            .into_iter()
            .map(|c| EntityCounselDto {
                uid: c.entity_uid,
                name: c.value,
                count: c.count,
            })
            .collect(),
    };

    Ok(EntityPageResponse { header, stats })
}

/// Décisions citant l'entité (`GET /entity/{ns}/{id}/decisions`), plus récentes
/// d'abord, paginées. `page` est 1-basé. Une entité inconnue renvoie une page
/// vide (total 0) — pas de 404 (la ressource paginée existe, elle est vide).
#[instrument(skip(state), fields(db.system = "postgresql", uid = %build_uid(ns, id)))]
pub async fn entity_decisions(
    state: &AppState,
    ns: &str,
    id: &str,
    page: i64,
    page_size: i64,
) -> Result<EntityDecisionsResponse> {
    let uid = build_uid(ns, id);
    let refs = referential(state).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let offset = (page - 1) * page_size;
    let (total, rows) = repo.entity_decisions(&uid, page_size, offset).await?;

    // Hydratation `SearchHit` par le chemin de la recherche (rendu unifié
    // `ResultCard`) : mêmes titres/tags/snippets que les pages de résultats.
    let ids: Vec<i64> = rows.iter().map(|r| r.decision_id).collect();
    let mut roles: HashMap<i64, (String, Option<String>)> = rows
        .into_iter()
        .map(|r| (r.decision_id, (r.quality, r.side)))
        .collect();
    let hits = crate::search::hits_for_decision_ids(&conn, &ids, &refs).await?;
    let items = hits
        .into_iter()
        .map(|(decision_id, hit)| {
            // Présent par construction : `decision_id` vient de `rows`.
            let (quality, side) = roles.remove(&decision_id).unwrap();
            EntityDecisionHitDto { hit, side, quality }
        })
        .collect();

    Ok(EntityDecisionsResponse {
        total,
        page,
        page_size,
        items,
    })
}

/// Encart « Parties » d'une décision (`GET /decision/{decision_id}/parties`) :
/// acteurs extraits, liés au registre quand résolus, ordre stable. Décision
/// inconnue → 404 ; décision sans partie → liste vide.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn decision_parties(
    state: &AppState,
    public_id: &str,
) -> Result<DecisionPartiesResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let rows = repo
        .decision_parties(public_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let parties = rows
        .into_iter()
        .map(|r| DecisionPartyDto {
            quality: r.quality,
            side: r.side,
            value: r.value,
            nature: r.nature,
            barreau: r.barreau,
            entity_uid: r.entity_uid,
        })
        .collect();

    Ok(DecisionPartiesResponse { parties })
}

// ── Annuaire des entités (ADR 0192) ──────────────────────────────────────────

/// Nombre max de résultats de recherche d'entités (borne dure de l'endpoint).
pub const ENTITY_SEARCH_MAX_LIMIT: i64 = 50;
/// Longueur minimale d'une requête de recherche d'entités (codepoints).
pub const ENTITY_SEARCH_MIN_QUERY: usize = 2;
/// Profondeur max du listing (`page × page_size`, ADR 0239) — au-delà, 422 ;
/// la recherche par préfixe couvre l'accès à la traîne du registre.
pub const ENTITY_DIRECTORY_MAX_DEPTH: i64 = 10_000;

/// Mappe le slug de catégorie de l'API (kebab, aligné sur la route web
/// `/annuaire/{kind}`) vers la valeur stockée dans `entity.category`.
/// `None` = kind inconnu (→ 422 côté route).
pub fn kind_to_category(kind: &str) -> Option<&'static str> {
    match kind {
        "entreprises" => Some("entreprises"),
        "personnes-publiques" => Some("personnes_publiques"),
        "associations" => Some("associations"),
        "avocats" => Some("avocats"),
        "cabinets" => Some("cabinets"),
        _ => None,
    }
}

/// Plie une requête de recherche du MÊME fold que `entity_contentieux
/// .denomination_folded` (fold_stable + blancs réduits, cf. chargeur de
/// registres), neutralise les wildcards LIKE et suffixe `%` : préfixe prêt pour
/// `LIKE $1 ESCAPE '\'`.
fn fold_search_prefix(q: &str) -> String {
    let folded = lj_core::text::fold_stable(q)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut escaped = String::with_capacity(folded.len() + 1);
    for c in folded.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

/// Convertit une ligne d'annuaire en item DTO.
fn directory_item(row: EntityDirectoryRow) -> EntityDirectoryItemDto {
    EntityDirectoryItemDto {
        uid: row.uid,
        namespace: row.namespace,
        denomination: row.denomination,
        nature: row.nature,
        forme: forme_label(row.forme),
        active: row.active,
        barreau: row.barreau,
        decision_count: row.decision_count,
    }
}

/// Recherche d'entités (`GET /entities/search`) : préfixe de dénomination sur
/// le registre complet (ADR 0239), filtre `kind` optionnel (déjà mappé en
/// catégorie), tri contentieux décroissant. `q` est déjà validé
/// (≥ 2 codepoints), `limit` borné par la route.
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn entity_search(
    state: &AppState,
    q: &str,
    category: Option<&str>,
    limit: i64,
) -> Result<EntitySearchResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let folded_like = fold_search_prefix(q);
    let rows = repo.entity_search(&folded_like, category, limit).await?;
    Ok(EntitySearchResponse {
        items: rows.into_iter().map(directory_item).collect(),
    })
}

/// Listing paginé d'une catégorie de l'annuaire (`GET /entities/directory`),
/// filtre barreau optionnel. `category`/`page`/`page_size` déjà validés par la
/// route ; `page` 1-basé.
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn entity_directory(
    state: &AppState,
    category: &str,
    barreau: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<EntityDirectoryResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let offset = (page - 1) * page_size;
    let (total, contentieux, rows) = repo
        .entity_directory(category, barreau, page_size, offset)
        .await?;
    Ok(EntityDirectoryResponse {
        total,
        contentieux,
        page,
        page_size,
        items: rows.into_iter().map(directory_item).collect(),
    })
}

/// Compteurs de l'annuaire par catégorie (`GET /entities/stats`) : total du
/// registre chargé + entités avec contentieux (ADR 0233).
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn annuaire_stats(state: &AppState) -> Result<AnnuaireStatsResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let mut resp = AnnuaireStatsResponse::default();
    for (category, registre, contentieux) in repo.annuaire_stats().await? {
        let slot = match category.as_str() {
            "entreprises" => &mut resp.entreprises,
            "personnes_publiques" => &mut resp.personnes_publiques,
            "associations" => &mut resp.associations,
            "avocats" => &mut resp.avocats,
            "cabinets" => &mut resp.cabinets,
            _ => continue,
        };
        *slot = AnnuaireCategorieStatsDto {
            registre,
            contentieux,
        };
    }
    Ok(resp)
}
