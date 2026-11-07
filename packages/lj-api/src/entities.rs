//! Entity page: registry identity and litigation aggregates from the actors
//! linked to it, paginated citing decisions, and a decision's « Parties »
//! panel.
//!
//! Fonctions pures `(&AppState, …) -> Result<Dto>` ; les adaptateurs axum
//! (path/query, headers de cache) vivent dans [`crate::routes`]. Tout le SQL vit
//! dans `lj-store` (`repository::entities`) ; ce module ne fait que résoudre les
//! libellés référentiels et assembler les DTOs.

use lj_dtos::{
    AnnuaireCategorieStatsDto, AnnuaireStatsResponse, DecisionPartiesResponse, DecisionPartyDto,
    EntityDecisionHitDto, EntityDecisionsResponse, EntityDenominationDto, EntityDirectoryItemDto,
    EntityDirectoryResponse, EntityHeaderDto, EntityKeyCountDto, EntityPageResponse,
    EntitySearchResponse, EntityStatsDto, EntityYearCountDto,
};
use lj_store::repository::{DecisionRepository, EntityDirectoryRow, EntityJurisdictionCountRow};
use std::collections::HashMap;
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

/// Uid namespacé depuis les segments d'URL (`{ns}` / `{id}` → `ns:id`).
fn build_uid(ns: &str, id: &str) -> String {
    match ns {
        // A name-derived uid answers to its accented spelling too.
        "cnb" | "oacc" => format!("{ns}:{}", lj_fold::fold_chars(id)),
        _ => format!("{ns}:{id}"),
    }
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

/// Fiche entité (`GET /entity/{ns}/{id}`) : en-tête registre + agrégats
/// contentieux. `None` amont (uid inconnu) → 404.
#[instrument(skip(state), fields(db.system = "postgresql", uid = %build_uid(ns, id)))]
pub async fn entity_page(state: &AppState, ns: &str, id: &str) -> Result<EntityPageResponse> {
    let uid = build_uid(ns, id);
    let refs = referential(state).await?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let header = repo.entity_header(&uid).await?.ok_or(ApiError::NotFound)?;
    let denominations = repo.entity_denominations(&uid).await?;
    let counts = repo.entity_contentieux_counts(&uid).await?;
    let by_year = repo.entity_by_year(&uid).await?;
    let by_jurisdiction = repo.entity_by_jurisdiction(&uid).await?;

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
    };

    Ok(EntityPageResponse { header, stats })
}

/// Décisions citant l'entité (`GET /entity/{ns}/{id}/decisions`), plus récentes
/// d'abord, paginées. `page` est 1-basé. Une entité inconnue renvoie une page
/// vide (total 0), pas de 404.
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
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let offset = (page - 1) * page_size;
    let (total, rows) = repo.entity_decisions(&uid, page_size, offset).await?;

    // Hydratation `SearchHit` par le chemin de la recherche : mêmes titres, tags et
    // snippets que les pages de résultats.
    let ids: Vec<i64> = rows.iter().map(|r| r.decision_id).collect();
    let mut roles: HashMap<i64, (String, Option<String>)> = rows
        .into_iter()
        .map(|r| (r.decision_id, (r.quality, r.side)))
        .collect();
    let hits = crate::search::hits_for_decision_ids(&conn, &ids, "", &refs).await?;
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
    let conn = state.pool.get().await?;
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
            ord: r.ord,
            party_ord: r.party_ord,
        })
        .collect();

    Ok(DecisionPartiesResponse { parties })
}

// ── Annuaire des entités ─────────────────────────────────────────────────────

/// Nombre max de résultats de recherche d'entités (borne dure de l'endpoint).
pub const ENTITY_SEARCH_MAX_LIMIT: i64 = 50;
/// Longueur minimale d'une requête de recherche d'entités (codepoints).
pub const ENTITY_SEARCH_MIN_QUERY: usize = 2;
/// Profondeur max du listing (`page × page_size`) — au-delà, 422.
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

/// Natural persons: an account is required to reach them, and they are never listed.
const NATURAL_PERSON_CATEGORIES: &[&str] = &["avocats"];

/// Exact complement of [`NATURAL_PERSON_CATEGORIES`] over the `kind_to_category` image.
const PUBLIC_CATEGORIES: &[&str] = &[
    "entreprises",
    "personnes_publiques",
    "associations",
    "cabinets",
];

pub fn names_natural_persons(category: &str) -> bool {
    NATURAL_PERSON_CATEGORIES.contains(&category)
}

/// Categories a search is allowed to read. `None` = no restriction.
fn search_scope(category: Option<&str>, authenticated: bool) -> Option<Vec<&str>> {
    match (category, authenticated) {
        (Some(c), _) => Some(vec![c]),
        (None, true) => None,
        (None, false) => Some(PUBLIC_CATEGORIES.to_vec()),
    }
}

/// Folds a search query like the `lj_fold(denomination)` expression of the
/// `entity` indexes, escapes the LIKE wildcards and appends `%`: a prefix ready
/// for `LIKE $1 ESCAPE '\'`.
fn fold_search_prefix(q: &str) -> String {
    let folded = lj_fold::fold(q);
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

/// Entity search (`GET /entities/search`): denomination prefix over the whole
/// registry, optional `category` narrowing, contentious volume first. `q` and
/// `limit` are validated by the route.
///
/// `authenticated` decides the scope when no category is asked for: an anonymous
/// caller searches [`PUBLIC_CATEGORIES`] only, so no unnamed search ever surfaces
/// a natural person. An explicit natural-person `category` is refused upstream.
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn entity_search(
    state: &AppState,
    q: &str,
    category: Option<&str>,
    authenticated: bool,
    limit: i64,
) -> Result<EntitySearchResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let scope = search_scope(category, authenticated);
    let folded_like = fold_search_prefix(q);
    let rows = repo
        .entity_search(&folded_like, scope.as_deref(), limit)
        .await?;
    Ok(EntitySearchResponse {
        items: rows.into_iter().map(directory_item).collect(),
    })
}

/// Paginated listing of one directory category (`GET /entities/directory`).
/// `category`, `page` and `page_size` are validated by the route, which also
/// refuses a natural-person category; `page` is 1-based.
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn entity_directory(
    state: &AppState,
    category: &str,
    page: i64,
    page_size: i64,
) -> Result<EntityDirectoryResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let offset = (page - 1) * page_size;
    let (total, contentieux, rows) = repo.entity_directory(category, page_size, offset).await?;
    Ok(EntityDirectoryResponse {
        total,
        contentieux,
        page,
        page_size,
        items: rows.into_iter().map(directory_item).collect(),
    })
}

/// Compteurs de l'annuaire par catégorie (`GET /entities/stats`) : total du
/// registre chargé + entités avec contentieux.
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn annuaire_stats(state: &AppState) -> Result<AnnuaireStatsResponse> {
    let conn = state.pool.get().await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every slug `kind_to_category` answers on, so the tests below range over the
    /// whole image and not over a list that could drift from it.
    const KIND_SLUGS: [&str; 5] = [
        "entreprises",
        "personnes-publiques",
        "associations",
        "avocats",
        "cabinets",
    ];

    #[test]
    fn an_anonymous_search_reaches_no_natural_person() {
        let scope = search_scope(None, false).expect("anonymous search is bounded");
        assert!(!scope.iter().any(|c| names_natural_persons(c)));
    }

    #[test]
    fn public_categories_are_the_exact_complement() {
        for slug in KIND_SLUGS {
            let category = kind_to_category(slug).expect("known slug");
            assert_eq!(
                PUBLIC_CATEGORIES.contains(&category),
                !names_natural_persons(category),
                "{category} sits in neither list or in both"
            );
        }
    }

    #[test]
    fn an_account_lifts_the_category_bound() {
        assert!(search_scope(None, true).is_none());
    }

    #[test]
    fn an_asked_category_is_honoured_as_asked() {
        assert_eq!(search_scope(Some("avocats"), true), Some(vec!["avocats"]));
    }
}
