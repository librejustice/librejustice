//! Référentiel de droit positif — law-at-date, timeline, citations, sommaire de
//! code (ADR 0097, généralise ADR 0092).
//!
//! Handlers purs `(&AppState, …) -> Result<DTO>` (même forme que
//! [`crate::decisions`]) ; l'adaptation axum (path/query, headers de cache) vit
//! dans [`crate::routes`]. Le `code` d'URL est résolu en `text_uid` par lookup
//! **exact** sur le slug ([`DecisionRepository::resolve_referential_code`]) ; rien
//! ne matche ⇒ 404 ([`ApiError::NotFound`]). Le `num` d'URL **est déjà** la clé
//! canonique (`num_key`) : nos liens internes la portent (DTO `numKey`, ADR 0123
//! §2), aucune normalisation au runtime serve — c'est ce qui sort la pile
//! `lj-extract` du chemin serve.

use chrono::{Datelike, NaiveDate};
use lj_core::source_authority::{is_live_authoritative, source_authority};
use lj_dtos::{
    ArticleNeighbor, ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse,
    CitingDecisionHit, CodeCatalogueEntry, CodeCatalogueResponse, CodeTocResponse, FacetChoice,
    JuridictionType, LawArticleResponse, LawArticleVersion, LawCodeSummary, TocEntry,
};
use lj_store::repository::{
    ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, CitingDecisionRow,
    DecisionRepository, FacetCount, LawVersionRow, LegalArticleRow, LegalTextCatalogRow,
    TocArticleRow,
};
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::state::AppState;
use crate::titles::decision_title;

/// URL Légifrance versionnée d'un article (ADR 0092) :
/// `/codes/article_lc/{LEGIARTI}/{date}`. `date` ISO `YYYY-MM-DD` est la date de
/// consultation (date demandée pour la version-à-date, sinon `date_debut` de la
/// version servie).
/// URL Légifrance absolue et versionnée d'un article **natif** Légifrance
/// (`source_uid` = LEGIARTI…). `None` pour une source curée (droit étranger, traités :
/// `source_uid` synthétique `text_uid#…`) qui porte déjà sa propre `source_url`
/// (ADR 0131). Sert à *remplir* `source_url` quand elle est absente — pas de champ
/// `legifrance_url` dédié (lien « source » unique et générique).
fn legifrance_source_url(source_uid: &str, date: &str) -> Option<String> {
    if source_uid.contains('#') {
        return None;
    }
    Some(format!(
        "https://www.legifrance.gouv.fr/codes/article_lc/{source_uid}/{date}"
    ))
}

/// Rend une `date_debut` en ISO `YYYY-MM-DD`, en mappant la sentinelle borne
/// ouverte (`0001-01-01`, posée par l'upsert pour les versions sans début connu —
/// la colonne est NOT NULL depuis ADR 0112) vers la chaîne vide, comme une borne
/// ouverte l'était avant (DTO `dateDebut` vide = pas de début). Un `None` (qui ne
/// devrait plus survenir, la colonne étant NOT NULL) est traité de même.
fn date_debut_display(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 => d.to_string(),
        _ => String::new(),
    }
}

/// Convertit une ligne de timeline du repo en DTO version (dates ISO telles
/// quelles, `dateDebut` borne ouverte ⇒ chaîne vide, `dateFin` absente ⇒ pas de
/// fin). `dateDebut` du DTO est requise : une borne ouverte (sentinelle
/// `0001-01-01` posée par l'upsert, ou source non versionnée) est rendue en
/// chaîne vide. `source_uid` = identifiant natif de la version (LEGIARTI…).
fn version_dto(row: LawVersionRow) -> LawArticleVersion {
    let date_debut = match row.date_debut {
        Some(d) if d != "0001-01-01" => d,
        _ => String::new(),
    };
    LawArticleVersion {
        legiarti: row.source_uid,
        etat: row.status,
        date_debut,
        date_fin: row.date_fin,
    }
}

/// Assemble la réponse article (version servie + timeline) à partir d'une ligne
/// `legal_article` et de la timeline. `consult_date` est la date de consultation
/// pour l'URL Légifrance (date demandée, sinon `date_debut` de la version, sinon
/// chaîne vide pour une borne ouverte). `legiarti` = identifiant natif de la
/// version (`source_uid`, LEGIARTI…) ; `legitext` = identité du texte
/// (`text_uid`).
async fn article_response(
    repo: &DecisionRepository<'_>,
    code: &str,
    article: LegalArticleRow,
    consult_date: &str,
    versions: Vec<LawVersionRow>,
    context: Vec<ArticleNeighborRow>,
) -> Result<LawArticleResponse> {
    let date_debut = date_debut_display(article.date_debut);
    let date_fin = article.date_fin.map(|d| d.to_string());
    // Lien « source » unique (ADR 0131) : l'URL curée si présente (droit étranger,
    // traités), sinon — pour un article natif Légifrance — la page Légifrance versionnée.
    let source_url = article
        .source_url
        .clone()
        .or_else(|| legifrance_source_url(&article.source_uid, consult_date));
    // Fraîcheur effective (ADR 0129) : `source_asof` par ligne (curé / bulk / jafbase),
    // sinon — pour les sources live re-synchronisées (legifrance/kali) — la date du
    // dernier sync (`ingest_freshness`). Axe distinct de `translation`.
    let source_asof = match article.source_asof {
        Some(d) => Some(d.to_string()),
        None if is_live_authoritative(&article.source) => repo
            .get_ingest_freshness(&article.source)
            .await?
            .map(|d| d.to_string()),
        None => None,
    };
    let source_authority = source_authority(&article.source).label().to_string();
    // Titre humain du texte pour l'en-tête « Article N du … » : le `code` d'URL est un
    // slug (le front l'humaniserait laidement, ex. « code de la famille senegal sn »).
    let code_title = repo.referential_title(code).await?;
    Ok(LawArticleResponse {
        legiarti: article.source_uid,
        legitext: article.text_uid,
        code: code.to_string(),
        code_title,
        num: article.num,
        num_key: article.num_key,
        etat: article.status,
        date_debut,
        source: article.source,
        titre_text: article.title_path,
        date_fin,
        texte: article.texte,
        texte_original: article.texte_original,
        lang_original: article.lang_original,
        translation: article.translation,
        source_asof,
        source_authority,
        source_url,
        source_upstream_url: article.source_upstream_url,
        nota: article.nota,
        versions: versions.into_iter().map(version_dto).collect(),
        context: context.into_iter().map(neighbor_dto).collect(),
    })
}

/// Mappe un voisin du repo vers le DTO (numéro + état + flag courant, ADR 0114).
fn neighbor_dto(row: ArticleNeighborRow) -> ArticleNeighbor {
    ArticleNeighbor {
        num: row.num,
        num_key: row.num_key,
        etat: row.status,
        current: row.current,
    }
}

/// Résout le `code` d'URL en `text_uid` du texte de référentiel par lookup
/// **exact** du slug (ADR 0112 §6 / ADR 0123 §2). 404 si le slug est inconnu :
/// nos liens internes portent le slug canonique (DTO), une forme legacy/tapée-main
/// ne bénéficie plus du rattrapage flou (assumé, ADR 0123).
async fn resolve_text(repo: &DecisionRepository<'_>, code: &str) -> Result<String> {
    repo.resolve_referential_code(code)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Version en vigueur d'un article (`GET /api/loi/{code}/{num}`), avec sa
/// timeline. 404 si le code ou l'article n'est pas en vigueur.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_in_force(
    state: &AppState,
    code: &str,
    num: &str,
) -> Result<LawArticleResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL = clé canonique (nos liens portent `numKey`, ADR 0123 §2).
    let num_key = num;

    let article = repo
        .law_article_at_date(&text_uid, num_key, None)
        .await?
        .ok_or(ApiError::NotFound)?;
    let consult_date = date_debut_display(article.date_debut);
    let versions = repo.law_article_versions(&text_uid, num_key).await?;
    let context = repo.article_context(&text_uid, num_key, None).await?;
    article_response(&repo, code, article, &consult_date, versions, context).await
}

/// Version d'un article à une date (`GET /api/loi/{code}/{num}/{date}`), avec sa
/// timeline. 404 si le code est inconnu ou si aucune version ne couvre la date.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date = %date))]
pub async fn article_at_date(
    state: &AppState,
    code: &str,
    num: &str,
    date: NaiveDate,
) -> Result<LawArticleResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL = clé canonique (nos liens portent `numKey`, ADR 0123 §2).
    let num_key = num;

    let article = repo
        .law_article_at_date(&text_uid, num_key, Some(date))
        .await?
        .ok_or(ApiError::NotFound)?;
    let consult_date = date.to_string();
    let versions = repo.law_article_versions(&text_uid, num_key).await?;
    let context = repo.article_context(&text_uid, num_key, Some(date)).await?;
    article_response(&repo, code, article, &consult_date, versions, context).await
}

/// Variante de [`article_at_date`] acceptant une date ISO `YYYY-MM-DD` en
/// chaîne, pour les appelants in-process (client SSR `lj-web`) qui ne
/// manipulent pas `chrono`. Format invalide ⇒ 422
/// ([`ApiError::Unprocessable`]) — même frontière de validation que la route
/// HTTP (#12).
pub async fn article_at_date_str(
    state: &AppState,
    code: &str,
    num: &str,
    date: &str,
) -> Result<LawArticleResponse> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", "date"],
            date,
            "invalid date",
        ))
    })?;
    article_at_date(state, code, num, parsed).await
}

/// Convertit une ligne `law_decisions_citing` en DTO citant : le `title`
/// machine est dérivé des métadonnées (même source que le détail/MCP), le
/// libellé de repli du type venant du référentiel (ADR 0146).
fn citing_dto(row: CitingDecisionRow, refs: &crate::referential::Referential) -> CitingDecisionHit {
    let dockets = row.docket_numbers.filter(|d| !d.is_empty());
    let title = decision_title(
        refs.juridiction_type_label(&row.juridiction_type)
            .unwrap_or(&row.juridiction_type),
        row.jurisdiction_name.as_deref(),
        None,
        row.date_lecture.as_deref(),
        dockets.as_deref(),
    );
    let juridiction_type = parse_jur_type(&row.juridiction_type).unwrap_or(JuridictionType::Ta);
    CitingDecisionHit {
        id: row.public_id,
        title,
        juridiction_type,
        date_lecture: row.date_lecture,
    }
}

/// Désérialise un code `juridiction_type` issu de la DB en [`JuridictionType`]
/// (mapping serde des variantes). `None` si le code est inattendu.
fn parse_jur_type(raw: &str) -> Option<JuridictionType> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Décisions citant un article (`GET /api/loi/{code}/{num}/citing`), paginées.
/// 404 si le code est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, limit, offset))]
pub async fn article_citing(
    state: &AppState,
    code: &str,
    num: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<CitingDecisionHit>> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL = clé canonique (nos liens portent `numKey`, ADR 0123 §2).
    let num_key = num;

    let rows = repo
        .law_decisions_citing(&text_uid, num_key, limit, offset)
        .await?;
    let refs = crate::referential::referential(state).await?;
    Ok(rows.into_iter().map(|r| citing_dto(r, &refs)).collect())
}

/// Sommaire d'un code (`GET /api/loi/{code}`) : métadonnées + nombre d'articles
/// en vigueur. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_summary(state: &AppState, code: &str) -> Result<LawCodeSummary> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let row = repo
        .law_code_summary(code)
        .await?
        .ok_or(ApiError::NotFound)?;
    // `slug` est non-NULL pour un code résolu par son slug (la requête filtre
    // `WHERE slug = $1`).
    let slug = row.slug.unwrap_or_else(|| code.to_string());
    Ok(LawCodeSummary {
        legitext: row.text_uid,
        code: slug,
        titre: row.title,
        nature: row.nature,
        derniere_modification: row.last_modified,
        article_count: row.article_count,
    })
}

/// Longueur cible d'un extrait d'article dans les résultats `/recherche-textes`.
const ARTICLE_SNIPPET_MAX: usize = 280;

/// Recherche plein-texte d'articles (`GET /api/search-textes`, ADR 0114),
/// **titre-primaire** : jambe titre (`search_title` formé, boostée + expansions
/// d'alias) > jambe corps. `code` optionnel borne la recherche à un texte (résolu
/// comme les pages `/loi`) ; absent ⇒ tout le référentiel navigable. Les facettes
/// `jurisdiction`/`nature`/`source` et le `total` sont comptés sous le même
/// prédicat BM25 + mêmes filtres que la page de hits, en une requête `GROUPING
/// SETS` lancée **en parallèle** des hits sur sa propre connexion — le prédicat
/// BM25 (~600 ms plein corpus) est le poste dominant, on ne le paie qu'une fois
/// par jambe. Chaque hit reçoit un extrait surligné via
/// [`crate::snippets::highlight`] (fallback corps tronqué).
#[instrument(skip(state), fields(db.system = "postgresql", limit, offset))]
#[allow(clippy::too_many_arguments)]
pub async fn search_textes(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<ArticleSearchResponse> {
    let checkout = || async {
        state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))
    };
    let (conn_hits, conn_stats) = tokio::try_join!(checkout(), checkout())?;
    let repo_hits = DecisionRepository::new(&conn_hits);
    let repo_stats = DecisionRepository::new(&conn_stats);

    let text_uid = match code {
        Some(c) if !c.is_empty() => Some(resolve_text(&repo_hits, c).await?),
        _ => None,
    };
    // Expansion d'alias (acronymes/noms usuels → forme développée) OR-ée dans la
    // jambe titre — substitut lexical au sémantique (ADR 0114).
    let expansions = lj_core::aliases::expand_query(q);
    let (rows, stats) = tokio::try_join!(
        repo_hits.search_articles(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            limit,
            offset,
        ),
        repo_stats.article_search_stats(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
        ),
    )?;
    Ok(ArticleSearchResponse {
        hits: hits_with_snippets(rows, q),
        total: stats.total,
        facets: facets_dto(stats),
    })
}

/// Mappe les facettes du repo vers le DTO (chaque `FacetCount` devient un
/// `FacetChoice`, tri préservé : count décroissant puis valeur croissante),
/// libellés humanisés via `lj_core::referential_labels` (pays FR pour
/// `jurisdiction`, natures LEGI/curées pour `nature`, diffuseurs à jeton court
/// pour `source` — valeur brute en repli).
fn facets_dto(stats: ArticleSearchStats) -> ArticleSearchFacets {
    use lj_core::referential_labels::{jurisdiction_label, nature_label, source_label};
    let map = |rows: Vec<FacetCount>, label: fn(&str) -> Option<&'static str>| {
        rows.into_iter()
            .map(|row| FacetChoice {
                label: label(&row.value)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.value.clone()),
                value: row.value,
                count: row.count,
                parent: None,
            })
            .collect()
    };
    ArticleSearchFacets {
        jurisdiction: map(stats.jurisdiction, jurisdiction_label),
        nature: map(stats.nature, nature_label),
        source: map(stats.source, source_label),
    }
}

/// Catalogue des codes navigables (`GET /api/codes`) : un texte de référentiel
/// par entrée, avec son nombre d'articles. `slug` du texte = `code` du DTO (lien
/// `/loi/{code}`).
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn code_catalogue(state: &AppState) -> Result<CodeCatalogueResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let rows = repo.list_legal_texts().await?;
    Ok(CodeCatalogueResponse {
        entries: rows.into_iter().map(catalogue_entry).collect(),
    })
}

/// Mappe une ligne de catalogue du repo vers le DTO (slug → `code`).
fn catalogue_entry(row: LegalTextCatalogRow) -> CodeCatalogueEntry {
    CodeCatalogueEntry {
        code: row.slug,
        title: row.title,
        nature: row.nature,
        jurisdiction: row.jurisdiction,
        article_count: row.article_count,
    }
}

/// Table des matières d'un code (`GET /api/loi/{code}/sommaire`) : articles
/// ordonnés, chacun avec son fil d'Ariane et son état. 404 si le slug est inconnu
/// (même résolution que les pages `/loi`).
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_toc(state: &AppState, code: &str) -> Result<CodeTocResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    let rows = repo.code_table_of_contents(&text_uid).await?;
    Ok(CodeTocResponse {
        entries: rows.into_iter().map(toc_entry).collect(),
    })
}

/// Mappe une ligne de sommaire du repo vers le DTO (`position` ignorée — l'ordre
/// est porté par la séquence de la requête).
fn toc_entry(row: TocArticleRow) -> TocEntry {
    TocEntry {
        num: row.num,
        num_key: row.num_key,
        title_path: row.title_path,
        status: row.status,
    }
}

/// Construit les hits DTO en surlignant le corps (`snippets::highlight`, mêmes
/// tokenizers que l'index). Les docs sont indexés par position (id synthétique) ;
/// un doc qui ne matche pas (absent du retour `highlight`) retombe sur un corps
/// tronqué. `slug` est non-NULL (la requête filtre `t.slug IS NOT NULL`).
fn hits_with_snippets(rows: Vec<ArticleSearchRow>, q: &str) -> Vec<ArticleSearchHit> {
    let docs: Vec<(i64, String)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i as i64, r.texte.clone().unwrap_or_default()))
        .collect();
    let snippets = crate::snippets::highlight(&docs, q, ARTICLE_SNIPPET_MAX);
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let snippet = snippets.get(&(i as i64)).cloned().unwrap_or_else(|| {
                truncate_plain(r.texte.as_deref().unwrap_or(""), ARTICLE_SNIPPET_MAX)
            });
            ArticleSearchHit {
                code: r.slug.unwrap_or_default(),
                code_title: r.code_title,
                num: r.num,
                num_key: r.num_key,
                titre_path: r.title_path,
                snippet,
                source: r.source,
            }
        })
        .collect()
}

/// Tronque un texte brut à `max` caractères (frontière char), suffixe `…` si
/// coupé. Fallback quand le corps ne matche pas la requête de surlignage.
fn truncate_plain(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &trimmed[..byte_idx]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legifrance_source_url_native_is_versioned() {
        assert_eq!(
            legifrance_source_url("LEGIARTI000006419292", "1992-05-15").as_deref(),
            Some("https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419292/1992-05-15")
        );
    }

    #[test]
    fn legifrance_source_url_none_for_curated() {
        // Source curée (`source_uid` synthétique) : pas de page Légifrance ; elle porte
        // sa propre `source_url` (ADR 0131) — pas de lien Légifrance fabriqué.
        assert_eq!(
            legifrance_source_url("JORFTEXT000000694290#6@1968-12-27", "2002-12-26"),
            None
        );
    }

    #[test]
    fn parse_jur_type_maps_known_codes() {
        assert_eq!(parse_jur_type("CC"), Some(JuridictionType::Cc));
        assert_eq!(parse_jur_type("TA"), Some(JuridictionType::Ta));
        assert_eq!(parse_jur_type("ZZZ"), None);
    }

    #[test]
    fn citing_dto_builds_title_and_falls_back_jur_type() {
        let row = CitingDecisionRow {
            id: 42,
            public_id: "CETATEXT000012345678".to_string(),
            juridiction_type: "CE".to_string(),
            jurisdiction_name: Some("Conseil d'État".to_string()),
            date_lecture: Some("2024-02-13".to_string()),
            docket_numbers: Some(vec!["123456".to_string()]),
        };
        let refs = crate::referential::Referential::new(Vec::new(), Vec::new());
        let hit = citing_dto(row, &refs);
        assert_eq!(hit.id, "CETATEXT000012345678");
        assert_eq!(hit.juridiction_type, JuridictionType::Ce);
        assert_eq!(hit.title, "Conseil d'État, 13 février 2024, 123456");
        assert_eq!(hit.date_lecture.as_deref(), Some("2024-02-13"));
    }
}
