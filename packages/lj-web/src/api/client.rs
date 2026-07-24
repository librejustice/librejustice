//! Client typé contre `lj-dtos`, à surface unique (`ApiClient::from_context()` +
//! ~18 méthodes) et transport aiguillé par cible (ADR 0061 §2) :
//!
//! - **SSR** (`ssr`) : appel **in-process** à la couche service `lj-api`
//!   (`search::search`, `decisions::get_decision`/`similar_decisions`) via
//!   l'[`AppState`] fourni au contexte Leptos par `lj-server`. Aucun
//!   HTTP-vers-soi sur le chemin chaud du rendu.
//! - **Hydrate** (`hydrate`, wasm) : `reqwest` (backend `fetch` du navigateur)
//!   vers `/api` same-origin. Le header `Authorization: Bearer` vient du shim
//!   auth (`crate::auth::get_access_token`).
//!
//! Les composants ignorent le transport : ils appellent les mêmes méthodes sur
//! le `ApiClient` obtenu via [`ApiClient::from_context`].

use lj_dtos::{
    AnnuaireStatsResponse, ArticleSearchResponse, CitingDecisionHit, CoCitedArticle,
    CodeCatalogueResponse, CodeTocResponse, CorpusStatsResponse, DecisionDetail,
    DecisionPartiesResponse, DecisionViewsResponse, EntityDecisionsResponse,
    EntityDirectoryResponse, EntityPageResponse, EntityRegistreResponse, EntitySearchResponse,
    LawArticleResponse, LawCodeSummary, LawCompareResponse, LawSectionResponse, SearchContext,
    SearchHistoryResponse, SearchRequest, SearchResponse, SimilarDecisionsResponse,
    SuggestResponse,
};
// Types portés uniquement par les routes client (`/me`, signets, hover cards).
#[cfg(feature = "hydrate")]
use lj_dtos::{BookmarksResponse, DecisionPreview, UserProfile, UserProfileUpdate};

/// Fenetre de pagination offset-based des listes « Mon activite ». Port de
/// `PageParams`.
#[derive(Debug, Clone, Copy)]
pub struct PageParams {
    pub limit: u32,
    pub offset: u32,
}

/// Filtres de la recherche d'articles (page `/textes`). `code` borne à un
/// texte ; `jurisdiction`/`nature`/`source` filtrent le corpus. Chaque borne
/// `None` (ou vide) est ignorée.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextesFilters<'a> {
    pub code: Option<&'a str>,
    pub jurisdiction: Option<&'a str>,
    pub nature: Option<&'a str>,
    pub source: Option<&'a str>,
    /// Sur-facette « portée » (ADR 0196) : `norme` | `doctrine_administrative`.
    pub scope: Option<&'a str>,
}

/// Erreur de la frontiere API. Port de `ApiError` (message + status HTTP).
#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for ApiError {}

type ApiResult<T> = Result<T, ApiError>;

// ════════════════════════════ SSR : services in-process ════════════════════════════

/// Client SSR : appelle la couche service `lj-api` sur l'[`AppState`] partagé,
/// sans HTTP. Seules les routes rendues côté serveur (recherche amorcée,
/// détail décision + voisins) sont portées ici ; le reste de l'API (`/me`,
/// signets, historique…) est client-only (`hydrate`).
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ApiClient {
    state: lj_api::state::AppState,
}

#[cfg(feature = "ssr")]
impl ApiClient {
    /// Récupère l'[`AppState`] fourni au contexte Leptos par `lj-server`.
    pub fn from_context() -> Self {
        let state = leptos::prelude::use_context::<lj_api::state::AppState>()
            .expect("AppState fourni au contexte SSR par lj-server (leptos_routes_with_context)");
        Self { state }
    }

    pub async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse> {
        // Rendu SSR = client in-process sans session (auth côté hydrate) → anonyme.
        lj_api::search::search(
            &self.state,
            request,
            lj_dtos::ActivitySource::Web,
            false,
            context,
        )
        .await
        .map_err(map_service_error)
    }

    /// Suggestions multi-mots de la barre de recherche (in-process — jamais
    /// appelée au rendu SSR, le fetch part d'un Effect client ; parité de
    /// surface avec le client hydrate).
    pub async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        lj_api::search::suggest::suggest(&self.state, q, mode)
            .await
            .map_err(map_service_error)
    }

    pub async fn fetch_decision(&self, id: &str) -> ApiResult<DecisionDetail> {
        lj_api::decisions::get_decision(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    pub async fn fetch_similar_decisions(&self, id: &str) -> ApiResult<SimilarDecisionsResponse> {
        let hits = lj_api::decisions::similar_decisions(&self.state, id, 4)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))?;
        Ok(SimilarDecisionsResponse {
            decision_id: id.to_string(),
            hits,
        })
    }

    // ── Référentiel LEGI (`/texte`, ADR 0092) ─────────────────────────────────

    /// Article LEGI à une date (`date = Some`) ou en vigueur (`date = None`),
    /// avec sa timeline. Slug inconnu / article absent ⇒ 404.
    pub async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
    ) -> ApiResult<LawArticleResponse> {
        let result = match date {
            Some(date) => lj_api::legi::article_at_date_str(&self.state, code, num, date).await,
            None => lj_api::legi::article_in_force(&self.state, code, num).await,
        };
        result.map_err(|e| map_not_found(e, "Article introuvable"))
    }

    /// Comparaison de deux versions d'un article (ADR 0193). Bornes = dates
    /// ISO de fenêtre de version (`initiale` = borne ouverte).
    pub async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
    ) -> ApiResult<LawCompareResponse> {
        lj_api::legi::article_compare(&self.state, code, num, de, a)
            .await
            .map_err(|e| map_not_found(e, "Version introuvable"))
    }

    pub async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        lj_api::legi::article_citing(
            &self.state,
            code,
            num,
            date,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    /// Articles co-cités avec l'article (« souvent cité avec », Phase D).
    pub async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        lj_api::legi::article_co_cited(&self.state, code, num)
            .await
            .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    pub async fn fetch_legi_code_summary(&self, code: &str) -> ApiResult<LawCodeSummary> {
        lj_api::legi::code_summary(&self.state, code)
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    /// Recherche plein-texte d'articles (page `/textes`, ADR 0114). `code`
    /// borne la recherche à un texte ; `jurisdiction`/`nature`/`source` filtrent
    /// le corpus. Chaque filtre `None`/vide est ignoré côté service. Le
    /// `context` ne compte qu'au transport HTTP (historique) — le service
    /// in-process n'enregistre rien.
    pub async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        _context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        lj_api::legi::search_textes(
            &self.state,
            q,
            filters.code,
            filters.jurisdiction,
            filters.nature,
            filters.source,
            filters.scope,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(map_service_error)
    }

    /// Catalogue des codes du corpus (`/codes`). `head_only` borne aux
    /// familles de tête (codes, constitutions) — le SSR de la page.
    pub async fn fetch_codes_catalogue(&self, head_only: bool) -> ApiResult<CodeCatalogueResponse> {
        lj_api::legi::code_catalogue(&self.state, head_only)
            .await
            .map_err(map_service_error)
    }

    /// Compteurs globaux du corpus (page d'accueil). Servi depuis le cache
    /// process-local (`stats::corpus_stats`) — pas de requête par rendu.
    pub async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        lj_api::stats::corpus_stats(&self.state)
            .await
            .map(|s| (*s).clone())
            .map_err(map_service_error)
    }

    /// Table des matières d'un code (`/texte/{code}/sommaire`). Slug inconnu ⇒ 404.
    pub async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
    ) -> ApiResult<CodeTocResponse> {
        lj_api::legi::code_toc_str(&self.state, code, date)
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    /// Vue-lecture d'une section (`/texte/{code}/section/{cid}`, ADR 0207).
    pub async fn fetch_law_section(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
    ) -> ApiResult<LawSectionResponse> {
        lj_api::legi::law_section_str(&self.state, code, cid, date)
            .await
            .map_err(|e| map_not_found(e, "Section introuvable"))
    }

    // ── Fiche entité (`/entite`, ADR 0189) ──────────────────────────────────

    /// Identité registre + agrégats contentieux d'une entité. Uid inconnu ⇒ 404.
    pub async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        lj_api::entities::entity_page(&self.state, ns, id)
            .await
            .map_err(|e| map_not_found(e, "Entité introuvable"))
    }

    /// Décisions citant l'entité, paginées (plus récentes d'abord).
    pub async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse> {
        lj_api::entities::entity_decisions(&self.state, ns, id, page, page_size)
            .await
            .map_err(|e| map_not_found(e, "Entité introuvable"))
    }

    /// Volet registre de l'entité (APIs publiques à l'affichage, ADR 0199).
    pub async fn fetch_entity_registre(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<EntityRegistreResponse> {
        lj_api::registre::entity_registre(&self.state, ns, id)
            .await
            .map_err(map_service_error)
    }

    /// Acteurs (parties + conseils) extraits d'une décision (encart « Parties »).
    pub async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        lj_api::entities::decision_parties(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    // ── Annuaire des entités (`/annuaire`, ADR 0192) ─────────────────────────

    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    pub async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        lj_api::entities::annuaire_stats(&self.state)
            .await
            .map_err(map_service_error)
    }

    /// Recherche d'entités (annuaire). `kind` (slug de catégorie) borne si fourni.
    pub async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse> {
        let category = kind.map(resolve_kind).transpose()?;
        lj_api::entities::entity_search(&self.state, q, category, i64::from(limit))
            .await
            .map_err(map_service_error)
    }

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    /// `barreau` filtre les avocats `cnb:`.
    pub async fn fetch_entities_directory(
        &self,
        kind: &str,
        barreau: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let category = resolve_kind(kind)?;
        lj_api::entities::entity_directory(&self.state, category, barreau, page, page_size)
            .await
            .map_err(map_service_error)
    }

    // Listes « Mon activité » : portées par les fetchers infinite-scroll, qui ne
    // s'exécutent que côté client (Effect `hydrate`) — le SSR est anonyme (aucune
    // session in-process). Présentes pour que les closures compilent en `ssr` ;
    // jamais appelées au rendu serveur.
    pub async fn list_search_history(&self, _page: PageParams) -> ApiResult<SearchHistoryResponse> {
        Err(anonymous())
    }

    pub async fn list_decision_views(&self, _page: PageParams) -> ApiResult<DecisionViewsResponse> {
        Err(anonymous())
    }
}

/// Slug de catégorie annuaire → catégorie stockée (mapping `lj_api`). Slug
/// inconnu ⇒ 422, parité de la route HTTP (le front valide en amont via
/// `pages::annuaire::common::Kind`).
#[cfg(feature = "ssr")]
fn resolve_kind(kind: &str) -> ApiResult<&'static str> {
    lj_api::entities::kind_to_category(kind).ok_or_else(|| ApiError {
        message: format!("Catégorie inconnue : {kind}"),
        status: 422,
    })
}

/// Erreur « pas de session » du client SSR (anonyme in-process).
#[cfg(feature = "ssr")]
fn anonymous() -> ApiError {
    ApiError {
        message: "auth_required".to_string(),
        status: 401,
    }
}

/// Mappe une erreur service `lj-api` en [`ApiError`] (status + message).
#[cfg(feature = "ssr")]
fn map_service_error(err: lj_api::error::ApiError) -> ApiError {
    ApiError {
        status: err.status(),
        message: err.to_string(),
    }
}

/// Comme [`map_service_error`], mais remplace le message d'un 404 par `not_found`
/// (parité du message spécifique des routes décision).
#[cfg(feature = "ssr")]
fn map_not_found(err: lj_api::error::ApiError, not_found: &str) -> ApiError {
    let status = err.status();
    let message = if status == 404 {
        not_found.to_string()
    } else {
        err.to_string()
    };
    ApiError { message, status }
}

// ════════════════════════════ Hydrate : reqwest /api same-origin ════════════════════

#[cfg(feature = "hydrate")]
use reqwest::{Client, Method, RequestBuilder, StatusCode};

#[cfg(feature = "hydrate")]
use crate::auth::get_access_token;

#[cfg(feature = "hydrate")]
impl ApiError {
    fn new(message: &str, status: StatusCode) -> Self {
        Self {
            message: message.to_string(),
            status: status.as_u16(),
        }
    }

    /// Erreur reseau (fetch/transport echoue avant toute reponse HTTP). Status 0,
    /// parite avec un `TypeError` cote `fetch` JS.
    fn transport(err: reqwest::Error) -> Self {
        Self {
            message: err.to_string(),
            status: 0,
        }
    }
}

/// Client hydrate : `reqwest` vers `/api` (route same-origin par `lj-server`).
#[cfg(feature = "hydrate")]
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: Client,
}

#[cfg(feature = "hydrate")]
impl ApiClient {
    /// Client navigateur : base `{origine}/api` same-origin, backend `fetch`.
    /// L'URL doit être ABSOLUE : reqwest-wasm parse l'URL via `url::Url` qui
    /// rejette un chemin relatif (`/api/…`) → "builder error". On préfixe donc
    /// l'origine de la page (`lj-server` sert `/api` en same-origin).
    pub fn from_context() -> Self {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        Self {
            base_url: format!("{origin}/api"),
            http: Client::new(),
        }
    }

    /// Construit une requete avec le header `Authorization` si une session
    /// existe. Port de `authHeaders()`.
    async fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut builder = self
            .http
            .request(method, format!("{}{path}", self.base_url));
        if let Some(token) = get_access_token().await {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    /// Envoie une requete sans corps, mappant `!ok` -> `ApiError(message)`. La
    /// closure `on_404` permet le message specifique des routes decision.
    async fn send_unit(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<&str>,
    ) -> ApiResult<()> {
        let resp = builder.send().await.map_err(ApiError::transport)?;
        check_status(&resp, message, not_found)?;
        Ok(())
    }

    /// Envoie une requete et desserialise le corps JSON en `T`.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<&str>,
    ) -> ApiResult<T> {
        let resp = builder.send().await.map_err(ApiError::transport)?;
        check_status(&resp, message, not_found)?;
        resp.json::<T>().await.map_err(ApiError::transport)
    }

    // ── Recherche ──────────────────────────────────────────────────────────

    pub async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse> {
        let mut query = build_search_query(request);
        if context == SearchContext::Teaser {
            query.push_str("&context=teaser");
        }
        let builder = self.request(Method::GET, &format!("/search?{query}")).await;
        self.send_json(builder, "search failed", None).await
    }

    // ── Decision ───────────────────────────────────────────────────────────

    pub async fn fetch_decision(&self, id: &str) -> ApiResult<DecisionDetail> {
        let builder = self.request(Method::GET, &format!("/decision/{id}")).await;
        self.send_json(
            builder,
            "decision fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    pub async fn fetch_similar_decisions(&self, id: &str) -> ApiResult<SimilarDecisionsResponse> {
        let builder = self
            .request(Method::GET, &format!("/decision/{id}/similar"))
            .await;
        self.send_json(
            builder,
            "similar decisions fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    /// Prévisualisation légère d'une décision (hover card, ADR 0168). Client-only :
    /// le survol n'existe qu'au navigateur.
    pub async fn fetch_decision_preview(&self, id: &str) -> ApiResult<DecisionPreview> {
        let builder = self
            .request(Method::GET, &format!("/decision/{id}/preview"))
            .await;
        self.send_json(
            builder,
            "decision preview fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    // ── Référentiel LEGI (`/texte`, ADR 0092) ─────────────────────────────────

    pub async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
    ) -> ApiResult<LawArticleResponse> {
        let path = match date {
            Some(date) => format!("/texte/{code}/{num}/{date}"),
            None => format!("/texte/{code}/{num}"),
        };
        let builder = self.request(Method::GET, &path).await;
        self.send_json(
            builder,
            "legi article fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    /// Comparaison de deux versions d'un article (ADR 0193).
    pub async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
    ) -> ApiResult<LawCompareResponse> {
        let builder = self
            .request(
                Method::GET,
                &format!("/texte/{code}/{num}/compare/{de}/{a}"),
            )
            .await;
        self.send_json(
            builder,
            "legi compare fetch failed",
            Some("Version introuvable"),
        )
        .await
    }

    pub async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        let date_q = date.map(|d| format!("&date={d}")).unwrap_or_default();
        let builder = self
            .request(
                Method::GET,
                &format!("/texte/{code}/{num}/citing{}{date_q}", page_query(page)),
            )
            .await;
        self.send_json(
            builder,
            "legi citing fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    /// Articles co-cités avec l'article (« souvent cité avec », Phase D).
    pub async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        let builder = self
            .request(Method::GET, &format!("/texte/{code}/{num}/related"))
            .await;
        self.send_json(
            builder,
            "legi related fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    pub async fn fetch_legi_code_summary(&self, code: &str) -> ApiResult<LawCodeSummary> {
        let builder = self.request(Method::GET, &format!("/texte/{code}")).await;
        self.send_json(builder, "legi code fetch failed", Some("Code introuvable"))
            .await
    }

    /// Recherche plein-texte d'articles (page `/textes`, ADR 0114).
    pub async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        let mut path = format!(
            "/search-textes?q={}&limit={}&offset={}",
            url_encode(q),
            page.limit,
            page.offset
        );
        if context == SearchContext::Teaser {
            path.push_str("&context=teaser");
        }
        for (key, value) in [
            ("code", filters.code),
            ("jurisdiction", filters.jurisdiction),
            ("nature", filters.nature),
            ("source", filters.source),
            ("scope", filters.scope),
        ] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                path.push_str(&format!("&{key}={}", url_encode(v)));
            }
        }
        let builder = self.request(Method::GET, &path).await;
        self.send_json(builder, "search textes failed", None).await
    }

    /// Catalogue des codes du corpus (`/codes`). `head_only` borne aux
    /// familles de tête (codes, constitutions).
    pub async fn fetch_codes_catalogue(&self, head_only: bool) -> ApiResult<CodeCatalogueResponse> {
        let path = if head_only {
            "/codes?scope=head"
        } else {
            "/codes"
        };
        let builder = self.request(Method::GET, path).await;
        self.send_json(builder, "codes catalogue fetch failed", None)
            .await
    }

    /// Compteurs globaux du corpus (page d'accueil).
    pub async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        let builder = self.request(Method::GET, "/corpus-stats").await;
        self.send_json(builder, "corpus stats fetch failed", None)
            .await
    }

    /// Table des matières d'un code (`/texte/{code}/sommaire`). Slug inconnu ⇒ 404.
    pub async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
    ) -> ApiResult<CodeTocResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let builder = self
            .request(Method::GET, &format!("/texte/{code}/sommaire{date_q}"))
            .await;
        self.send_json(builder, "code toc fetch failed", Some("Code introuvable"))
            .await
    }

    /// Vue-lecture d'une section (`/texte/{code}/section/{cid}`, ADR 0207).
    pub async fn fetch_law_section(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
    ) -> ApiResult<LawSectionResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let builder = self
            .request(Method::GET, &format!("/texte/{code}/section/{cid}{date_q}"))
            .await;
        self.send_json(
            builder,
            "law section fetch failed",
            Some("Section introuvable"),
        )
        .await
    }

    // ── Fiche entité (`/entite`, ADR 0189) ──────────────────────────────────

    pub async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        let builder = self
            .request(Method::GET, &format!("/entity/{ns}/{id}"))
            .await;
        self.send_json(builder, "entity fetch failed", Some("Entité introuvable"))
            .await
    }

    pub async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse> {
        let builder = self
            .request(
                Method::GET,
                &format!("/entity/{ns}/{id}/decisions?page={page}&page_size={page_size}"),
            )
            .await;
        self.send_json(
            builder,
            "entity decisions fetch failed",
            Some("Entité introuvable"),
        )
        .await
    }

    /// Volet registre de l'entité (APIs publiques à l'affichage, ADR 0199).
    pub async fn fetch_entity_registre(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<EntityRegistreResponse> {
        let builder = self
            .request(Method::GET, &format!("/entity/{ns}/{id}/registre"))
            .await;
        self.send_json(builder, "entity registre fetch failed", None)
            .await
    }

    pub async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        let builder = self
            .request(Method::GET, &format!("/decision/{id}/parties"))
            .await;
        self.send_json(
            builder,
            "decision parties fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    // ── Autocomplétion (ADR 0216) ─────────────────────────────────────────────

    /// Suggestions multi-mots de la barre de recherche.
    /// `mode` ∈ `jurisprudence` | `textes` | `annuaire`.
    pub async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        let path = format!("/suggest?q={}&mode={mode}", url_encode(q));
        let builder = self.request(Method::GET, &path).await;
        self.send_json(builder, "suggest fetch failed", None).await
    }

    // ── Annuaire des entités (`/annuaire`, ADR 0192) ─────────────────────────

    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    pub async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        let builder = self.request(Method::GET, "/entities/stats").await;
        self.send_json(builder, "annuaire stats fetch failed", None)
            .await
    }

    /// Recherche d'entités (annuaire). `kind` borne à une catégorie si fourni.
    pub async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse> {
        let mut path = format!("/entities/search?q={}&limit={limit}", url_encode(q));
        if let Some(kind) = kind.filter(|k| !k.is_empty()) {
            path.push_str(&format!("&kind={}", url_encode(kind)));
        }
        let builder = self.request(Method::GET, &path).await;
        self.send_json(builder, "entities search failed", None)
            .await
    }

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    /// `barreau` filtre les avocats `cnb:`.
    pub async fn fetch_entities_directory(
        &self,
        kind: &str,
        barreau: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let mut path = format!(
            "/entities/directory?kind={}&page={page}&page_size={page_size}",
            url_encode(kind)
        );
        if let Some(barreau) = barreau.filter(|b| !b.is_empty()) {
            path.push_str(&format!("&barreau={}", url_encode(barreau)));
        }
        let builder = self.request(Method::GET, &path).await;
        self.send_json(builder, "entities directory fetch failed", None)
            .await
    }

    // ── Compte ─────────────────────────────────────────────────────────────

    pub async fn fetch_me(&self) -> ApiResult<UserProfile> {
        let builder = self.request(Method::GET, "/me").await;
        self.send_json(builder, "me fetch failed", None).await
    }

    pub async fn update_me(&self, update: &UserProfileUpdate) -> ApiResult<UserProfile> {
        let builder = self.request(Method::PATCH, "/me").await.json(update);
        self.send_json(builder, "me update failed", None).await
    }

    pub async fn set_activity_tracking(&self, enabled: bool) -> ApiResult<UserProfile> {
        let builder = self
            .request(Method::PUT, "/me/activity-tracking")
            .await
            .json(&lj_dtos::ActivityTrackingUpdate { enabled });
        self.send_json(builder, "activity tracking update failed", None)
            .await
    }

    pub async fn delete_account(&self) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, "/me").await;
        self.send_unit(builder, "account deletion failed", None)
            .await
    }

    // ── Signets ──────────────────────────────────────────────────────────────

    pub async fn list_bookmarks(&self) -> ApiResult<BookmarksResponse> {
        let builder = self.request(Method::GET, "/me/bookmarks").await;
        self.send_json(builder, "bookmarks fetch failed", None)
            .await
    }

    pub async fn add_bookmark(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self
            .request(Method::PUT, &format!("/me/bookmarks/{decision_id}"))
            .await;
        self.send_unit(builder, "bookmark add failed", None).await
    }

    pub async fn remove_bookmark(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self
            .request(Method::DELETE, &format!("/me/bookmarks/{decision_id}"))
            .await;
        self.send_unit(builder, "bookmark remove failed", None)
            .await
    }

    // ── Historique de recherche ────────────────────────────────────────────────

    pub async fn list_search_history(&self, page: PageParams) -> ApiResult<SearchHistoryResponse> {
        let builder = self
            .request(
                Method::GET,
                &format!("/me/search-history{}", page_query(page)),
            )
            .await;
        self.send_json(builder, "history fetch failed", None).await
    }

    pub async fn delete_search_history_entry(&self, entry_id: i64) -> ApiResult<()> {
        let builder = self
            .request(Method::DELETE, &format!("/me/search-history/{entry_id}"))
            .await;
        self.send_unit(builder, "history delete failed", None).await
    }

    pub async fn clear_search_history(&self) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, "/me/search-history").await;
        self.send_unit(builder, "history clear failed", None).await
    }

    // ── Decisions consultees ───────────────────────────────────────────────────

    pub async fn list_decision_views(&self, page: PageParams) -> ApiResult<DecisionViewsResponse> {
        let builder = self
            .request(
                Method::GET,
                &format!("/me/decision-views{}", page_query(page)),
            )
            .await;
        self.send_json(builder, "decision views fetch failed", None)
            .await
    }

    pub async fn record_decision_view(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self
            .request(Method::POST, &format!("/me/decision-views/{decision_id}"))
            .await;
        self.send_unit(builder, "decision view record failed", None)
            .await
    }

    pub async fn delete_decision_view(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self
            .request(Method::DELETE, &format!("/me/decision-views/{decision_id}"))
            .await;
        self.send_unit(builder, "decision view delete failed", None)
            .await
    }

    pub async fn clear_decision_views(&self) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, "/me/decision-views").await;
        self.send_unit(builder, "decision views clear failed", None)
            .await
    }
}

/// Verifie le status HTTP, mappant `404` vers `not_found` (si fourni) et tout
/// autre `!ok` vers `message`. Port du pattern `if (!response.ok) throw`.
#[cfg(feature = "hydrate")]
fn check_status(resp: &reqwest::Response, message: &str, not_found: Option<&str>) -> ApiResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::NOT_FOUND {
        if let Some(nf) = not_found {
            return Err(ApiError::new(nf, status));
        }
    }
    Err(ApiError::new(message, status))
}

/// Query string `?limit=&offset=`. Port de `pageQuery`.
#[cfg(feature = "hydrate")]
fn page_query(page: PageParams) -> String {
    format!("?limit={}&offset={}", page.limit, page.offset)
}

/// Encode un `SearchRequest` en query string. Port fidele de `buildSearchParams`
/// (mêmes cles camelCase, mêmes omissions conditionnelles).
#[cfg(feature = "hydrate")]
fn build_search_query(request: &SearchRequest) -> String {
    use lj_dtos::SortOrder;

    let mut params: Vec<(String, String)> = Vec::new();
    params.push(("q".to_string(), request.query.clone()));

    append_multi(&mut params, "jurisdictionType", &request.jurisdiction_type);
    append_multi(&mut params, "solution", &request.solution);
    append_multi(&mut params, "procedure", &request.procedure);
    append_multi(&mut params, "office", &request.office);
    append_multi(&mut params, "legalDomain", &request.legal_domain);
    append_multi_str(&mut params, "jurisdictionCode", &request.jurisdiction_code);
    append_multi_str(&mut params, "legalInstrument", &request.legal_instrument);
    append_multi_str(&mut params, "legalArticle", &request.legal_article);
    append_multi(&mut params, "significance", &request.significance);
    append_multi_str(&mut params, "publication", &request.publication);

    if let Some(date_from) = &request.date_from {
        params.push(("dateFrom".to_string(), date_from.clone()));
    }
    if let Some(date_to) = &request.date_to {
        params.push(("dateTo".to_string(), date_to.clone()));
    }
    // `mode` toujours emis : le DTO le materialise (defaut Auto) la ou le TS
    // l'omettait quand `undefined`. L'API resout Auto cote serveur — equivalent.
    params.push(("mode".to_string(), enum_value(&request.mode)));
    // `sort` omis si Relevance (parite `if (sort !== "relevance")`).
    if !matches!(request.sort, SortOrder::Relevance) {
        params.push(("sort".to_string(), enum_value(&request.sort)));
    }
    params.push(("limit".to_string(), request.limit.to_string()));
    if request.offset != 0 {
        params.push(("offset".to_string(), request.offset.to_string()));
    }
    if request.ai_mode {
        params.push(("aiMode".to_string(), "true".to_string()));
    }

    encode_pairs(&params)
}

/// Serialise une valeur enum DTO en sa chaine serde (sans guillemets).
#[cfg(feature = "hydrate")]
fn enum_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("enum DTO serialise en chaine")
}

/// Ajoute une cle multivaluee a partir d'enums DTO. Port de `appendMulti`.
#[cfg(feature = "hydrate")]
fn append_multi<T: serde::Serialize>(
    params: &mut Vec<(String, String)>,
    key: &str,
    values: &Option<Vec<T>>,
) {
    if let Some(values) = values {
        for v in values {
            params.push((key.to_string(), enum_value(v)));
        }
    }
}

/// Ajoute une cle multivaluee de chaines. Port de `appendMulti` (cas String).
#[cfg(feature = "hydrate")]
fn append_multi_str(params: &mut Vec<(String, String)>, key: &str, values: &Option<Vec<String>>) {
    if let Some(values) = values {
        for v in values {
            params.push((key.to_string(), v.clone()));
        }
    }
}

/// Encode une liste de paires en query string (RFC 3986, `URLSearchParams`).
#[cfg(feature = "hydrate")]
fn encode_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode `application/x-www-form-urlencoded` (espace -> `+`), parite
/// `URLSearchParams.toString()`.
#[cfg(feature = "hydrate")]
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
