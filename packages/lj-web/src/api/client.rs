//! Client typé contre `lj-dtos`, à surface unique (`ApiClient::from_context()` +
//! ~18 méthodes) et transport aiguillé par cible :
//!
//! - **SSR** (`ssr`) : appel **in-process** à la couche service `lj-api`
//!   (`search::search`, `decisions::get_decision`/`similar_decisions`) via
//!   l'[`AppState`] fourni au contexte Leptos par `lj-server`. Aucun
//!   HTTP-vers-soi sur le chemin chaud du rendu.
//! - **Hydrate** (`hydrate`, wasm) : `reqwest` (backend `fetch` du navigateur)
//!   vers `/internal` same-origin. Aucun en-tete d'auth a poser : le cookie de session
//!   part avec la requete, `fetch` etant en `credentials: "same-origin"`.
//!
//! Les composants ignorent le transport : ils appellent les mêmes méthodes sur
//! le `ApiClient` obtenu via [`ApiClient::from_context`].

use lj_dtos::{
    AnnonceCatalogueResponse, AnnonceDetailResponse, AnnuaireStatsResponse, ArticleSearchResponse,
    CitingDecisionHit, CoCitedArticle, CodeCatalogueResponse, CodeTocResponse, CorpusStatsResponse,
    DecisionDetail, DecisionPartiesResponse, EntityDecisionsResponse, EntityDirectoryResponse,
    EntityPageResponse, EntityRegistreResponse, EntitySearchResponse, JournalSearchRequest,
    JournalSearchResponse, JurisdictionCatalogueResponse, JurisdictionHubResponse,
    JurisdictionYearResponse, LawArticleResponse, LawCodeSummary, LawCompareResponse,
    LawSectionResponse, NormCatalogueResponse, NormFondResponse, NormYearResponse,
    OpenSourceResponse, PlaceCorpusResponse, PlacesResponse, PublicationNotice, ReaderJurisdiction,
    SearchContext, SearchRequest, SearchResponse, SimilarDecisionsResponse, SuggestResponse,
};
// Types portés uniquement par les routes client (`/me`, hover cards).
#[cfg(feature = "hydrate")]
use lj_dtos::{AccountSummary, DecisionPreview, UserProfile};

/// Fenetre de pagination offset-based des listes « Mon activite ».
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
    /// The places read from: what applies in any of them is served, the default
    /// reader's when there is none.
    pub places: &'a [String],
    /// Orders or type paths (`FR.DECRET`), any of them served.
    pub jurisdictions: &'a [String],
    pub source: Option<&'a str>,
    /// A category of `lj_vocab::NORM_CATEGORIES`.
    pub scope: Option<&'a str>,
    pub valid_from: Option<&'a str>,
    pub as_of: Option<&'a str>,
}

/// What the API did not find, named by the call that asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Missing {
    Decision,
    Article,
    Places,
    Corpus,
    Version,
    Code,
    Consultation,
    Notice,
    Section,
    Entity,
}

/// Why a call to the API failed, worded where it is shown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Failure {
    Missing(Missing),
    RateLimited,
    InvalidId,
    InvalidEntity,
    /// Worded already: by the page, in the reader's language, or by the service.
    Said(String),
}

impl Failure {
    pub fn text(&self, ui: &crate::i18n::ShellUi) -> String {
        match self {
            Failure::Missing(missing) => (ui.missing)(*missing).to_string(),
            Failure::RateLimited => ui.rate_limited.to_string(),
            Failure::InvalidId => ui.invalid_id.to_string(),
            Failure::InvalidEntity => ui.invalid_entity.to_string(),
            Failure::Said(text) => text.clone(),
        }
    }
}

/// A call to the API that failed.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub failure: Failure,
    pub status: u16,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} (HTTP {})", self.failure, self.status)
    }
}

impl std::error::Error for ApiError {}

/// Why a search failed. The 429 and the 422 have their own words, being the
/// failures the reader can resolve.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchFailure {
    RateLimited,
    /// The query's length in characters, past [`lj_dtos::QUERY_MAX_CHARS`].
    TooLong(usize),
    UnknownFilter,
    Down(Failure),
}

impl SearchFailure {
    /// Takes the query because a 422 has two causes calling for opposite
    /// gestures, and the refusal body is not carried this far.
    pub fn from_error(err: &ApiError, query: &str) -> Self {
        let chars = query.chars().count();
        match err.status {
            429 => Self::RateLimited,
            422 if chars > lj_dtos::QUERY_MAX_CHARS => Self::TooLong(chars),
            422 => Self::UnknownFilter,
            _ => Self::Down(err.failure.clone()),
        }
    }

    /// Its title, then what to do about it.
    pub fn words(&self, ui: &crate::i18n::Ui) -> (String, &'static str) {
        let search = &ui.search;
        match self {
            Self::RateLimited => (search.rate_limited.to_string(), search.rate_limited_hint),
            Self::TooLong(chars) => (
                (search.too_long)(*chars, lj_dtos::QUERY_MAX_CHARS),
                search.too_long_hint,
            ),
            Self::UnknownFilter => (
                search.unknown_filter.to_string(),
                search.unknown_filter_hint,
            ),
            Self::Down(Failure::Said(_)) => (search.down.to_string(), search.down_hint),
            Self::Down(failure) => (failure.text(&ui.shell), search.down_hint),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ════════════════════════════ SSR : services in-process ════════════════════════════

/// What the SSR render asks of the service layer. `lj-server` implements it over
/// `lj-api`, so a change in the backend crates does not rebuild the front.
#[cfg(feature = "ssr")]
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// Jamais atteinte au rendu de `/decisions` : la page y peint son squelette
    /// (`search_is_loading` vrai côté serveur, la ressource n'est donc pas lue) et
    /// la recherche part du client à l'hydratation.
    async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse>;

    /// Décompte et facettes de la même recherche, second appel de la page.
    async fn search_count(
        &self,
        request: &SearchRequest,
    ) -> ApiResult<lj_dtos::SearchCountResponse>;

    /// What the « Textes cités » panel input names in the catalogue. Never
    /// reached at SSR: the input starts empty, and the fetch leaves the wasm.
    async fn search_instruments(
        &self,
        request: &SearchRequest,
        needle: &str,
    ) -> ApiResult<lj_dtos::LegalInstrumentSuggestions>;

    /// Suggestions multi-mots de la barre de recherche (in-process, jamais appelée
    /// au rendu SSR : le fetch part d'un Effect client).
    async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse>;

    /// The decision `id`, its text picked for the reader's language `lang` first.
    async fn fetch_decision(&self, id: &str, lang: Option<&str>) -> ApiResult<DecisionDetail>;

    /// `lang` is the reader's, the language the summaries are served in when held.
    async fn fetch_similar_decisions(
        &self,
        id: &str,
        lang: &str,
    ) -> ApiResult<SimilarDecisionsResponse>;

    /// Article LEGI à une date (`date = Some`) ou en vigueur (`date = None`),
    /// avec sa timeline. Slug inconnu / article absent ⇒ 404.
    async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawArticleResponse>;

    /// The place vocabulary a reader picks from.
    async fn fetch_places(&self) -> ApiResult<PlacesResponse>;

    /// The jurisdictions a reader can hold to, France first.
    async fn fetch_reader_jurisdictions(&self) -> ApiResult<Vec<ReaderJurisdiction>>;

    /// What applies in a place at a date, grouped by journal.
    async fn fetch_place_corpus(
        &self,
        place: Option<&str>,
        date: Option<&str>,
    ) -> ApiResult<PlaceCorpusResponse>;

    /// Comparaison de deux versions d'un article. Bornes = dates ISO de fenêtre de
    /// version (`initiale` = borne ouverte).
    async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
        juris: &str,
    ) -> ApiResult<LawCompareResponse>;

    #[allow(clippy::too_many_arguments)]
    async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
        page: PageParams,
        lang: &str,
    ) -> ApiResult<Vec<CitingDecisionHit>>;

    /// Articles co-cités avec l'article (« souvent cité avec »).
    async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
        juris: &str,
    ) -> ApiResult<Vec<CoCitedArticle>>;

    async fn fetch_legi_code_summary(&self, code: &str, juris: &str) -> ApiResult<LawCodeSummary>;

    /// Recherche plein-texte d'articles (page `/textes`). `code` borne la recherche
    /// à un texte ; `jurisdiction`/`nature`/`source` filtrent le corpus. Chaque
    /// filtre `None`/vide est ignoré côté service. Le `context` qualifie le signal
    /// d'usage : un `teaser` reste un fetch machine.
    async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        mode: lj_dtos::SearchMode,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse>;

    /// Total and facets of the norms search the same arguments name, asked
    /// beside its hits: the server counts the answer that served them.
    async fn search_textes_count(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        mode: lj_dtos::SearchMode,
        context: SearchContext,
    ) -> ApiResult<lj_dtos::ArticleSearchCountResponse>;

    /// Catalogue du fond codes (`/normes/codes`) : codes et constitutions
    /// (`scope=head` de l'API). Servi depuis le cache process-local 12 h.
    async fn fetch_codes_catalogue(&self) -> ApiResult<CodeCatalogueResponse>;

    /// Catalogue des juridictions groupé par famille. Servi depuis le cache
    /// process-local 12 h.
    async fn fetch_juridictions(&self) -> ApiResult<JurisdictionCatalogueResponse>;

    /// The third-party components of the published code, with their licences.
    async fn fetch_open_source(&self) -> ApiResult<OpenSourceResponse>;

    /// Hub d'une juridiction : années couvertes + volume.
    async fn fetch_juridiction_hub(&self, code: &str) -> ApiResult<JurisdictionHubResponse>;

    /// Page paginée des décisions d'une juridiction×année.
    async fn fetch_juridiction_year(
        &self,
        code: &str,
        year: i32,
        page: u32,
    ) -> ApiResult<JurisdictionYearResponse>;

    /// Catalogue des normes par fond. Servi depuis le cache process-local 12 h.
    async fn fetch_normes(&self) -> ApiResult<NormCatalogueResponse>;

    /// Hub d'un fond de normes : années couvertes + volume.
    async fn fetch_norm_fond(&self, juris: &str, fond: &str) -> ApiResult<NormFondResponse>;

    /// Page paginée des textes d'un fond×année (`year = None` = bucket sans date).
    async fn fetch_norm_year(
        &self,
        juris: &str,
        fond: &str,
        year: Option<i32>,
        page: u32,
    ) -> ApiResult<NormYearResponse>;

    /// Page du moteur d'annonces (fond `marche`).
    async fn search_journals(
        &self,
        request: &JournalSearchRequest,
    ) -> ApiResult<JournalSearchResponse>;

    /// Fiche d'une consultation (`/marche/{uid}`). Uid inconnu ⇒ 404.
    async fn fetch_annonce(&self, uid: &str) -> ApiResult<AnnonceDetailResponse>;

    /// What a journal printed, read as a notice (`/notice/{juris}/{journal}:{text_uid}`). Unknown uid ⇒ 404.
    async fn fetch_publication_notice(&self, uid: &str) -> ApiResult<PublicationNotice>;

    /// Catalogue du fond annonces (stock ouvert par département et nature).
    async fn fetch_journals_catalogue(&self) -> ApiResult<AnnonceCatalogueResponse>;

    /// Compteurs globaux du corpus (page d'accueil), lus dans l'instantané
    /// `corpus_counts` : trois lignes indexées par rendu.
    async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse>;

    /// Table des matières d'un code (`/norm/{code}/sommaire`). Slug inconnu ⇒ 404.
    async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<CodeTocResponse>;

    /// Vue-lecture d'une section (`/norm/{code}/section/{path}`).
    async fn fetch_law_section(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawSectionResponse>;

    /// Décisions citant une division (`…/section-citing/{path}`) : union des
    /// citantes de son sous-arbre à la date demandée.
    async fn fetch_law_section_citing(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
        page: PageParams,
        lang: &str,
    ) -> ApiResult<Vec<CitingDecisionHit>>;

    // ── Fiche entité (`/entite`) ────────────────────────────────────────────
    /// Identité registre + agrégats contentieux d'une entité. Uid inconnu ⇒ 404.
    async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse>;

    /// Décisions citant l'entité, paginées (plus récentes d'abord).
    async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse>;

    /// Volet registre de l'entité (APIs publiques à l'affichage).
    async fn fetch_entity_registre(&self, ns: &str, id: &str) -> ApiResult<EntityRegistreResponse>;

    /// Acteurs (parties + conseils) extraits d'une décision (encart « Parties »).
    async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse>;

    // ── Annuaire des entités (`/annuaire`) ───────────────────────────────────
    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse>;

    /// Recherche d'entités (annuaire). `kind` (slug de catégorie) borne si fourni.
    async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse>;

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    async fn fetch_entities_directory(
        &self,
        kind: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse>;

    /// The bearer's account summary, read from the session cookie of the request.
    async fn account_summary(
        &self,
        parts: &axum::http::request::Parts,
    ) -> Option<lj_dtos::AccountSummary>;

    /// The registered name of an OAuth client.
    async fn oauth_client_name(&self, client_id: &str) -> Option<String>;
}

/// The SSR client, provided to the Leptos context of every request by `lj-server`.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ApiClient(std::sync::Arc<dyn Backend>);

#[cfg(feature = "ssr")]
impl ApiClient {
    pub fn new(backend: impl Backend + 'static) -> Self {
        Self(std::sync::Arc::new(backend))
    }

    pub fn from_context() -> Self {
        leptos::prelude::use_context::<Self>()
            .expect("ApiClient provided to the SSR context by lj-server")
    }
}

#[cfg(feature = "ssr")]
impl std::ops::Deref for ApiClient {
    type Target = dyn Backend;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

// ════════════════════════════ Hydrate : reqwest /internal same-origin ════════════════════

#[cfg(feature = "hydrate")]
use reqwest::{Client, Method, RequestBuilder, StatusCode};

#[cfg(feature = "hydrate")]
impl ApiError {
    fn new(failure: Failure, status: StatusCode) -> Self {
        Self {
            failure,
            status: status.as_u16(),
        }
    }

    /// Erreur reseau (fetch/transport echoue avant toute reponse HTTP). Status 0.
    fn transport(err: reqwest::Error) -> Self {
        Self {
            failure: Failure::Said(err.to_string()),
            status: 0,
        }
    }
}

/// Délai au bout duquel une requête `/internal` du navigateur est abandonnée.
///
/// Calé au-dessus du budget que le serveur s'accorde à lui-même
/// (`statement_timeout=30s`) : une réponse lente mais légitime arrive avant,
/// seule une réponse qui n'arrivera jamais est coupée.
#[cfg(feature = "hydrate")]
const REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// Client hydrate : `reqwest` vers `/internal` (route same-origin par `lj-server`).
#[cfg(feature = "hydrate")]
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: Client,
}

#[cfg(feature = "hydrate")]
impl ApiClient {
    /// Client navigateur : base `{origine}/internal` same-origin, backend `fetch`.
    /// L'URL doit être ABSOLUE : reqwest-wasm parse l'URL via `url::Url` qui
    /// rejette un chemin relatif (`/internal/…`) → "builder error". On préfixe
    /// donc l'origine de la page (`lj-server` sert `/internal` en same-origin).
    pub fn from_context() -> Self {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        Self {
            base_url: format!("{origin}/internal"),
            http: Client::new(),
        }
    }

    /// Construit une requete. La session voyage seule : le cookie `HttpOnly` part
    /// avec toute requete same-origin.
    ///
    /// Toute requête porte [`REQUEST_DEADLINE`] : les pages montent leurs
    /// ressources dans un `<Suspense>` sans `<Transition>`, donc une réponse qui
    /// n'arrive jamais (connexion HTTP/2 morte en silence, onglet réveillé, bascule
    /// de réseau) laisse la zone de contenu vide indéfiniment. Le délai transforme
    /// ce gel en `ApiError` que les pages rendent déjà.
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .timeout(REQUEST_DEADLINE)
    }

    /// Envoie une requete sans corps, mappant `!ok` -> `ApiError(message)`.
    /// `not_found` permet le message specifique des routes decision.
    async fn send_unit(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<Missing>,
    ) -> ApiResult<()> {
        let resp = self.envoyer(builder).await?;
        check_status(&resp, message, not_found)?;
        Ok(())
    }

    /// Envoie une requete et desserialise le corps JSON en `T`.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<Missing>,
    ) -> ApiResult<T> {
        let resp = self.envoyer(builder).await?;
        check_status(&resp, message, not_found)?;
        resp.json::<T>().await.map_err(ApiError::transport)
    }

    async fn envoyer(&self, builder: RequestBuilder) -> ApiResult<reqwest::Response> {
        // `build` then `execute`: the deadline set above rides on the request, so
        // the split keeps it, and the method stays readable after the send.
        let requete = builder.build().map_err(ApiError::transport)?;
        let is_get = requete.method() == Method::GET;
        let is_search = requete.url().path().starts_with("/internal/search");
        let resp = self
            .http
            .execute(requete)
            .await
            .map_err(ApiError::transport)?;
        note_build(&resp);
        if is_get {
            recover_from_challenge(&resp);
            if !is_search {
                offer_sign_in(&resp);
            }
        }
        Ok(resp)
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
        let builder = self.request(Method::GET, &format!("/search?{query}"));
        self.send_json(builder, "search failed", None).await
    }

    /// Décompte et facettes de la même recherche, lancé en parallèle d'elle : la
    /// route les retrouve depuis les mêmes paramètres.
    pub async fn search_count(
        &self,
        request: &SearchRequest,
    ) -> ApiResult<lj_dtos::SearchCountResponse> {
        let query = build_search_query(request);
        let builder = self.request(Method::GET, &format!("/search/decompte?{query}"));
        self.send_json(builder, "search count failed", None).await
    }

    /// What the « Textes cités » panel input names in the catalogue, counted on
    /// the same search: the facet only serves the head of its vocabulary.
    pub async fn search_instruments(
        &self,
        request: &SearchRequest,
        needle: &str,
    ) -> ApiResult<lj_dtos::LegalInstrumentSuggestions> {
        let query = build_search_query(request);
        let needle = encode_pairs(&[("needle".to_string(), needle.to_string())]);
        let builder = self.request(
            Method::GET,
            &format!("/search/instruments?{query}&{needle}"),
        );
        self.send_json(builder, "instrument suggest failed", None)
            .await
    }

    // ── Decision ───────────────────────────────────────────────────────────

    pub async fn fetch_decision(&self, id: &str, lang: Option<&str>) -> ApiResult<DecisionDetail> {
        let query = lang
            .map(|l| format!("?lang={}", url_encode(l)))
            .unwrap_or_default();
        let builder = self.request(Method::GET, &format!("/decision/{id}{query}"));
        self.send_json(builder, "decision fetch failed", Some(Missing::Decision))
            .await
    }

    pub async fn fetch_similar_decisions(
        &self,
        id: &str,
        lang: &str,
    ) -> ApiResult<SimilarDecisionsResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/decision/{id}/similar?lang={}", url_encode(lang)),
        );
        self.send_json(
            builder,
            "similar decisions fetch failed",
            Some(Missing::Decision),
        )
        .await
    }

    /// Prévisualisation légère d'une décision (hover card). Client-only : le survol
    /// n'existe qu'au navigateur.
    pub async fn fetch_decision_preview(&self, id: &str, lang: &str) -> ApiResult<DecisionPreview> {
        let builder = self.request(
            Method::GET,
            &format!("/decision/{id}/preview?lang={}", url_encode(lang)),
        );
        self.send_json(
            builder,
            "decision preview fetch failed",
            Some(Missing::Decision),
        )
        .await
    }

    // ── Référentiel LEGI (`/texte`) ───────────────────────────────────────────

    pub async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawArticleResponse> {
        let text = lj_vocab::norm_path(juris, code);
        let mut path = match date {
            Some(date) => format!("{text}/{num}/{date}"),
            None => format!("{text}/{num}"),
        };
        if let Some(place) = place {
            path.push_str(&format!("?place={place}"));
        }
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "legi article fetch failed", Some(Missing::Article))
            .await
    }

    pub async fn fetch_places(&self) -> ApiResult<PlacesResponse> {
        let builder = self.request(Method::GET, "/places");
        self.send_json(builder, "places fetch failed", Some(Missing::Places))
            .await
    }

    pub async fn fetch_reader_jurisdictions(&self) -> ApiResult<Vec<ReaderJurisdiction>> {
        let builder = self.request(Method::GET, "/jurisdictions");
        self.send_json(builder, "jurisdictions fetch failed", None)
            .await
    }

    pub async fn fetch_place_corpus(
        &self,
        place: Option<&str>,
        date: Option<&str>,
    ) -> ApiResult<PlaceCorpusResponse> {
        let mut path = match date {
            Some(d) => format!("/corpus/{d}"),
            None => "/corpus".to_string(),
        };
        if let Some(place) = place {
            path.push_str(&format!("?place={place}"));
        }
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "corpus fetch failed", Some(Missing::Corpus))
            .await
    }

    /// Comparaison de deux versions d'un article.
    pub async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
        juris: &str,
    ) -> ApiResult<LawCompareResponse> {
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(Method::GET, &format!("{text}/{num}/compare/{de}/{a}"));
        self.send_json(builder, "legi compare fetch failed", Some(Missing::Version))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        place: Option<&str>,
        juris: &str,
        page: PageParams,
        lang: &str,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        let date_q = date.map(|d| format!("&date={d}")).unwrap_or_default();
        let place_q = place.map(|p| format!("&place={p}")).unwrap_or_default();
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(
            Method::GET,
            &format!(
                "{text}/{num}/citing{}{date_q}{place_q}&lang={}",
                page_query(page),
                url_encode(lang)
            ),
        );
        self.send_json(builder, "legi citing fetch failed", Some(Missing::Article))
            .await
    }

    /// Articles co-cités avec l'article (« souvent cité avec »).
    pub async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
        juris: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(Method::GET, &format!("{text}/{num}/related"));
        self.send_json(builder, "legi related fetch failed", Some(Missing::Article))
            .await
    }

    pub async fn fetch_legi_code_summary(
        &self,
        code: &str,
        juris: &str,
    ) -> ApiResult<LawCodeSummary> {
        let builder = self.request(Method::GET, &lj_vocab::norm_path(juris, code));
        self.send_json(builder, "legi code fetch failed", Some(Missing::Code))
            .await
    }

    /// Recherche plein-texte d'articles (page `/textes`).
    pub async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        mode: lj_dtos::SearchMode,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        let query = textes_query(q, filters, mode, context);
        let path = format!(
            "/search-textes?{query}&limit={}&offset={}",
            page.limit, page.offset
        );
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "search textes failed", None).await
    }

    pub async fn search_textes_count(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        mode: lj_dtos::SearchMode,
        context: SearchContext,
    ) -> ApiResult<lj_dtos::ArticleSearchCountResponse> {
        let query = textes_query(q, filters, mode, context);
        let builder = self.request(Method::GET, &format!("/search-textes/decompte?{query}"));
        self.send_json(builder, "search textes count failed", None)
            .await
    }

    /// Catalogue du fond codes (`/normes/codes`) : codes et constitutions
    /// (`scope=head` de l'API).
    pub async fn fetch_codes_catalogue(&self) -> ApiResult<CodeCatalogueResponse> {
        let builder = self.request(Method::GET, "/codes?scope=head");
        self.send_json(builder, "codes catalogue fetch failed", None)
            .await
    }

    /// Catalogue des juridictions groupé par famille.
    pub async fn fetch_juridictions(&self) -> ApiResult<JurisdictionCatalogueResponse> {
        let builder = self.request(Method::GET, "/juridictions");
        self.send_json(builder, "juridictions catalogue fetch failed", None)
            .await
    }

    pub async fn fetch_open_source(&self) -> ApiResult<OpenSourceResponse> {
        let builder = self.request(Method::GET, "/open-source");
        self.send_json(builder, "open source fetch failed", None)
            .await
    }

    /// Hub d'une juridiction : années couvertes + volume.
    pub async fn fetch_juridiction_hub(&self, code: &str) -> ApiResult<JurisdictionHubResponse> {
        let path = format!("/juridictions/{code}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "juridiction hub fetch failed", None)
            .await
    }

    /// Page paginée des décisions d'une juridiction×année.
    pub async fn fetch_juridiction_year(
        &self,
        code: &str,
        year: i32,
        page: u32,
    ) -> ApiResult<JurisdictionYearResponse> {
        let path = format!("/juridictions/{code}/{year}?page={page}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "juridiction year fetch failed", None)
            .await
    }

    /// Catalogue des normes par fond.
    pub async fn fetch_normes(&self) -> ApiResult<NormCatalogueResponse> {
        let builder = self.request(Method::GET, "/normes");
        self.send_json(builder, "normes catalogue fetch failed", None)
            .await
    }

    /// Hub d'un fond de normes : années couvertes + volume.
    pub async fn fetch_norm_fond(&self, juris: &str, fond: &str) -> ApiResult<NormFondResponse> {
        let path = format!("/normes/{juris}/{fond}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "norm fond fetch failed", None)
            .await
    }

    /// Page paginée des textes d'un fond×année (`year = None` = bucket sans date).
    pub async fn fetch_norm_year(
        &self,
        juris: &str,
        fond: &str,
        year: Option<i32>,
        page: u32,
    ) -> ApiResult<NormYearResponse> {
        let annee = match year {
            Some(y) => y.to_string(),
            None => "sans-date".to_string(),
        };
        let path = format!("/normes/{juris}/{fond}/{annee}?page={page}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "norm year fetch failed", None)
            .await
    }

    /// Page du moteur d'annonces.
    pub async fn search_journals(
        &self,
        request: &JournalSearchRequest,
    ) -> ApiResult<JournalSearchResponse> {
        let path = format!("/journals?{}", build_journals_query(request));
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "annonces search failed", None)
            .await
    }

    /// Fiche d'une consultation (`/marche/{uid}`).
    pub async fn fetch_annonce(&self, uid: &str) -> ApiResult<AnnonceDetailResponse> {
        let path = format!("/journals/{}", url_encode(uid));
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "annonce fetch failed", Some(Missing::Consultation))
            .await
    }

    /// What a journal printed, read as a notice.
    pub async fn fetch_publication_notice(&self, uid: &str) -> ApiResult<PublicationNotice> {
        let path = format!("/notices/{}", url_encode(uid));
        let builder = self.request(Method::GET, &path);
        self.send_json(
            builder,
            "publication notice fetch failed",
            Some(Missing::Notice),
        )
        .await
    }

    /// Catalogue du fond annonces.
    pub async fn fetch_journals_catalogue(&self) -> ApiResult<AnnonceCatalogueResponse> {
        let builder = self.request(Method::GET, "/journals/catalogue");
        self.send_json(builder, "annonces catalogue fetch failed", None)
            .await
    }

    /// Compteurs globaux du corpus (page d'accueil).
    pub async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        let builder = self.request(Method::GET, "/corpus-stats");
        self.send_json(builder, "corpus stats fetch failed", None)
            .await
    }

    /// Table des matières d'un code (`…/sommaire`). Slug inconnu ⇒ 404.
    pub async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<CodeTocResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(Method::GET, &format!("{text}/sommaire{date_q}"));
        self.send_json(builder, "code toc fetch failed", Some(Missing::Code))
            .await
    }

    /// Vue-lecture d'une section (`…/section/{path}`).
    pub async fn fetch_law_section(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
    ) -> ApiResult<LawSectionResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(Method::GET, &format!("{text}/section/{path}{date_q}"));
        self.send_json(builder, "law section fetch failed", Some(Missing::Section))
            .await
    }

    /// Décisions citant une division (`…/section-citing/{path}`).
    pub async fn fetch_law_section_citing(
        &self,
        code: &str,
        path: &str,
        date: Option<&str>,
        juris: &str,
        page: PageParams,
        lang: &str,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        let date_q = date.map(|d| format!("&date={d}")).unwrap_or_default();
        let text = lj_vocab::norm_path(juris, code);
        let builder = self.request(
            Method::GET,
            &format!(
                "{text}/section-citing/{path}{}{date_q}&lang={}",
                page_query(page),
                url_encode(lang)
            ),
        );
        self.send_json(
            builder,
            "law section citing fetch failed",
            Some(Missing::Section),
        )
        .await
    }

    // ── Fiche entité (`/entite`) ────────────────────────────────────────────

    pub async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        let builder = self.request(Method::GET, &format!("/entity/{ns}/{id}"));
        self.send_json(builder, "entity fetch failed", Some(Missing::Entity))
            .await
    }

    pub async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/entity/{ns}/{id}/decisions?page={page}&page_size={page_size}"),
        );
        self.send_json(
            builder,
            "entity decisions fetch failed",
            Some(Missing::Entity),
        )
        .await
    }

    /// Volet registre de l'entité (APIs publiques à l'affichage).
    pub async fn fetch_entity_registre(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<EntityRegistreResponse> {
        let builder = self.request(Method::GET, &format!("/entity/{ns}/{id}/registre"));
        self.send_json(builder, "entity registre fetch failed", None)
            .await
    }

    pub async fn fetch_entity_documents(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<lj_dtos::EntityDocumentsResponse> {
        let builder = self.request(Method::GET, &format!("/entity/{ns}/{id}/documents"));
        self.send_json(builder, "entity documents fetch failed", None)
            .await
    }

    pub async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        let builder = self.request(Method::GET, &format!("/decision/{id}/parties"));
        self.send_json(
            builder,
            "decision parties fetch failed",
            Some(Missing::Decision),
        )
        .await
    }

    // ── Autocomplétion ────────────────────────────────────────────────────────

    /// Suggestions multi-mots de la barre de recherche.
    /// `mode` ∈ `jurisprudence` | `textes` | `annuaire`.
    pub async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        let path = format!("/suggest?q={}&mode={mode}", url_encode(q));
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "suggest fetch failed", None).await
    }

    // ── Annuaire des entités (`/annuaire`) ───────────────────────────────────

    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    pub async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        let builder = self.request(Method::GET, "/entities/stats");
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
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "entities search failed", None)
            .await
    }

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    pub async fn fetch_entities_directory(
        &self,
        kind: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let path = format!(
            "/entities/directory?kind={}&page={page}&page_size={page_size}",
            url_encode(kind)
        );
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "entities directory fetch failed", None)
            .await
    }

    // ── Compte ─────────────────────────────────────────────────────────────

    pub async fn fetch_me(&self) -> ApiResult<UserProfile> {
        let builder = self.request(Method::GET, "/me");
        self.send_json(builder, "me fetch failed", None).await
    }

    /// Déconnexion de CE navigateur. Sans effet sur les autorisations MCP :
    /// elles vivent dans nos tables de jetons, cycle de vie distinct de la
    /// session web.
    pub async fn logout(&self) -> ApiResult<()> {
        let builder = self.request(Method::POST, "/auth/logout");
        self.send_unit(builder, "logout failed", None).await
    }

    /// Résumé du compte pour `/profile` en navigation SPA — le premier rendu le
    /// lit côté serveur, sans passer ici.
    pub async fn fetch_account(&self) -> ApiResult<AccountSummary> {
        let builder = self.request(Method::GET, "/account");
        self.send_json(builder, "account fetch failed", None).await
    }

    // ── Clés de la surface publique ──────────────────────────────────────────

    pub async fn list_api_keys(&self) -> ApiResult<lj_dtos::ApiKeysResponse> {
        let builder = self.request(Method::GET, "/me/api-keys");
        self.send_json(builder, "api keys fetch failed", None).await
    }

    /// Le secret ne revient que là : il n'est pas stocké en clair, donc la liste
    /// ne le resservira pas.
    pub async fn create_api_key(&self, label: &str) -> ApiResult<lj_dtos::ApiKeyCreated> {
        let builder = self
            .request(Method::POST, "/me/api-keys")
            .json(&lj_dtos::ApiKeyCreate {
                label: label.to_string(),
            });
        self.send_json(builder, "api key create failed", None).await
    }

    pub async fn revoke_api_key(&self, id: &str) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, &format!("/me/api-keys/{id}"));
        self.send_unit(builder, "api key revoke failed", None).await
    }

    pub async fn report_read(&self, report: &lj_dtos::ReadReport) -> ApiResult<()> {
        let builder = self.request(Method::POST, "/reads").json(report);
        self.send_unit(builder, "read report failed", None).await
    }

    pub async fn report_landing(&self, report: &lj_dtos::LandingReport) -> ApiResult<()> {
        let builder = self.request(Method::POST, "/landings").json(report);
        self.send_unit(builder, "landing report failed", None).await
    }

    pub async fn report_entity_search(
        &self,
        report: &lj_dtos::EntitySearchReport,
    ) -> ApiResult<()> {
        let builder = self.request(Method::POST, "/entity-searches").json(report);
        self.send_unit(builder, "entity search report failed", None)
            .await
    }
}

/// Key holding the date of the last reload spent in this tab.
#[cfg(feature = "hydrate")]
const CHALLENGE_RELOAD: &str = "lj-defi-recharge";

/// Separates the two populations rather than capping a count: a client the edge
/// never lets through comes back within seconds, where a clearance that expires
/// honestly is a `challenge_ttl` away, 24 h. Any value between the two holds.
#[cfg(feature = "hydrate")]
const CHALLENGE_RELOAD_COOLDOWN_MS: f64 = 60_000.0;

/// A 403 here can only come from the edge: the API returns none, on any route. An
/// edge challenge needs a **document** response to be solved, which a `fetch`
/// cannot show, so a GET navigation to the same page hands the browser one.
#[cfg(feature = "hydrate")]
fn recover_from_challenge(resp: &reqwest::Response) {
    if resp.status() != StatusCode::FORBIDDEN {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    let now = js_sys::Date::now();
    let last = storage
        .get_item(CHALLENGE_RELOAD)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f64>().ok());
    if last.is_some_and(|last| now - last < CHALLENGE_RELOAD_COOLDOWN_MS) {
        return;
    }
    let location = window.location();
    let (Ok(path), Ok(query)) = (location.pathname(), location.search()) else {
        return;
    };
    let _ = storage.set_item(CHALLENGE_RELOAD, &now.to_string());
    // `reload()` repeats the document's method, and a page the challenge form served
    // is a POST the origin answers 405. A fragment would turn this into a scroll.
    let _ = location.replace(&format!("{path}{query}"));
}

/// A refused read: the head script's bar offers to sign in, an account reading on its
/// own budget. A refused search says so in its own results.
#[cfg(feature = "hydrate")]
fn offer_sign_in(resp: &reqwest::Response) {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    if resp.status() != StatusCode::TOO_MANY_REQUESTS {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(bar) = js_sys::Reflect::get(&window, &JsValue::from_str("__ljLimit")) else {
        return;
    };
    if let Ok(bar) = bar.dyn_into::<js_sys::Function>() {
        let _ = bar.call0(&window);
    }
}

/// Transmet au suivi de l'onglet l'identite du bundle portee par la reponse. Le
/// suivi lui-meme vit dans le script de tete (`app::BANDEAU`), pose avant
/// l'hydratation : une seule memoire par onglet, une seule annonce.
#[cfg(feature = "hydrate")]
fn note_build(resp: &reqwest::Response) {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    let Some(id) = resp
        .headers()
        .get("x-lj-front-build")
        .and_then(|v| v.to_str().ok())
    else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(suivi) = js_sys::Reflect::get(&window, &JsValue::from_str("__ljBuild")) else {
        return;
    };
    if let Ok(suivi) = suivi.dyn_into::<js_sys::Function>() {
        let _ = suivi.call1(&window, &JsValue::from_str(id));
    }
}

/// Maps `404` to `not_found` when given, `429` to [`Failure::RateLimited`], any other failure to `message`.
#[cfg(feature = "hydrate")]
fn check_status(
    resp: &reqwest::Response,
    message: &str,
    not_found: Option<Missing>,
) -> ApiResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::new(Failure::RateLimited, status));
    }
    if status == StatusCode::NOT_FOUND {
        if let Some(missing) = not_found {
            return Err(ApiError::new(Failure::Missing(missing), status));
        }
    }
    Err(ApiError::new(Failure::Said(message.to_string()), status))
}

/// Query string `?limit=&offset=`.
#[cfg(feature = "hydrate")]
fn page_query(page: PageParams) -> String {
    format!("?limit={}&offset={}", page.limit, page.offset)
}

/// A `SearchRequest` as a query string, under the names of the `search_decisions`
/// arguments, defaults left out.
#[cfg(any(feature = "hydrate", test))]
fn build_search_query(request: &SearchRequest) -> String {
    use lj_dtos::SortOrder;

    let mut params: Vec<(String, String)> = Vec::new();
    params.push(("query".to_string(), request.query.clone()));

    append_multi(&mut params, "jurisdiction_type", &request.jurisdiction_type);
    append_multi(&mut params, "solution", &request.solution);
    append_multi(&mut params, "legal_domain", &request.legal_domain);
    append_multi_str(&mut params, "jurisdiction_code", &request.jurisdiction_code);
    append_multi_str(&mut params, "seat", &request.seat);
    if let Some(home) = &request.home {
        params.push(("home".to_string(), home.clone()));
    }
    append_multi_str(&mut params, "formation", &request.formation);
    append_multi_str(&mut params, "legal_instrument", &request.legal_instrument);
    append_multi_str(&mut params, "legal_article", &request.legal_article);
    append_multi_str(&mut params, "publication", &request.publication);
    append_multi(&mut params, "search_in", &request.search_in);

    if let Some(date_from) = &request.date_from {
        params.push(("date_from".to_string(), date_from.clone()));
    }
    if let Some(date_to) = &request.date_to {
        params.push(("date_to".to_string(), date_to.clone()));
    }
    // `mode` toujours emis : le DTO le materialise (defaut Auto). L'API resout
    // Auto cote serveur.
    params.push(("mode".to_string(), enum_value(&request.mode)));
    if !matches!(request.sort, SortOrder::Relevance) {
        params.push(("sort".to_string(), enum_value(&request.sort)));
    }
    params.push(("limit".to_string(), request.limit.to_string()));
    if request.offset != 0 {
        params.push(("offset".to_string(), request.offset.to_string()));
    }
    if request.ai_mode {
        params.push(("ai_rerank".to_string(), "true".to_string()));
    }
    if let Some(lang) = &request.lang {
        params.push(("lang".to_string(), lang.clone()));
    }

    encode_pairs(&params)
}

/// A `JournalSearchRequest` as a query string: the keys of the `/journal-search`
/// page's URL, a list as a repeated parameter.
#[cfg(feature = "hydrate")]
fn build_journals_query(request: &JournalSearchRequest) -> String {
    let mut params: Vec<(String, String)> = Vec::new();
    if let Some(q) = request.q.as_deref().filter(|q| !q.is_empty()) {
        params.push(("q".to_string(), q.to_string()));
    }
    for (key, values) in [
        ("fond", &request.fond),
        ("zone", &request.zone),
        ("famille", &request.famille),
        ("cpv", &request.cpv),
    ] {
        for v in values {
            params.push((key.to_string(), v.clone()));
        }
    }
    if let Some(j) = request.echeance_jours {
        params.push(("echeanceJours".to_string(), j.to_string()));
    }
    if request.closes {
        params.push(("closes".to_string(), "true".to_string()));
    }
    for (key, value) in [
        ("jurisdiction", &request.jurisdiction),
        ("home", &request.home),
    ] {
        if let Some(v) = value {
            params.push((key.to_string(), v.clone()));
        }
    }
    params.push(("sort".to_string(), enum_value(&request.sort)));
    params.push(("limit".to_string(), request.limit.to_string()));
    if request.offset != 0 {
        params.push(("offset".to_string(), request.offset.to_string()));
    }
    encode_pairs(&params)
}

/// The query string a norms search and its count share.
#[cfg(feature = "hydrate")]
fn textes_query(
    q: &str,
    filters: TextesFilters<'_>,
    mode: lj_dtos::SearchMode,
    context: SearchContext,
) -> String {
    let mut query = format!("query={}&mode={}", url_encode(q), enum_value(&mode));
    if context == SearchContext::Teaser {
        query.push_str("&context=teaser");
    }
    for (key, values) in [
        ("place", filters.places),
        ("jurisdiction", filters.jurisdictions),
    ] {
        for v in values {
            query.push_str(&format!("&{key}={}", url_encode(v)));
        }
    }
    for (key, value) in [
        ("code", filters.code),
        ("source", filters.source),
        ("scope", filters.scope),
        ("valid_from", filters.valid_from),
        ("as_of", filters.as_of),
    ] {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            query.push_str(&format!("&{key}={}", url_encode(v)));
        }
    }
    query
}

/// Serialise une valeur enum DTO en sa chaine serde (sans guillemets).
#[cfg(any(feature = "hydrate", test))]
fn enum_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("enum DTO serialise en chaine")
}

/// Ajoute une cle multivaluee a partir d'enums DTO.
#[cfg(any(feature = "hydrate", test))]
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

/// Ajoute une cle multivaluee de chaines.
#[cfg(any(feature = "hydrate", test))]
fn append_multi_str(params: &mut Vec<(String, String)>, key: &str, values: &Option<Vec<String>>) {
    if let Some(values) = values {
        for v in values {
            params.push((key.to_string(), v.clone()));
        }
    }
}

/// Encode une liste de paires en query string (RFC 3986, `URLSearchParams`).
#[cfg(any(feature = "hydrate", test))]
fn encode_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode `application/x-www-form-urlencoded` (espace -> `+`).
#[cfg(any(feature = "hydrate", test))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{Domain, JurisdictionType, SearchMode, Solution, SortOrder};

    fn failed(status: u16) -> ApiError {
        ApiError {
            failure: Failure::Said("search failed".to_string()),
            status,
        }
    }

    /// An invalid filter is not an outage: the 422 names the link, not the
    /// API's availability.
    #[test]
    fn a_422_does_not_blame_the_api() {
        assert_eq!(
            SearchFailure::from_error(&failed(422), "bail commercial"),
            SearchFailure::UnknownFilter
        );
        assert!(matches!(
            SearchFailure::from_error(&failed(500), "bail commercial"),
            SearchFailure::Down(_)
        ));
    }

    /// An over-length 422 carries the length and never speaks of a filter.
    #[test]
    fn over_length_422_names_the_length() {
        let too_long = "x".repeat(lj_dtos::QUERY_MAX_CHARS + 1);
        assert_eq!(
            SearchFailure::from_error(&failed(422), &too_long),
            SearchFailure::TooLong(lj_dtos::QUERY_MAX_CHARS + 1)
        );
    }

    /// Toute clé sérialisée par le `SearchRequest` doit partir dans la query
    /// string : le filtre manquant à cette liste ne dit rien, l'URL montre la
    /// facette cochée et l'API cherche sans elle.
    #[test]
    fn build_search_query_emet_chaque_filtre_du_dto() {
        let request = SearchRequest {
            query: "bail commercial".to_string(),
            jurisdiction_type: Some(vec![JurisdictionType::Cc]),
            solution: Some(vec![Solution::Dismissed]),
            legal_domain: Some(vec![Domain::Social]),
            jurisdiction_code: Some(vec!["cc".to_string()]),
            seat: Some(vec!["FR.CC.C2".to_string()]),
            home: Some("fr".to_string()),
            formation: Some(vec!["FR.CC.RESTREINTE".to_string()]),
            legal_instrument: Some(vec!["code-civil".to_string()]),
            legal_article: Some(vec!["code-civil|203".to_string()]),
            publication: Some(vec!["publie".to_string()]),
            date_from: Some("2020-01-01".to_string()),
            date_to: Some("2026-01-01".to_string()),
            search_in: None,
            mode: SearchMode::Semantic,
            sort: SortOrder::DateDesc,
            limit: 20,
            offset: 20,
            ai_mode: true,
            lang: Some("de".to_string()),
        };

        let qs = build_search_query(&request);
        let emitted: Vec<&str> = qs
            .split('&')
            .filter_map(|pair| pair.split('=').next())
            .collect();
        let serialized = serde_json::to_value(&request).expect("SearchRequest sérialise");
        for field in serialized.as_object().expect("objet JSON").keys() {
            // The DTO is camelCase, the query string the tool's snake_case.
            let key = match field.as_str() {
                "aiMode" => "ai_rerank".to_string(),
                f => f.chars().fold(String::new(), |mut k, c| {
                    if c.is_ascii_uppercase() {
                        k.push('_');
                    }
                    k.push(c.to_ascii_lowercase());
                    k
                }),
            };
            assert!(
                emitted.contains(&key.as_str()),
                "clé `{key}` absente de `{qs}`"
            );
        }
    }
}
