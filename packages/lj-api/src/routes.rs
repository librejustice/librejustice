//! Assemblage du routeur axum (port de `main.create_app`).
//!
//! Monte les mêmes routes que le `create_app` FastAPI, mêmes chemins/méthodes
//! (syntaxe axum 0.8 `/{id}`), même ordre de middleware :
//!
//! 1. CORS (allowlist explicite, ADR 0038) ;
//! 2. compression gzip (`minimum_size=1024`, parité `GZipMiddleware`) ;
//! 3. access-log (trace tower-http — substitut de `_AccessLogMiddleware`).
//!
//! Les handlers de données (`search`, `decision/*`, `me/*`…) sont des fonctions
//! pures `(&AppState, …) -> Result<…>` ; ce module les adapte en handlers axum
//! (extraction des query/path params + alias camelCase, application des headers
//! `Cache-Control`). Les résumés sont servis embarqués dans la recherche et le
//! détail (garantis en base, ADR 0051) — pas d'endpoint de génération à la volée.

use crate::auth::{OptionalUser, RequiredUser};
use crate::cache::{CachePolicy, CACHE_DECISION, CACHE_SEARCH};
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;
use crate::{
    bookmarks, decision_views, decisions, docx_export, legi, mcp, me, oauth, pdf_export, redirect,
    search, search_history, sitemap, stats,
};
use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use lj_dtos::{
    ActivitySource, ActivityTrackingUpdate, ArticleSearchFacets, ArticleSearchResponse,
    BookmarksResponse, DecisionViewsResponse, Domaine, HealthResponse, JuridictionType, Office,
    Portee, SearchHistoryResponse, SearchMode, SearchRequest, SimilarDecisionsResponse, Solution,
    SortOrder, UserProfileUpdate, Voie,
};
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Assemble les routes (data API `/api`, OAuth, discovery `.well-known`, MCP
/// optionnel) avec l'état injecté — sans fallback ni middleware. Brique partagée
/// par [`build_router`] (serveur fusionné, fallback fourni par l'hôte) et
/// [`create_app`] (serveur API autonome).
fn assemble_routes(state: AppState, enable_mcp: bool) -> Router {
    // ── DATA API (préfixe `/api`, ADR 0042) ──────────────────────────────────
    let api = Router::new()
        .route("/health", get(health))
        .route("/search", get(search_endpoint))
        // Recherche plein-texte d'articles (ADR 0114), corpus distinct des
        // décisions : route séparée, jambe BM25 lexicale sur `legal_article`.
        .route("/search-textes", get(search_textes_endpoint))
        // Catalogue des codes navigables (page `/codes`) : un texte de
        // référentiel par entrée, avec son nombre d'articles.
        .route("/codes", get(codes_catalogue))
        // Compteurs globaux du corpus (page d'accueil) : estimation décisions +
        // nombre de codes, servis depuis un cache process-local (TTL 12 h).
        .route("/corpus-stats", get(corpus_stats))
        .route("/decision/{decision_id}", get(decision))
        .route("/decision/{decision_id}/similar", get(decision_similar))
        .route("/decision/{decision_id}/preview", get(decision_preview))
        .route(
            "/decision/{decision_id}/download.docx",
            get(decision_download_docx),
        )
        .route(
            "/decision/{decision_id}/download.pdf",
            get(decision_download_pdf),
        )
        // Référentiel LEGI versionné (`/loi`, ADR 0092). Ordre des routes : la
        // version-à-date `{code}/{num}/{date}` et la sous-ressource
        // `{code}/{num}/citing` ne peuvent collisionner — `citing` est un
        // segment littéral, distinct d'une date ISO.
        .nest("/loi", legi_router())
        // me / bookmarks / search-history / decision-views (RequiredUser).
        .route("/me", get(get_me).patch(patch_me).delete(delete_me))
        .route("/me/activity-tracking", put(set_activity_tracking))
        .route("/me/bookmarks", get(list_bookmarks))
        .route(
            "/me/bookmarks/{decision_id}",
            put(add_bookmark).delete(remove_bookmark),
        )
        .route(
            "/me/search-history",
            get(list_history).delete(clear_history),
        )
        .route("/me/search-history/{entry_id}", delete(delete_entry))
        .route("/me/decision-views", get(list_views).delete(clear_views))
        .route(
            "/me/decision-views/{decision_id}",
            post(record_view).delete(delete_view),
        );

    // ── Racine : OAuth + discovery `.well-known` (consommés hors `/api`) ──────
    // Ces sous-routeurs sont `Router<AppState>` ; on injecte l'état avec
    // `with_state` pour obtenir un `Router<()>` montable par `axum::serve`.
    let mut app: Router = Router::new()
        .nest("/api", api)
        .merge(oauth::router())
        .merge(oauth::well_known_router())
        // Sitemaps servis depuis Postgres (`/sitemap.xml` + `/sitemaps/{file}`),
        // ADR 0064.
        .merge(sitemap::router())
        .with_state(state.clone());

    // ── MCP (gated) ───────────────────────────────────────────────────────────
    // `mcp_router` porte déjà son état et ses chemins absolus `/mcp/` + `/mcp`.
    if enable_mcp {
        app = app.merge(mcp::mcp_router(state.clone()));
    }

    // ── IndexNow (ADR 0044) ───────────────────────────────────────────────────
    // Le protocole exige la clé à `https://<host>/<clé>.txt` : sans elle, les
    // soumissions nocturnes de `lj-ingest indexnow` sont rejetées par les
    // moteurs. Clé publique par protocole, lue des `Settings` (même patron que
    // le challenge OpenAI).
    if let Some(key) = state.settings.indexnow_key.clone() {
        let path = format!("/{key}.txt");
        app = app.route(&path, get(move || std::future::ready(key.clone())));
    }
    app
}

/// Empile le middleware commun (CORS, compression, trace OTel) sur un routeur
/// assemblé.
///
/// L'ordre `.layer` est appliqué de l'extérieur vers l'intérieur : on empile la
/// trace OTel (la plus externe : voit toutes les requêtes) puis compression puis
/// CORS, ce qui reproduit l'ordre Python (CORS le plus externe côté Starlette,
/// mais l'effet observable — gzip + en-têtes CORS — est identique).
///
/// `OtelAxumLayer` ouvre un span serveur par requête, nommé
/// `{http.method} {http.route}` et portant les champs semconv OTel
/// (`http.route`, `http.request.method`, `http.response.status_code`) que le
/// cockpit Grafana filtre côté Tempo. `OtelInResponseLayer` (plus interne)
/// réinjecte le `traceparent` dans la réponse.
fn apply_layers(app: Router, cors: CorsLayer) -> Router {
    // `pg_error_boundary` est la couche la PLUS INTERNE (premier `.layer`) : elle
    // doit réécrire l'erreur PG marquée par le handler avant que CORS/gzip/OTel ne
    // la voient, pour qu'ils s'appliquent au 422 final.
    app.layer(middleware::from_fn(pg_error_boundary))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(OtelAxumLayer::default())
        .layer(OtelInResponseLayer)
}

/// Frontière HTTP qui reclasse en 422 les erreurs Postgres marquées par
/// [`ApiError::into_response`] (extension [`crate::error::PgErrKind`]) — parité des
/// deux `@app.exception_handler` de `main.py` (`psycopg.DataError` → 422
/// `invalid_request_bytes` ; `InternalError_` + `_PARADEDB_PARSE_ERR_RE` → 422
/// `invalid_query_syntax`). Le `path`/`q`, indisponibles au moment du rendu de
/// l'erreur, sont réinjectés ici depuis la requête entrante.
async fn pg_error_boundary(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let raw_query = req.uri().query().unwrap_or("").to_string();
    let resp = next.run(req).await;
    let Some(kind) = resp.extensions().get::<crate::error::PgErrKind>().copied() else {
        return resp;
    };
    let detail = match kind {
        crate::error::PgErrKind::DataException => {
            serde_json::json!({ "error": "invalid_request_bytes", "path": path })
        }
        crate::error::PgErrKind::ParseSyntax => {
            let q = parse_qs(&raw_query)
                .into_iter()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v)
                .unwrap_or_default();
            serde_json::json!({ "error": "invalid_query_syntax", "query": q })
        }
    };
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({ "detail": detail })),
    )
        .into_response()
}

/// Construit le routeur API (routes + middleware CORS/compression/OTel) sans
/// fallback racine. Destiné au serveur fusionné de l'ADR 0061, qui pose son
/// propre catch-all (handler de fichiers Leptos).
pub fn build_router(state: AppState, enable_mcp: bool) -> Router {
    let cors = cors_layer(&state.settings.cors_origins);
    apply_layers(assemble_routes(state, enable_mcp), cors)
}

/// Construit l'app axum complète (routes search/decisions/auth/mcp + fallback de
/// redirection trailing-slash + middleware CORS, access-log, cache headers).
/// `enable_mcp` monte l'endpoint MCP.
pub fn create_app(state: AppState, enable_mcp: bool) -> Router {
    let cors = cors_layer(&state.settings.cors_origins);
    // Fallback racine : redirection trailing-slash 307 (parité Starlette
    // `redirect_slashes=True`) ; 404 sinon. Posé avant les couches.
    let app = assemble_routes(state, enable_mcp).fallback(redirect::slash_redirect_fallback);
    apply_layers(app, cors)
}

/// Couche CORS — allowlist explicite (parité `CORSMiddleware`, ADR 0038).
///
/// `allow_credentials=False`, méthodes `GET/POST/PUT/PATCH/DELETE`, tous headers.
fn cors_layer(origins: &[String]) -> CorsLayer {
    let parsed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| o.parse::<HeaderValue>().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers(Any)
}

/// Pose le header `Cache-Control` sur une réponse JSON (parité `public_cache`).
fn with_cache<T: serde::Serialize>(policy: CachePolicy, body: T) -> Response {
    let mut resp = Json(body).into_response();
    if let Ok(value) = HeaderValue::from_str(&policy.header_value()) {
        resp.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    resp
}

// ── Health ────────────────────────────────────────────────────────────────────

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: state.settings.version.clone(),
    })
}

// ── Search ──────────────────────────────────────────────────────────────────

/// Paramètres de la query `/api/search`, alias camelCase (parité `Query(alias=…)`).
///
/// Construits depuis la query string brute (les listes répétées
/// `?juridictionType=A&juridictionType=B` ne sont pas gérées par
/// `serde_urlencoded`), puis convertis en [`SearchRequest`].
#[derive(Debug)]
struct SearchParams {
    q: Option<String>,
    juridiction_type: Vec<JuridictionType>,
    solution: Vec<Solution>,
    voie: Vec<Voie>,
    office: Vec<Office>,
    legal_domain: Vec<Domaine>,
    jurisdiction_code: Vec<String>,
    legal_instrument: Vec<String>,
    legal_article: Vec<String>,
    portee: Vec<Portee>,
    publication: Vec<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: SearchMode,
    sort: SortOrder,
    limit: u32,
    offset: u32,
    ai_mode: bool,
}

impl Default for SearchParams {
    fn default() -> Self {
        // Défauts Pydantic : mode=auto, sort=relevance, limit=20, offset=0.
        Self {
            q: None,
            juridiction_type: Vec::new(),
            solution: Vec::new(),
            voie: Vec::new(),
            office: Vec::new(),
            legal_domain: Vec::new(),
            jurisdiction_code: Vec::new(),
            legal_instrument: Vec::new(),
            legal_article: Vec::new(),
            portee: Vec::new(),
            publication: Vec::new(),
            date_from: None,
            date_to: None,
            mode: SearchMode::Auto,
            sort: SortOrder::Relevance,
            limit: 20,
            offset: 0,
            ai_mode: false,
        }
    }
}

async fn search_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let req = parse_search_query(raw.as_deref().unwrap_or(""))?;
    let result = search::search(&state, &req).await?;
    if user_id.is_some() {
        search_history::record_search(
            &state.pool,
            user_id.as_deref().unwrap_or(""),
            &req,
            ActivitySource::Web,
        )
        .await;
    }
    Ok(with_cache(CACHE_SEARCH, result))
}

/// Parse la query string `/api/search` → [`SearchRequest`], avec les bornes
/// Pydantic (`q` min/max, `limit` 1–50, `offset` 0–99). Erreur 400 sur entrée
/// invalide (parité `422` Python — la frontière de validation).
fn parse_search_query(raw: &str) -> Result<SearchRequest> {
    let mut p = SearchParams {
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: 20,
        offset: 0,
        ..SearchParams::default()
    };
    for (key, value) in parse_qs(raw) {
        match key.as_str() {
            "q" => p.q = Some(value),
            "juridictionType" => {
                if let Some(v) = de_enum::<JuridictionType>(&value) {
                    p.juridiction_type.push(v);
                }
            }
            "solution" => {
                if let Some(v) = de_enum::<Solution>(&value) {
                    p.solution.push(v);
                }
            }
            "voie" => {
                if let Some(v) = de_enum::<Voie>(&value) {
                    p.voie.push(v);
                }
            }
            "office" => {
                if let Some(v) = de_enum::<Office>(&value) {
                    p.office.push(v);
                }
            }
            "legalDomain" => {
                if let Some(v) = de_enum::<Domaine>(&value) {
                    p.legal_domain.push(v);
                }
            }
            "jurisdictionCode" => p.jurisdiction_code.push(value),
            "legalInstrument" => p.legal_instrument.push(value),
            "legalArticle" => p.legal_article.push(value),
            "portee" => {
                if let Some(v) = de_enum::<Portee>(&value) {
                    p.portee.push(v);
                }
            }
            "publication" => p.publication.push(value),
            "dateFrom" => p.date_from = Some(value),
            "dateTo" => p.date_to = Some(value),
            "mode" => {
                p.mode = de_enum::<SearchMode>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "mode"],
                        &value,
                        &["auto", "lexical", "semantic"],
                    ))
                })?
            }
            "sort" => {
                p.sort = de_enum::<SortOrder>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "sort"],
                        &value,
                        &["relevance", "date_desc", "date_asc"],
                    ))
                })?
            }
            "limit" => {
                p.limit = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &value))
                })?
            }
            "offset" => {
                p.offset = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "offset"], &value))
                })?
            }
            "aiMode" => p.ai_mode = matches!(value.as_str(), "1" | "true" | "yes" | "on"),
            _ => {} // no_cache & inconnus : ignorés (parité extra="ignore").
        }
    }

    // `q` est `Query(..., min_length=1, max_length=512)` côté FastAPI : absent →
    // `missing`, vide → `string_too_short`, trop long → `string_too_long`.
    let Some(query) = p.q else {
        return Err(ApiError::Unprocessable(validation::missing(&[
            "query", "q",
        ])));
    };
    let q_len = query.chars().count() as u64;
    if q_len < 1 {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &query,
            1,
        )));
    }
    if q_len > 512 {
        return Err(ApiError::Unprocessable(validation::string_too_long(
            &["query", "q"],
            &query,
            512,
        )));
    }
    if p.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            1,
        )));
    }
    if p.limit > 50 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            50,
        )));
    }
    if p.offset > 99 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "offset"],
            &p.offset.to_string(),
            99,
        )));
    }
    // `dateFrom`/`dateTo` sont `Optional[date]` bornés côté FastAPI : Pydantic
    // valide format + plage AVANT le SQL. Sans ce garde, une date malformée
    // partait en `::date` cast → 500. Validée à la frontière → 422.
    validate_date_param("dateFrom", &p.date_from)?;
    validate_date_param("dateTo", &p.date_to)?;

    Ok(SearchRequest {
        query,
        juridiction_type: opt_vec(p.juridiction_type),
        solution: opt_vec(p.solution),
        voie: opt_vec(p.voie),
        office: opt_vec(p.office),
        legal_domain: opt_vec(p.legal_domain),
        jurisdiction_code: opt_vec(p.jurisdiction_code),
        legal_instrument: opt_vec(p.legal_instrument),
        legal_article: opt_vec(p.legal_article),
        portee: opt_vec(p.portee),
        publication: opt_vec(p.publication),
        date_from: p.date_from,
        date_to: p.date_to,
        mode: p.mode,
        sort: p.sort,
        limit: p.limit,
        offset: p.offset,
        ai_mode: p.ai_mode,
    })
}

/// Valide une date `dateFrom`/`dateTo` à la frontière HTTP (parité du champ
/// `Optional[date]` borné de l'oracle) : 422 byte-identique à la
/// `RequestValidationError` sur format/plage invalide.
fn validate_date_param(field: &str, raw: &Option<String>) -> Result<()> {
    let Some(s) = raw else { return Ok(()) };
    match search::parse_search_date(s) {
        Ok(_) => Ok(()),
        Err(search::DateError::Parse(msg)) => Err(ApiError::Unprocessable(
            validation::date_parsing(&["query", field], s, msg),
        )),
        Err(search::DateError::TooEarly) => Err(ApiError::Unprocessable(
            validation::date_greater_than_equal(&["query", field], s, search::DATE_GE),
        )),
        Err(search::DateError::TooLate) => Err(ApiError::Unprocessable(
            validation::date_less_than_equal(&["query", field], s, search::DATE_LE),
        )),
    }
}

fn opt_vec<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Désérialise une valeur de query en enum via serde (les enums DTO ont leur
/// `rename_all` propre — on passe par `serde_json` pour respecter ce mapping).
fn de_enum<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

/// Parse une query string `a=1&b=2` en paires décodées (`+`→espace, `%XX`).
/// Dependency-free (pas de `form_urlencoded` en dép directe de `lj-api`).
fn parse_qs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (pct_decode(k), pct_decode(v)),
            None => (pct_decode(pair), String::new()),
        })
        .collect()
}

/// Décode un composant `application/x-www-form-urlencoded` (`+`→espace, `%XX`).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── Décision (détail + sous-ressources) ───────────────────────────────────────

async fn decision(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

/// Prévisualisation légère (hover card, ADR 0168).
async fn decision_preview(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::decision_preview(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

#[derive(Debug, Deserialize)]
struct SimilarQuery {
    // Capturé en brut : un `limit` non entier (`abc`) doit donner 422 (parité
    // FastAPI `Query(int)`), pas le rejet 400 de l'extracteur `Query<u32>`.
    limit: Option<String>,
}

async fn decision_similar(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Query(q): Query<SimilarQuery>,
) -> Result<Response> {
    let limit: u32 = match q.limit {
        None => 4,
        Some(v) => v.parse().map_err(|_| {
            ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &v))
        })?,
    };
    if limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &limit.to_string(),
            1,
        )));
    }
    if limit > 12 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &limit.to_string(),
            12,
        )));
    }
    let hits = decisions::similar_decisions(&state, &decision_id, limit).await?;
    let body = SimilarDecisionsResponse { decision_id, hits };
    Ok(with_cache(CACHE_DECISION, body))
}

/// Nom de fichier d'export (parité `_decision_filename`).
fn decision_filename(detail: &lj_dtos::DecisionDetail) -> String {
    // Parité Python (`result.jurisdiction_name or result.juridiction_type`) :
    // le fallback est le code brut du type (« TA », « CE »…), pas un libellé.
    let raw = detail.jurisdiction_name.clone().unwrap_or_else(|| {
        serde_json::to_value(detail.juridiction_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    });
    let jur = abbreviate_jurisdiction(&raw);

    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let date_str = detail
        .date_lecture
        .as_deref()
        .map(|d| {
            let parts: Vec<&str> = d.split('-').collect();
            match (parts.first(), parts.get(1), parts.get(2)) {
                (Some(y), Some(m), Some(day)) => match (day.parse::<usize>(), m.parse::<usize>()) {
                    (Ok(dd), Ok(mm)) if (1..=12).contains(&mm) => {
                        format!("{dd} {} {y}", MONTHS[mm - 1])
                    }
                    _ => d.to_string(),
                },
                _ => d.to_string(),
            }
        })
        .unwrap_or_default();

    let docket = detail
        .docket_numbers
        .as_ref()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_default();

    let parts: Vec<String> = [jur, date_str, docket]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        format!("decision-{}", detail.id)
    } else {
        parts.join(" ")
    }
}

/// Abrège les noms de juridiction (parité des `re.sub` de `_decision_filename`).
fn abbreviate_jurisdiction(jur: &str) -> String {
    let lower = jur.to_lowercase();
    if let Some(rest) = strip_prefix_ci(jur, &lower, "tribunal administratif") {
        format!("TA{rest}")
    } else if let Some(rest) = strip_prefix_ci(jur, &lower, "cour administrative d'appel") {
        format!("CAA{rest}")
    } else if let Some(rest) = strip_prefix_ci(jur, &lower, "conseil d'etat")
        .or_else(|| strip_prefix_ci(jur, &lower, "conseil d'état"))
    {
        format!("CE{rest}")
    } else {
        jur.to_string()
    }
}

fn strip_prefix_ci<'a>(orig: &'a str, lower: &str, prefix: &str) -> Option<&'a str> {
    lower.strip_prefix(prefix).map(|_| &orig[prefix.len()..])
}

async fn decision_download_docx(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id).await?;
    let filename = decision_filename(&result);
    // Rendu DOCX (docx-rs + packing ZIP) = CPU-bloquant → offload sur le pool
    // blocking, hors de l'unique thread async (runtime `current_thread`, ADR
    // 0061), comme le parse de `get_decision` et le highlight de search.rs.
    let content = tokio::task::spawn_blocking(move || docx_export::build_decision_docx(&result))
        .await
        .map_err(|e| ApiError::Internal(format!("docx render task: {e}")))?;
    Ok(attachment_response(
        content,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        &format!("attachment; filename=\"{filename}.docx\""),
    ))
}

async fn decision_download_pdf(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id).await?;
    let filename = decision_filename(&result);
    // Rendu PDF = CPU-bloquant (poste le plus lourd) → offload sur le pool
    // blocking, hors de l'unique thread async (runtime `current_thread`, ADR 0061).
    let content = tokio::task::spawn_blocking(move || pdf_export::build_decision_pdf(&result))
        .await
        .map_err(|e| ApiError::Internal(format!("pdf render task: {e}")))?;
    Ok(attachment_response(
        content,
        "application/pdf",
        &format!("attachment; filename=\"{filename}.pdf\""),
    ))
}

/// Réponse binaire avec `Content-Type`, `Content-Disposition` et `Cache-Control`
/// décision (les téléchargements portent `CACHE_DECISION`, parité Python).
fn attachment_response(content: Vec<u8>, content_type: &str, disposition: &str) -> Response {
    let mut resp = (StatusCode::OK, content).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&CACHE_DECISION.header_value()) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    resp
}

// ── Référentiel LEGI (`/api/loi/*`, ADR 0092) ─────────────────────────────────

/// Sous-routeur LEGI monté sous `/api/loi`. La sous-ressource littérale
/// `citing` est déclarée avant la capture `{date}` ; matchit (axum 0.8)
/// privilégie le segment statique sur le paramètre, sans collision.
fn legi_router() -> Router<AppState> {
    Router::new()
        .route("/{code}", get(legi_code_summary))
        // Table des matières (`/loi/{code}/sommaire`) : le segment littéral
        // `sommaire` est privilégié par matchit (axum 0.8) sur la capture
        // `{num}`, sans collision.
        .route("/{code}/sommaire", get(legi_code_toc))
        .route("/{code}/{num}", get(legi_article_in_force))
        .route("/{code}/{num}/citing", get(legi_article_citing))
        .route("/{code}/{num}/{date}", get(legi_article_at_date))
}

async fn legi_code_summary(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = legi::code_summary(&state, &code).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_code_toc(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = legi::code_toc(&state, &code).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_in_force(
    State(state): State<AppState>,
    Path((code, num)): Path<(String, String)>,
) -> Result<Response> {
    let body = legi::article_in_force(&state, &code, &num).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_at_date(
    State(state): State<AppState>,
    Path((code, num, date)): Path<(String, String, String)>,
) -> Result<Response> {
    let date = parse_legi_date(&date)?;
    let body = legi::article_at_date(&state, &code, &num, date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_citing(
    State(state): State<AppState>,
    Path((code, num)): Path<(String, String)>,
    Query(p): Query<LegiCitingQuery>,
) -> Result<Response> {
    p.validate()?;
    let body = legi::article_citing(&state, &code, &num, p.limit, p.offset).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Pagination des décisions citantes (`limit` 1–100, `offset` ≥ 0).
#[derive(Debug, Deserialize)]
struct LegiCitingQuery {
    #[serde(default = "default_citing_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_citing_limit() -> i64 {
    20
}
impl LegiCitingQuery {
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        Ok(())
    }
}

/// Recherche plein-texte d'articles (`GET /api/search-textes`, ADR 0114). `q`
/// vide ⇒ réponse vide (pas d'appel ParadeDB). Mêmes bornes `limit`/`offset` que
/// les décisions citantes. `code` optionnel borne la recherche à un texte.
async fn search_textes_endpoint(
    State(state): State<AppState>,
    Query(p): Query<ArticleSearchQuery>,
) -> Result<Response> {
    p.validate()?;
    if p.q.trim().is_empty() {
        return Ok(with_cache(
            CACHE_SEARCH,
            ArticleSearchResponse {
                hits: Vec::new(),
                total: 0,
                facets: ArticleSearchFacets {
                    jurisdiction: Vec::new(),
                    nature: Vec::new(),
                    source: Vec::new(),
                },
            },
        ));
    }
    let body = legi::search_textes(
        &state,
        &p.q,
        p.code.as_deref(),
        p.jurisdiction.as_deref(),
        p.nature.as_deref(),
        p.source.as_deref(),
        p.limit,
        p.offset,
    )
    .await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Catalogue des codes navigables (`GET /api/codes`). Réponse stable (le corpus
/// de référentiel bouge à l'ingest), cache décision comme les pages `/loi`.
async fn codes_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = legi::code_catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Compteurs globaux du corpus (`GET /api/corpus-stats`). Chiffres quasi
/// immuables (bougent à l'ingest quotidien) → cache décision, 24 h CDN.
async fn corpus_stats(State(state): State<AppState>) -> Result<Response> {
    let body = stats::corpus_stats(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Query `/api/search-textes` : `q` (requête), `code` (slug optionnel), facettes
/// `jurisdiction`/`nature`/`source` (filtres optionnels), `limit` 1–100,
/// `offset` ≥ 0.
#[derive(Debug, Deserialize)]
struct ArticleSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    jurisdiction: Option<String>,
    #[serde(default)]
    nature: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default = "default_citing_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
impl ArticleSearchQuery {
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        Ok(())
    }
}

/// Parse le segment `{date}` d'URL (ISO `YYYY-MM-DD`) — frontière de validation
/// unique (#12). Format invalide → 422 à la frontière HTTP (jamais envoyé au cast
/// `::date` du SQL).
fn parse_legi_date(raw: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(validation::date_parsing(
            &["path", "date"],
            raw,
            "invalid date",
        ))
    })
}

// ── Compte utilisateur (`/api/me/*`, RequiredUser) ────────────────────────────

async fn get_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    Ok(Json(me::get_me(&state, &sub).await?).into_response())
}

async fn patch_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Json(body): Json<UserProfileUpdate>,
) -> Result<Response> {
    // Bornes Pydantic `UserProfileUpdate.display_name` : min_length=1, max_length=80
    // quand fourni (None = pas de changement). Hors bornes → 422.
    if let Some(name) = body.display_name.as_deref() {
        let n = name.chars().count() as u64;
        if n < 1 {
            return Err(ApiError::Unprocessable(validation::string_too_short(
                &["body", "displayName"],
                name,
                1,
            )));
        }
        if n > 80 {
            return Err(ApiError::Unprocessable(validation::string_too_long(
                &["body", "displayName"],
                name,
                80,
            )));
        }
    }
    let profile = me::patch_me(&state, &sub, body.display_name.as_deref()).await?;
    Ok(Json(profile).into_response())
}

async fn delete_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    me::delete_me(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_activity_tracking(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Json(body): Json<ActivityTrackingUpdate>,
) -> Result<Response> {
    let profile = me::set_activity_tracking(&state, &sub, body.enabled).await?;
    Ok(Json(profile).into_response())
}

// ── Signets ─────────────────────────────────────────────────────────────────

async fn list_bookmarks(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    let (items, total) = bookmarks::list_bookmarks(&state, &sub).await?;
    Ok(Json(BookmarksResponse { items, total }).into_response())
}

async fn add_bookmark(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    bookmarks::add_bookmark(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_bookmark(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    bookmarks::remove_bookmark(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Historique de recherche ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default = "default_page_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_page_limit() -> i64 {
    50
}
impl PageQuery {
    /// Bornes Pydantic `list_history`/`list_views` : `limit` 1–100, `offset` ≥ 0.
    /// Hors bornes → 422 (et évite de passer un `offset` négatif au SQL → 500).
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        Ok(())
    }
}

async fn list_history(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Query(p): Query<PageQuery>,
) -> Result<Response> {
    p.validate()?;
    let (items, total) = search_history::list_history(&state, &sub, p.limit, p.offset).await?;
    Ok(Json(SearchHistoryResponse { items, total }).into_response())
}

async fn delete_entry(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(entry_id): Path<String>,
) -> Result<StatusCode> {
    // `{entry_id}` est `int` côté FastAPI : un id non entier → 422 (pas 400 du
    // rejet d'extracteur axum `Path<i64>`). On parse à la main pour la parité.
    let entry_id: i64 = entry_id.parse().map_err(|_| {
        ApiError::Unprocessable(validation::int_parsing(&["path", "entry_id"], &entry_id))
    })?;
    search_history::delete_entry(&state, &sub, entry_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_history(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    search_history::clear_history(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Décisions consultées ──────────────────────────────────────────────────────

async fn list_views(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Query(p): Query<PageQuery>,
) -> Result<Response> {
    p.validate()?;
    let (items, total) = decision_views::list_views(&state, &sub, p.limit, p.offset).await?;
    Ok(Json(DecisionViewsResponse { items, total }).into_response())
}

async fn record_view(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    decision_views::record_view(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_view(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    decision_views::delete_view(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_views(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    decision_views::clear_views(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Spec (parité `test_search_crash_inputs.py` côté oracle) : une date
    /// malformée ou hors plage est rejetée en 422 à la frontière HTTP — jamais
    /// envoyée au `::date` cast (qui ferait un 500).
    #[test]
    fn search_query_rejects_malformed_date() {
        let err = parse_search_query("q=x&dateFrom=not-a-date").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable, eu {err:?}");
        };
        assert_eq!(
            obj,
            json!({
                "type": "date_from_datetime_parsing",
                "loc": ["query", "dateFrom"],
                "msg": "Input should be a valid date or datetime, invalid character in year",
                "input": "not-a-date",
                "ctx": { "error": "invalid character in year" }
            })
        );
    }

    #[test]
    fn search_query_rejects_out_of_range_date() {
        let err = parse_search_query("q=x&dateTo=2300-01-01").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["type"], json!("less_than_equal"));
        assert_eq!(obj["loc"], json!(["query", "dateTo"]));
    }

    #[test]
    fn search_query_accepts_valid_date() {
        let req = parse_search_query("q=x&dateFrom=2024-01-01&dateTo=2024-12-31").unwrap();
        assert_eq!(req.date_from.as_deref(), Some("2024-01-01"));
        assert_eq!(req.date_to.as_deref(), Some("2024-12-31"));
    }
}
