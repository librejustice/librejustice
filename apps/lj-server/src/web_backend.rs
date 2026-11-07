//! The front's SSR backend: the `lj-api` service layer, called in-process on the
//! shared `AppState`, with no HTTP round trip to itself.

use async_trait::async_trait;
use lj_api::state::AppState;
use lj_dtos::{
    AccountSummary, AnnonceCatalogueResponse, AnnonceDetailResponse, AnnonceSearchRequest,
    AnnonceSearchResponse, AnnuaireStatsResponse, ArticleSearchResponse, CitingDecisionHit,
    CoCitedArticle, CodeCatalogueResponse, CodeTocResponse, CorpusStatsResponse, DecisionDetail,
    DecisionPartiesResponse, EntityDecisionsResponse, EntityDirectoryResponse, EntityPageResponse,
    EntityRegistreResponse, EntitySearchResponse, JurisdictionCatalogueResponse,
    JurisdictionHubResponse, JurisdictionYearResponse, LawArticleResponse, LawCodeSummary,
    LawCompareResponse, LawSectionResponse, NormCatalogueResponse, NormFondResponse,
    NormYearResponse, OpenSourceResponse, PlaceCorpusResponse, PlacesResponse, SearchContext,
    SearchRequest, SearchResponse, SimilarDecisionsResponse, SuggestResponse,
};
use lj_web::api::{ApiError, ApiResult, Backend, PageParams, TextesFilters};

pub struct ServiceBackend {
    pub state: AppState,
}

#[async_trait]
impl Backend for ServiceBackend {
    async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse> {
        // The SSR render has no session (auth lives on the hydrate side), so it is
        // anonymous. No budget either: there is no client socket to name, and the
        // user's own search leaves the wasm for `/internal/search`, where it counts.
        lj_api::search::search(
            &self.state,
            request,
            lj_api::usage::Caller::web_anonymous(context),
            None,
        )
        .await
        .map_err(map_service_error)
    }

    async fn search_count(
        &self,
        request: &SearchRequest,
    ) -> ApiResult<lj_dtos::SearchCountResponse> {
        lj_api::search::count(&self.state, request, None)
            .await
            .map_err(map_service_error)
    }

    async fn search_instruments(
        &self,
        request: &SearchRequest,
        needle: &str,
    ) -> ApiResult<lj_dtos::LegalInstrumentSuggestions> {
        lj_api::search::suggest_instruments(&self.state, request, needle, None)
            .await
            .map_err(map_service_error)
    }

    async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        lj_api::search::suggest::suggest(&self.state, q, mode)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_decision(&self, id: &str) -> ApiResult<DecisionDetail> {
        lj_api::decisions::get_decision(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    async fn fetch_similar_decisions(&self, id: &str) -> ApiResult<SimilarDecisionsResponse> {
        lj_api::decisions::similar_decisions(&self.state, id, 4)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawArticleResponse> {
        let juris = Some(juridiction(juris));
        let result = match date {
            Some(date) => {
                lj_api::legi::article_at_date_str(&self.state, code, juris, num, date, place).await
            }
            None => lj_api::legi::article_in_force(&self.state, code, juris, num, place).await,
        };
        result.map_err(|e| map_not_found(e, "Article introuvable"))
    }

    async fn fetch_places(&self) -> ApiResult<PlacesResponse> {
        lj_api::legi::places(&self.state)
            .await
            .map_err(|e| map_not_found(e, "Lieux introuvables"))
    }

    async fn fetch_place_corpus(
        &self,
        place: Option<&str>,
        date: Option<&str>,
    ) -> ApiResult<PlaceCorpusResponse> {
        lj_api::legi::place_corpus_str(&self.state, place, date)
            .await
            .map_err(|e| map_not_found(e, "Corpus introuvable"))
    }

    async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
        juris: &str,
    ) -> ApiResult<LawCompareResponse> {
        lj_api::legi::article_compare(&self.state, code, Some(juridiction(juris)), num, de, a)
            .await
            .map_err(|e| map_not_found(e, "Version introuvable"))
    }

    async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        lj_api::legi::article_citing(
            &self.state,
            code,
            Some(juridiction(juris)),
            num,
            date,
            place,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
        juris: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        lj_api::legi::article_co_cited(&self.state, code, Some(juridiction(juris)), num)
            .await
            .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    async fn fetch_legi_code_summary(&self, code: &str, juris: &str) -> ApiResult<LawCodeSummary> {
        lj_api::legi::code_summary(&self.state, code, Some(juridiction(juris)))
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        mode: lj_dtos::SearchMode,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        let date = |field, raw: Option<&str>| {
            raw.map(|d| lj_api::legi::query_date(field, d))
                .transpose()
                .map_err(map_service_error)
        };
        lj_api::legi::search_textes(
            &self.state,
            q,
            filters.code,
            filters.places,
            filters.jurisdictions,
            filters.source,
            filters.scope,
            date("as_of", filters.as_of)?,
            date("valid_from", filters.valid_from)?,
            i64::from(page.limit),
            i64::from(page.offset),
            mode,
            lj_api::usage::Caller::web_anonymous(context),
            // In-process SSR: no socket, so no caller to name.
            None,
        )
        .await
        .map_err(map_service_error)
    }

    async fn fetch_codes_catalogue(&self) -> ApiResult<CodeCatalogueResponse> {
        lj_api::legi::code_catalogue(&self.state, true)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    async fn fetch_juridictions(&self) -> ApiResult<JurisdictionCatalogueResponse> {
        lj_api::jurisdiction_hubs::catalogue(&self.state)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    async fn fetch_open_source(&self) -> ApiResult<OpenSourceResponse> {
        Ok(lj_api::open_source::response().clone())
    }

    async fn fetch_juridiction_hub(&self, code: &str) -> ApiResult<JurisdictionHubResponse> {
        lj_api::jurisdiction_hubs::hub(&self.state, code)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_juridiction_year(
        &self,
        code: &str,
        year: i32,
        page: u32,
    ) -> ApiResult<JurisdictionYearResponse> {
        lj_api::jurisdiction_hubs::year_page(&self.state, code, year, page)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_normes(&self) -> ApiResult<NormCatalogueResponse> {
        lj_api::norm_hubs::catalogue(&self.state)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    async fn fetch_norm_fond(&self, juris: &str, fond: &str) -> ApiResult<NormFondResponse> {
        lj_api::norm_hubs::hub(&self.state, juris, fond)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_norm_year(
        &self,
        juris: &str,
        fond: &str,
        year: Option<i32>,
        page: u32,
    ) -> ApiResult<NormYearResponse> {
        lj_api::norm_hubs::year_page(&self.state, juris, fond, year, page)
            .await
            .map_err(map_service_error)
    }

    async fn search_annonces(
        &self,
        request: &AnnonceSearchRequest,
    ) -> ApiResult<AnnonceSearchResponse> {
        lj_api::annonces::search(&self.state, request)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_annonce(&self, uid: &str) -> ApiResult<AnnonceDetailResponse> {
        lj_api::annonces::detail(&self.state, uid)
            .await
            .map_err(|e| map_not_found(e, "Consultation introuvable"))
    }

    async fn fetch_annonces_catalogue(&self) -> ApiResult<AnnonceCatalogueResponse> {
        lj_api::annonces::catalogue(&self.state)
            .await
            .map(|c| (*c).clone())
            .map_err(map_service_error)
    }

    async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        lj_api::stats::corpus_stats(&self.state)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<CodeTocResponse> {
        lj_api::legi::code_toc_str(&self.state, code, Some(juridiction(juris)), date)
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    async fn fetch_law_section(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawSectionResponse> {
        lj_api::legi::law_section_str(&self.state, code, Some(juridiction(juris)), path, date)
            .await
            .map_err(|e| map_not_found(e, "Section introuvable"))
    }

    async fn fetch_law_section_citing(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        lj_api::legi::section_citing(
            &self.state,
            code,
            Some(juridiction(juris)),
            path,
            date,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(|e| map_not_found(e, "Section introuvable"))
    }

    async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        lj_api::entities::entity_page(&self.state, ns, id)
            .await
            .map_err(|e| map_not_found(e, "Entité introuvable"))
    }

    async fn fetch_entity_decisions(
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

    async fn fetch_entity_registre(&self, ns: &str, id: &str) -> ApiResult<EntityRegistreResponse> {
        lj_api::registre::entity_registre(&self.state, ns, id)
            .await
            .map_err(map_service_error)
    }

    async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        lj_api::entities::decision_parties(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        lj_api::entities::annuaire_stats(&self.state)
            .await
            .map_err(map_service_error)
    }

    async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse> {
        let category = kind.map(resolve_kind).transpose()?;
        // SSR runs without a session, so it never reaches a natural-person category.
        lj_api::entities::entity_search(&self.state, q, category, false, i64::from(limit))
            .await
            .map_err(map_service_error)
    }

    async fn fetch_entities_directory(
        &self,
        kind: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let category = resolve_kind(kind)?;
        lj_api::entities::entity_directory(&self.state, category, page, page_size)
            .await
            .map_err(map_service_error)
    }

    async fn account_summary(&self, parts: &axum::http::request::Parts) -> Option<AccountSummary> {
        let (token, _) = lj_api::session::from_parts(parts)?;
        match lj_api::session::resolve(&self.state, &token).await {
            lj_api::session::Verdict::Porteur { sub, .. } => {
                lj_api::account::resume(&self.state, &sub).await.ok()
            }
            _ => None,
        }
    }

    async fn oauth_client_name(&self, client_id: &str) -> Option<String> {
        lj_api::oauth::client_name(&self.state, client_id)
            .await
            .ok()?
    }
}

/// The jurisdiction a URL segment names, or the segment itself, which no text is
/// filed under and which therefore answers 404 the way the HTTP seam does.
fn juridiction(segment: &str) -> &str {
    lj_api::legi::from_url_segment(segment).unwrap_or(segment)
}

/// Directory category slug to stored category. An unknown slug is a 422, as on
/// the HTTP route (the front validates it upstream).
fn resolve_kind(kind: &str) -> ApiResult<&'static str> {
    lj_api::entities::kind_to_category(kind).ok_or_else(|| ApiError {
        message: format!("Catégorie inconnue : {kind}"),
        status: 422,
    })
}

/// A service error as the front's [`ApiError`] (status and message).
fn map_service_error(err: lj_api::error::ApiError) -> ApiError {
    ApiError {
        status: err.status(),
        message: err.to_string(),
    }
}

/// As [`map_service_error`], with `not_found` as the message of a 404, the one the
/// HTTP routes give.
fn map_not_found(err: lj_api::error::ApiError, not_found: &str) -> ApiError {
    let status = err.status();
    let message = if status == 404 {
        not_found.to_string()
    } else {
        err.to_string()
    };
    ApiError { message, status }
}
