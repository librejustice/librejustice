//! Assemblage du routeur axum.
//!
//! Les handlers de données (`search`, `decision/*`, `me/*`…) sont des fonctions
//! pures `(&AppState, …) -> Result<…>` ; ce module les adapte en handlers axum
//! (extraction des query/path params + alias camelCase, application des headers
//! `Cache-Control`).

use crate::auth::{OptionalUser, RequiredUser};
use crate::cache::{CachePolicy, CACHE_DECISION, CACHE_SEARCH};
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;
use crate::{
    decisions, entities, journals, jurisdiction_hubs, legi, mcp, me, norm_hubs, oauth, pdf_export,
    registre, robots, search, sitemap, stats, usage,
};
use axum::extract::{OriginalUri, Path, Query, RawQuery, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use lj_dtos::{
    ActivitySource, DecisionPart, Domain, HealthResponse, JurisdictionType, SearchMode,
    SearchRequest, Solution, SortOrder,
};
use serde::Deserialize;
use tower::Layer as _;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

/// Assemble les routes (data API `/api`, OAuth, discovery `.well-known`, MCP
/// optionnel) avec l'état injecté — sans fallback ni middleware.
fn assemble_routes(state: AppState, enable_mcp: bool) -> Router {
    // ── DATA API (préfixe `/internal`) ────────────────────────────────────────
    // The corpus reads, one `PageData` token each; searches and files spend their own class.
    let reads = Router::new()
        .route("/codes", get(codes_catalogue))
        .route("/corpus-stats", get(corpus_stats))
        .route("/jurisdictions", get(reader_jurisdictions))
        .route("/open-source", get(open_source_list))
        .route("/juridictions", get(jurisdictions_catalogue))
        .route("/juridictions/{code}", get(jurisdiction_hub))
        .route("/juridictions/{code}/{annee}", get(jurisdiction_year))
        .route("/normes", get(normes_catalogue))
        .route("/normes/{juris}/{fond}", get(norm_hub))
        .route("/normes/{juris}/{fond}/{annee}", get(norm_year))
        .route("/journals", get(journals_endpoint))
        .route("/journals/catalogue", get(journals_catalogue))
        .route("/journals/{uid}", get(annonce_detail))
        .route("/notices/{uid}", get(notice_detail))
        .route("/decision/{decision_id}", get(decision))
        .route("/decision/{decision_id}/similar", get(decision_similar))
        .route("/decision/{decision_id}/preview", get(decision_preview))
        .route("/decision/{decision_id}/parties", get(decision_parties))
        .route("/entity/{ns}/{id}", get(entity_page))
        .route("/entity/{ns}/{id}/decisions", get(entity_decisions))
        .route("/entity/{ns}/{id}/registre", get(entity_registre))
        .route("/entity/{ns}/{id}/documents", get(entity_documents))
        .route("/entities/search", get(entities_search))
        .route("/entities/directory", get(entities_directory))
        .route("/entities/stats", get(entities_stats))
        .nest_service(
            "/norm",
            middleware::from_fn(detach_jurisdiction).layer(legi_router().with_state(state.clone())),
        )
        .route("/places", get(places))
        .route("/corpus", get(place_corpus))
        .route("/corpus/{date}", get(place_corpus_at_date))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::admission::admit_read,
        ));
    let api = Router::new()
        .route("/health", get(health))
        .route("/origine-interne", get(origine_interne))
        .route("/search", get(search_endpoint))
        .route("/search/decompte", get(search_decompte_endpoint))
        .route("/search/instruments", get(search_instruments_endpoint))
        .route("/search-textes", get(search_textes_endpoint))
        .route(
            "/search-textes/decompte",
            get(search_textes_decompte_endpoint),
        )
        .route("/suggest", get(suggest_endpoint))
        .route(
            "/entity/{ns}/{id}/documents/{kind}/{doc}",
            get(entity_document),
        )
        .route(
            "/decision/{decision_id}/download.pdf",
            get(decision_download_pdf),
        )
        .route("/texte/{*rest}", get(texte_moved))
        .merge(reads)
        .merge(
            Router::new()
                .nest("/auth", crate::accounts::router())
                .nest(
                    "/auth/google",
                    crate::oidc_auth::router(crate::oidc_auth::Fournisseur::Google),
                )
                .nest(
                    "/auth/microsoft",
                    crate::oidc_auth::router(crate::oidc_auth::Fournisseur::Microsoft),
                )
                .layer(middleware::from_fn_with_state(state.clone(), auth_budget)),
        )
        .nest("/account", crate::account::router())
        .route("/me", get(get_me))
        .route("/me/api-keys", get(list_api_keys).post(create_api_key))
        .route("/me/api-keys/{key_id}", delete(revoke_api_key))
        .route(
            "/client/incident",
            post(client_incident).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/reads",
            post(crate::reads::report).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/landings",
            post(crate::reads::report_landing).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/actions/{action}", post(crate::reads::report_action))
        .route(
            "/entity-searches",
            post(crate::reads::report_entity_search)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .merge(crate::support::router().layer(axum::extract::DefaultBodyLimit::max(16 * 1024)))
        .merge(crate::contribution::router())
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex"),
        ));

    // ── Racine : OAuth + discovery `.well-known` (consommés hors `/api`) ──────
    let mut app: Router = Router::new()
        .nest("/admin", operations(state.clone()))
        .nest("/api", publique(state.clone()))
        .nest("/internal", api)
        .merge(oauth::router().layer(middleware::from_fn_with_state(state.clone(), auth_budget)))
        .merge(oauth::well_known_router())
        .merge(sitemap::router())
        .merge(robots::router())
        .merge(journals_feeds_router())
        .with_state(state.clone());

    // ── MCP (gated) ───────────────────────────────────────────────────────────
    if enable_mcp {
        app = app.merge(mcp::mcp_router(state.clone()));
    }

    // ── IndexNow ──────────────────────────────────────────────────────────────
    // Le protocole exige la clé servie à `https://<host>/<clé>.txt`.
    if let Some(key) = state.settings.indexnow_key.clone() {
        let path = format!("/{key}.txt");
        app = app.route(&path, get(move || std::future::ready(key.clone())));
    }
    app
}

fn apply_layers(app: Router, cors: CorsLayer) -> Router {
    app.layer(middleware::from_fn(pg_error_boundary))
        .layer(middleware::from_fn(renew_session_cookie))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(slow_request_log))
        .layer(OtelAxumLayer::default())
        .layer(OtelInResponseLayer)
}

/// Seuil au-delà duquel une requête laisse une ligne. Aligné sur le bucket
/// `le="2.5"` de l'histogramme Caddy que compte la règle d'alerte « latence
/// dégradée » : la métrique dit **combien**, cette ligne dit **lesquelles**.
const SLOW_REQUEST: std::time::Duration = std::time::Duration::from_millis(2500);

async fn slow_request_log(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = std::time::Instant::now();
    let resp = next.run(req).await;
    let elapsed = started.elapsed();
    if elapsed >= SLOW_REQUEST {
        let ms = elapsed.as_millis();
        // Le chemin MCP sait dire où son temps est passé ; les autres non, et une
        // ligne sans découpage vaut mieux qu'un champ à zéro qui ment.
        match resp.extensions().get::<crate::mcp::McpPhases>() {
            Some(p) => tracing::warn!(
                requete_lente = true,
                method = %method,
                path = %path,
                status = resp.status().as_u16(),
                elapsed_ms = ms as u64,
                pool_wait_ms = p.pool.as_millis() as u64,
                lookup_jeton_ms = p.lookup.as_millis() as u64,
                bump_last_seen_ms = p.bump.as_millis() as u64,
                dispatch_ms = p.dispatch.as_millis() as u64,
                outil = p.tool.unwrap_or("protocole"),
                "slow request: {method} {path} in {ms} ms \
                 (pool {}, jeton {}, bump {}, dispatch {} pour {})",
                p.pool.as_millis(),
                p.lookup.as_millis(),
                p.bump.as_millis(),
                p.dispatch.as_millis(),
                p.tool.unwrap_or("protocole"),
            ),
            None => tracing::warn!(
                requete_lente = true,
                method = %method,
                path = %path,
                status = resp.status().as_u16(),
                elapsed_ms = ms as u64,
                "slow request: {method} {path} in {ms} ms"
            ),
        }
    }
    resp
}

/// Frontière HTTP qui reclasse en 422 les erreurs Postgres marquées par
/// [`ApiError::into_response`] (extension [`crate::error::PgErrKind`]) : class 22
/// → 422 `invalid_request_bytes` ; message de syntaxe `paradedb.parse` → 422
/// `invalid_query_syntax`. Le `path`/`q` sont réinjectés ici depuis la requête
/// entrante.
/// Réémet le cookie de session quand l'extracteur d'identité l'a demandé : le
/// `Max-Age` du navigateur est figé à la connexion, alors que la base fait
/// glisser l'échéance. Sans ça une session utilisée tous les jours meurt quand
/// même au 30ᵉ.
async fn renew_session_cookie(mut req: Request, next: Next) -> Response {
    let slot = crate::session::CookieRefresh::default();
    req.extensions_mut().insert(slot.clone());
    let mut resp = next.run(req).await;
    if let Some(valeur) = slot.take() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&valeur) {
            resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
        }
    }
    resp
}

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
                .find(|(k, _)| matches!(k.as_str(), "query" | "q"))
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

pub fn build_router(state: AppState, enable_mcp: bool) -> Router {
    let cors = cors_layer(&state.settings.cors_origins);
    apply_layers(assemble_routes(state, enable_mcp), cors)
}

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

fn with_cache<T: serde::Serialize>(policy: CachePolicy, body: T) -> Response {
    let mut resp = Json(body).into_response();
    if let Ok(value) = HeaderValue::from_str(&policy.header_value()) {
        resp.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    resp
}

/// A body whose visibility depends on the caller, so the CDN must never hold it.
fn no_store<T: serde::Serialize>(body: T) -> Response {
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// ── Health ────────────────────────────────────────────────────────────────────

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: state.settings.version.clone(),
    })
}

async fn origine_interne(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(token) = state.settings.internal_origin_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !crate::usage::internal_origin(&headers, Some(token)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let pose = format!(
        "{}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
        lj_dtos::INTERNAL_ORIGIN_COOKIE,
        365 * 86_400
    );
    match crate::session::header(pose) {
        // An empty 200 and not a 204: Chrome aborts a navigation to a 204, and
        // this route is only ever a navigation target.
        Ok(entete) => (StatusCode::OK, [entete], Html("")).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Sentinel queries of the deep probe, each required non-empty. The wide one is
/// timeless, its list empties only if search is broken; the case name alone
/// triggers the exact key leg, and also carries the count, its narrow aggregate
/// costing a fraction of the wide one's.
const DEEP_PROBE_QUERIES: [&str; 2] = ["responsabilité de l'État", "Danthony"];

/// Recherche d'une sentinelle : une seule ligne rendue.
fn deep_probe_request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        jurisdiction_type: None,
        solution: None,
        legal_domain: None,
        jurisdiction_code: None,
        seat: None,
        home: None,
        formation: None,
        legal_instrument: None,
        legal_article: None,
        publication: None,
        date_from: None,
        date_to: None,
        search_in: None,
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: 1,
        offset: 0,
        ai_mode: false,
        lang: None,
    }
}

#[tracing::instrument(name = "health_deep", skip_all, fields(
    librejustice.probe.outcome = tracing::field::Empty,
    librejustice.probe.total = tracing::field::Empty,
    librejustice.probe.servis = tracing::field::Empty,
    librejustice.probe.elapsed_ms = tracing::field::Empty,
))]
async fn health_deep(State(state): State<AppState>) -> Response {
    let started = std::time::Instant::now();
    let mut total = 0i64;
    let mut servis = 0usize;
    let mut state_label = "ok";
    for (rang, query) in DEEP_PROBE_QUERIES.iter().enumerate() {
        let decompte = rang == DEEP_PROBE_QUERIES.len() - 1;
        let req = deep_probe_request(query);
        let fait = if decompte {
            search::search_nocache_counted(&state, &req).await
        } else {
            search::search_nocache(&state, &req).await
        };
        match fait {
            Ok(resp) if !resp.all_hit_paths.is_empty() && resp.total.unwrap_or(1) > 0 => {
                servis += resp.all_hit_paths.len();
                total += resp.total.unwrap_or_default();
            }
            Ok(_) => {
                tracing::warn!(query, "deep probe: sentinel with no result");
                state_label = "empty";
                break;
            }
            Err(e) => {
                tracing::warn!(query, error = %e, "deep probe: search failed");
                state_label = "error";
                break;
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let span = tracing::Span::current();
    span.record("librejustice.probe.elapsed_ms", elapsed_ms);
    span.record("librejustice.probe.total", total);
    span.record("librejustice.probe.servis", servis);
    span.record("librejustice.probe.outcome", state_label);

    let status = if state_label == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": state_label,
            "total": total,
            "servis": servis,
            "elapsed_ms": elapsed_ms,
            "version": state.settings.version,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClientIncidentKind {
    Hydration,
    HydrationOk,
    // `snake_case` puts no underscore before a digit.
    #[serde(rename = "route_404")]
    Route404,
    RechercheRefusee,
    StylesheetMissing,
}

impl ClientIncidentKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Hydration => "hydration",
            Self::HydrationOk => "hydration_ok",
            Self::Route404 => "route_404",
            Self::RechercheRefusee => "recherche_refusee",
            Self::StylesheetMissing => "stylesheet_missing",
        }
    }
}

#[derive(Deserialize)]
struct ClientIncident {
    kind: ClientIncidentKind,
    path: String,
    #[serde(default)]
    assets: Vec<String>,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    status: u16,
    #[serde(default)]
    stylesheet_missing: bool,
}

async fn client_incident(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClientIncident>,
) -> StatusCode {
    let budget_key = crate::ratelimit::caller_key(None, &headers);
    if state
        .budgets
        .take(crate::ratelimit::Cost::ClientIncident, budget_key.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let kind = body.kind.as_str();
    let assets = body.assets.join(",");
    match &body.kind {
        ClientIncidentKind::HydrationOk => tracing::info!(
            librejustice.client.kind = kind,
            librejustice.client.path = %body.path,
            librejustice.client.assets = %assets,
            librejustice.client.elapsed_ms = body.elapsed_ms,
            librejustice.client.status = body.status,
            "client incident: {kind}",
        ),
        _ => tracing::warn!(
            librejustice.client.kind = kind,
            librejustice.client.path = %body.path,
            librejustice.client.assets = %assets,
            librejustice.client.elapsed_ms = body.elapsed_ms,
            librejustice.client.status = body.status,
            librejustice.client.stylesheet_missing = body.stylesheet_missing,
            "client incident: {kind}",
        ),
    }
    StatusCode::NO_CONTENT
}

// ── Search ──────────────────────────────────────────────────────────────────

/// The query of `/api/search` and `/internal/search`, under the names of the
/// `search_decisions` arguments.
///
/// Read from the raw query string (`serde_urlencoded` does not take the repeated
/// lists `?jurisdiction_type=A&jurisdiction_type=B`), then turned into a
/// [`SearchRequest`].
#[derive(Debug)]
struct SearchParams {
    query: Option<String>,
    jurisdiction_type: Vec<JurisdictionType>,
    solution: Vec<Solution>,
    legal_domain: Vec<Domain>,
    jurisdiction_code: Vec<String>,
    seat: Vec<String>,
    home: Option<String>,
    formation: Vec<String>,
    legal_instrument: Vec<String>,
    legal_article: Vec<String>,
    publication: Vec<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    search_in: Vec<DecisionPart>,
    mode: SearchMode,
    sort: SortOrder,
    limit: u32,
    offset: u32,
    ai_rerank: Option<bool>,
    lang: Option<String>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: None,
            jurisdiction_type: Vec::new(),
            solution: Vec::new(),
            legal_domain: Vec::new(),
            jurisdiction_code: Vec::new(),
            seat: Vec::new(),
            home: None,
            formation: Vec::new(),
            legal_instrument: Vec::new(),
            legal_article: Vec::new(),
            publication: Vec::new(),
            date_from: None,
            date_to: None,
            search_in: Vec::new(),
            mode: SearchMode::Auto,
            sort: SortOrder::Relevance,
            limit: 20,
            offset: 0,
            ai_rerank: None,
            lang: None,
        }
    }
}

/// Trace un refus de validation de recherche. L'objet porte `loc`, `msg` et la
/// valeur fautive, jamais la question posée.
fn journaliser_refus(err: ApiError) -> ApiError {
    if let ApiError::Unprocessable(detail) = &err {
        tracing::warn!(
            librejustice.search.refus = %detail,
            "search refused: validation",
        );
    }
    err
}

async fn search_endpoint(
    state: State<AppState>,
    user: OptionalUser,
    headers: HeaderMap,
    raw: RawQuery,
) -> Result<Response> {
    search_served(state, user, headers, raw, Surface::Internal).await
}

async fn search_public_endpoint(
    state: State<AppState>,
    user: OptionalUser,
    headers: HeaderMap,
    raw: RawQuery,
) -> Result<Response> {
    search_served(state, user, headers, raw, Surface::Public).await
}

/// The parameters `openapi.json` names on the public doors, and the retired
/// filters, answered without them. Any other parameter would be ignored and answer
/// another question than the one asked, so it is refused.
const PUBLIC_SEARCH_PARAMS: [&str; 20] = [
    "query",
    "jurisdiction_type",
    "jurisdiction_code",
    "seat",
    "formation",
    "solution",
    "procedure",
    "office",
    "legal_domain",
    "publication",
    "legal_instrument",
    "legal_article",
    "date_from",
    "date_to",
    "search_in",
    "mode",
    "sort",
    "ai_rerank",
    "limit",
    "offset",
];
const PUBLIC_NORM_SEARCH_PARAMS: [&str; 8] = [
    "query",
    "code",
    "jurisdiction",
    "scope",
    "valid_from",
    "as_of",
    "limit",
    "offset",
];
const PUBLIC_NORM_PARAMS: [&str; 1] = ["as_of"];

fn refuse_unknown_params(raw: &str, known: &[&str]) -> Result<()> {
    let refused: Vec<serde_json::Value> = parse_qs(raw)
        .into_iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| validation::extra_forbidden(&["query", &key], &value, known))
        .collect();
    match refused.len() {
        0 => Ok(()),
        1 => Err(ApiError::Unprocessable(refused.into_iter().next().unwrap())),
        _ => Err(ApiError::Unprocessable(serde_json::Value::Array(refused))),
    }
}

/// The names `/api` took before it spoke the tools' vocabulary, still read for
/// the integrations written against them.
const LEGACY_SEARCH_PARAMS: [(&str, &str); 9] = [
    ("q", "query"),
    ("jurisdictionType", "jurisdiction_type"),
    ("jurisdictionCode", "jurisdiction_code"),
    ("legalDomain", "legal_domain"),
    ("legalInstrument", "legal_instrument"),
    ("legalArticle", "legal_article"),
    ("dateFrom", "date_from"),
    ("dateTo", "date_to"),
    ("aiMode", "ai_rerank"),
];
const LEGACY_NORM_PARAMS: [(&str, &str); 2] = [("q", "query"), ("date", "as_of")];

/// The query under the current names, and the former names it carried.
fn rename_legacy_params<'a>(
    raw: &str,
    legacy: &[(&'a str, &str)],
) -> Option<(String, Vec<&'a str>)> {
    let mut former = Vec::new();
    let pairs: Vec<String> = raw
        .split('&')
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .map_or((pair, None), |(k, v)| (k, Some(v)));
            match legacy.iter().find(|(old, _)| *old == key) {
                Some((old, new)) => {
                    former.push(*old);
                    value.map_or(new.to_string(), |v| format!("{new}={v}"))
                }
                None => pair.to_string(),
            }
        })
        .collect();
    (!former.is_empty()).then(|| (pairs.join("&"), former))
}

async fn legacy_search_params(req: Request, next: Next) -> Response {
    next.run(read_legacy_params(req, &LEGACY_SEARCH_PARAMS))
        .await
}

async fn legacy_norm_params(req: Request, next: Next) -> Response {
    next.run(read_legacy_params(req, &LEGACY_NORM_PARAMS)).await
}

/// Renames on the request and on its `OriginalUri` both, which the norm doors
/// read their parameters from.
fn read_legacy_params(mut req: Request, legacy: &[(&str, &str)]) -> Request {
    let Some((query, former)) = req
        .uri()
        .query()
        .and_then(|raw| rename_legacy_params(raw, legacy))
    else {
        return req;
    };
    tracing::warn!(
        librejustice.api.former_params = former.join(","),
        "api: parameters read under their former names"
    );
    let with_query = |uri: &Uri| {
        let mut parts = uri.clone().into_parts();
        parts.path_and_query = Some(
            format!("{}?{query}", uri.path())
                .parse()
                .expect("renaming ASCII keys keeps a query valid"),
        );
        Uri::from_parts(parts).expect("only the query changed")
    };
    let uri = with_query(req.uri());
    *req.uri_mut() = uri;
    if let Some(OriginalUri(original)) = req.extensions().get::<OriginalUri>() {
        let original = with_query(original);
        req.extensions_mut().insert(OriginalUri(original));
    }
    req
}

/// `Public` answers in the shape the MCP tools describe, one set of descriptions
/// covering both doors, and carries its own total: it has no `/decompte`.
#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Internal,
    Public,
}

impl Surface {
    fn source(self) -> ActivitySource {
        match self {
            Surface::Internal => ActivitySource::Web,
            Surface::Public => ActivitySource::Api,
        }
    }
}

async fn search_served(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    surface: Surface,
) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    if surface == Surface::Public {
        refuse_unknown_params(raw, &PUBLIC_SEARCH_PARAMS).map_err(journaliser_refus)?;
    }
    let mut req = parse_search_query(raw, surface == Surface::Public).map_err(journaliser_refus)?;
    validate_referential_filters(&state, &mut req)
        .await
        .map_err(journaliser_refus)?;
    let retired: Vec<String> = parse_qs(raw)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| search::RETIRED_FILTERS.contains(&key.as_str()))
        .collect();
    if !retired.is_empty() {
        tracing::warn!(
            librejustice.api.retired_params = retired.join(","),
            "api: retired filters answered without them"
        );
    }
    let marker = raw
        .split('&')
        .find_map(|pair| pair.strip_prefix("context="));
    let context = search_context(marker, surface);
    let identity = lj_identity::Identity::resolve(user_id.as_deref(), &headers);
    let budget_key = crate::ratelimit::key_of(&identity);
    let caller = usage::Caller::http(
        surface.source(),
        user_id.is_some(),
        context,
        &headers,
        &state.settings,
    )
    .pseudonymise(&identity, state.pseudonymiser.as_ref(), &headers);
    let result = match surface {
        Surface::Public => {
            search::search_with_count(&state, &req, caller, budget_key.as_ref()).await?
        }
        Surface::Internal => search::search(&state, &req, caller, budget_key.as_ref()).await?,
    };
    match surface {
        Surface::Internal => Ok(with_cache(CACHE_SEARCH, result)),
        Surface::Public => {
            let mut resp = with_cache(
                CACHE_SEARCH,
                crate::mcp_presenters::present_search_response(
                    &result,
                    &state.settings.web_base_url,
                    req.seat.as_deref().unwrap_or_default(),
                ),
            );
            if !retired.is_empty() {
                resp.headers_mut().insert(
                    header::HeaderName::from_static("x-lj-warning"),
                    HeaderValue::from_static(search::RETIRED_FILTERS_WARNING),
                );
            }
            Ok(resp)
        }
    }
}

/// Décompte et facettes des mêmes paramètres que `/api/search`, appelable en
/// parallèle : la clé du bundle se recalcule des paramètres, aucun jeton ne
/// circule. N'écrit pas d'historique, la recherche l'ayant déjà fait.
async fn search_decompte_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    let mut req = parse_search_query(raw, false).map_err(journaliser_refus)?;
    validate_referential_filters(&state, &mut req)
        .await
        .map_err(journaliser_refus)?;
    let identity = lj_identity::Identity::resolve(user_id.as_deref(), &headers);
    let budget_key = crate::ratelimit::key_of(&identity);
    let result = search::count(&state, &req, budget_key.as_ref()).await?;
    Ok(with_cache(CACHE_SEARCH, result))
}

/// What `needle` names in the catalogue, on the same parameters as
/// `/api/search`: the panel merges this answer into the facet of `/decompte`,
/// so both have to count the same set.
async fn search_instruments_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    let needle = parse_qs(raw)
        .into_iter()
        .find(|(k, _)| k == "needle")
        .map(|(_, v)| v)
        .unwrap_or_default();
    let mut req = parse_search_query(raw, false).map_err(journaliser_refus)?;
    validate_referential_filters(&state, &mut req)
        .await
        .map_err(journaliser_refus)?;
    let identity = lj_identity::Identity::resolve(user_id.as_deref(), &headers);
    let budget_key = crate::ratelimit::key_of(&identity);
    let result = search::suggest_instruments(&state, &req, &needle, budget_key.as_ref()).await?;
    Ok(with_cache(CACHE_SEARCH, result))
}

fn search_context(marker: Option<&str>, surface: Surface) -> lj_dtos::SearchContext {
    match (surface, marker) {
        (Surface::Internal, Some("teaser")) => lj_dtos::SearchContext::Teaser,
        _ => lj_dtos::SearchContext::User,
    }
}

/// The query string of `/api/search` as a [`SearchRequest`], within its bounds
/// (`query` length, `limit` 1–50, `offset` 0–99), 422 otherwise. The rerank
/// defaults on where the tool's does, on the public door.
fn parse_search_query(raw: &str, rerank_by_default: bool) -> Result<SearchRequest> {
    let mut p = SearchParams {
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: 20,
        offset: 0,
        ..SearchParams::default()
    };
    for (key, value) in parse_qs(raw) {
        match key.as_str() {
            "query" => p.query = Some(value),
            "jurisdiction_type" => p.jurisdiction_type.push(de_enum_strict(
                "jurisdiction_type",
                &value,
                &JurisdictionType::ALL,
            )?),
            "solution" => p
                .solution
                .push(de_enum_strict("solution", &value, &Solution::ALL)?),
            "legal_domain" => {
                p.legal_domain
                    .push(de_enum_strict("legal_domain", &value, &Domain::ALL)?)
            }
            "jurisdiction_code" => p.jurisdiction_code.push(value),
            "seat" => p.seat.push(value),
            "home" => p.home = Some(crate::jurisdictions::parse_home(&value)?),
            "formation" => p.formation.push(value),
            "legal_instrument" => p.legal_instrument.push(value),
            "legal_article" => p.legal_article.push(value),
            "publication" => p.publication.push(value),
            "date_from" => p.date_from = Some(value),
            "date_to" => p.date_to = Some(value),
            "search_in" => {
                p.search_in
                    .push(de_enum_strict("search_in", &value, &DecisionPart::ALL)?)
            }
            "mode" => {
                p.mode = de_enum::<SearchMode>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "mode"],
                        &value,
                        &["auto", "lexical", "semantic", "reference"],
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
            "ai_rerank" => {
                p.ai_rerank = Some(matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            }
            "lang" => p.lang = Some(checked_lang(&value)?.to_owned()),
            _ => {}
        }
    }

    // A missing or empty `query` is browsing: the filtered corpus listed by date.
    let query = p.query.unwrap_or_default();
    let q_len = query.chars().count() as u64;
    if q_len > lj_dtos::QUERY_MAX_CHARS as u64 {
        return Err(ApiError::Unprocessable(validation::string_too_long(
            &["query", "query"],
            &query,
            lj_dtos::QUERY_MAX_CHARS as u64,
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
    validate_date_param("date_from", &p.date_from)?;
    validate_date_param("date_to", &p.date_to)?;

    Ok(SearchRequest {
        query,
        jurisdiction_type: opt_vec(p.jurisdiction_type),
        solution: opt_vec(p.solution),
        legal_domain: opt_vec(p.legal_domain),
        jurisdiction_code: opt_vec(p.jurisdiction_code),
        seat: opt_vec(p.seat),
        home: p.home,
        formation: opt_vec(p.formation),
        legal_instrument: opt_vec(p.legal_instrument),
        legal_article: opt_vec(p.legal_article),
        publication: opt_vec(p.publication),
        date_from: p.date_from,
        date_to: p.date_to,
        search_in: opt_vec(p.search_in),
        mode: p.mode,
        sort: p.sort,
        limit: p.limit,
        offset: p.offset,
        ai_mode: p.ai_rerank.unwrap_or(rerank_by_default),
        lang: p.lang,
    })
}

/// The axes whose vocabulary is the referential's (`publication`,
/// `jurisdiction_code`, `seat`), which the pure boundary
/// ([`parse_search_query`]) does not know. An unknown token is a 422.
async fn validate_referential_filters(state: &AppState, req: &mut SearchRequest) -> Result<()> {
    if let Some(bad) = req
        .formation
        .iter()
        .flatten()
        .find(|p| !lj_core::formation::is_formation_path(p))
    {
        return Err(ApiError::Unprocessable(validation::value_error(
            &["query", "formation"],
            bad,
            &crate::referential::unknown_formation_msg(bad),
        )));
    }
    if req.publication.is_none() && req.jurisdiction_code.is_none() && req.seat.is_none() {
        return Ok(());
    }
    let refs = crate::referential::referential(state).await?;
    {
        let (field, values) = ("publication", &req.publication);
        if let Some(bad) = values
            .as_deref()
            .and_then(|vs| refs.find_unknown_token(field, vs))
        {
            let tokens = refs.facet_tokens(field);
            return Err(ApiError::Unprocessable(validation::enum_error(
                &["query", field],
                bad,
                &tokens,
            )));
        }
    }
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(ApiError::Unprocessable(validation::value_error(
            &["query", "jurisdiction_code"],
            bad,
            &crate::referential::unknown_jurisdiction_code_msg(bad, &refs),
        )));
    }
    if let Some(seats) = req.seat.as_mut() {
        crate::referential::canonicalize_seats(seats);
        for path in seats.iter_mut().filter(|p| !refs.has_seat(p)) {
            if let Some(node) = crate::referential::seat_under_its_order(path, refs.seat_nodes()) {
                *path = node.to_string();
            }
        }
    }
    if let Some(bad) = req.seat.iter().flatten().find(|p| !refs.has_seat(p)) {
        return Err(ApiError::Unprocessable(validation::value_error(
            &["query", "seat"],
            bad,
            &crate::referential::unknown_seat_msg(bad, refs.seat_nodes()),
        )));
    }
    Ok(())
}

/// A `date_from` or `date_to` out of form or range is a 422.
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

/// Désérialise un token d'axe enum de `/api/search` ; token inconnu → 422 `enum`
/// listant les variantes valides.
fn de_enum_strict<T: serde::de::DeserializeOwned + serde::Serialize>(
    field: &str,
    value: &str,
    all: &[T],
) -> Result<T> {
    de_enum(value).ok_or_else(|| {
        let variants: Vec<String> = all
            .iter()
            .filter_map(|v| match serde_json::to_value(v) {
                Ok(serde_json::Value::String(s)) => Some(s),
                _ => None,
            })
            .collect();
        let variants: Vec<&str> = variants.iter().map(String::as_str).collect();
        ApiError::Unprocessable(validation::enum_error(&["query", field], value, &variants))
    })
}

/// Parse une query string `a=1&b=2` en paires décodées (`+`→espace, `%XX`).
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

/// `?lang=it`: the reader's language, which the decision's text is picked for first.
#[derive(Debug, Deserialize)]
struct LangQuery {
    lang: Option<String>,
}

fn checked_lang(v: &str) -> Result<&str> {
    if v.len() == 2 && v.bytes().all(|b| b.is_ascii_lowercase()) {
        Ok(v)
    } else {
        Err(ApiError::Unprocessable(validation::value_error(
            &["query", "lang"],
            v,
            "a language is two lower-case letters",
        )))
    }
}

impl LangQuery {
    fn lang(&self) -> Result<Option<&str>> {
        self.lang.as_deref().map(checked_lang).transpose()
    }

    /// The reader's language a summary is served in, French when none is named.
    fn reader(&self) -> Result<&str> {
        Ok(self.lang()?.unwrap_or("fr"))
    }
}

async fn decision(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Query(q): Query<LangQuery>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id, q.lang()?).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

async fn decision_public(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let detail = decisions::get_decision(&state, &decision_id, None).await?;
    api_read(
        &state,
        user_id.as_deref(),
        &headers,
        &uri,
        lj_dtos::ReadKind::Decision,
        &lj_dtos::decision_path(
            detail.jurisdiction_type,
            detail.jurisdiction_code.as_deref(),
            &detail.id,
        ),
    );
    let refs = crate::referential::referential(&state).await?;
    Ok(with_cache(
        CACHE_DECISION,
        crate::mcp_presenters::present_decision_detail(
            &detail,
            &state.settings.web_base_url,
            &refs,
        ),
    ))
}

async fn decision_preview(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Query(q): Query<LangQuery>,
) -> Result<Response> {
    let result = decisions::decision_preview(&state, &decision_id, q.reader()?).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

/// Encart « Parties » d'une décision. Décision inconnue → 404.
async fn decision_parties(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let body = entities::decision_parties(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

// ── Fiche entité (`/api/entity/*`) ───────────────────────────────────────────

async fn entity_page(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
) -> Result<Response> {
    let body = entities::entity_page(&state, &ns, &id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Volet registre d'une entité — APIs publiques à l'affichage, sections absentes
/// si l'amont est indisponible (jamais de 5xx).
async fn entity_registre(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
) -> Result<Response> {
    let body = registre::entity_registre(&state, &ns, &id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// A company's deeds and accounts, listed by the INPI on the reader's click.
async fn entity_documents(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Path((ns, id)): Path<(String, String)>,
) -> Result<Response> {
    spend_export(&state, user_id.as_deref(), &headers).await?;
    let body = crate::inpi::documents(&state, &ns, &id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn entity_document(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Path((ns, id, kind, doc)): Path<(String, String, String, String)>,
) -> Result<Response> {
    spend_export(&state, user_id.as_deref(), &headers).await?;
    let content = crate::inpi::file(&state, &ns, &id, &kind, &doc).await?;
    Ok(attachment_response(
        content.to_vec(),
        "application/pdf",
        &format!("inline; filename=\"{id}-{doc}.pdf\""),
    ))
}

async fn spend_export(state: &AppState, user_id: Option<&str>, headers: &HeaderMap) -> Result<()> {
    let budget_key = crate::ratelimit::caller_key(user_id, headers);
    if state
        .budgets
        .take(crate::ratelimit::Cost::Export, budget_key.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return Err(ApiError::TooManyRequests);
    }
    Ok(())
}

/// Pagination des décisions d'une entité : `page` 1-basé (défaut 1),
/// `page_size` 1–100 (défaut 20). Hors bornes → 422.
#[derive(Debug, Deserialize)]
struct EntityDecisionsQuery {
    #[serde(default = "default_entity_page")]
    page: i64,
    #[serde(default = "default_entity_page_size")]
    page_size: i64,
}
fn default_entity_page() -> i64 {
    1
}
fn default_entity_page_size() -> i64 {
    20
}
impl EntityDecisionsQuery {
    fn validate(&self) -> Result<()> {
        if self.page < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "page"],
                &self.page.to_string(),
                1,
            )));
        }
        if self.page_size < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "page_size"],
                &self.page_size.to_string(),
                1,
            )));
        }
        if self.page_size > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "page_size"],
                &self.page_size.to_string(),
                100,
            )));
        }
        Ok(())
    }
}

async fn entity_decisions(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
    Query(p): Query<EntityDecisionsQuery>,
) -> Result<Response> {
    p.validate()?;
    let body = entities::entity_decisions(&state, &ns, &id, p.page, p.page_size).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

// ── Annuaire des entités (`/api/entities/*`) ─────────────────────────────────

/// Slugs de catégorie acceptés par `kind`, alignés sur la route web
/// `/annuaire/{kind}`.
const ENTITY_KINDS: &[&str] = &[
    "entreprises",
    "personnes-publiques",
    "associations",
    "avocats",
    "cabinets",
];

/// [`ENTITY_KINDS`] minus the kinds naming natural persons, who are never listed.
const DIRECTORY_KINDS: &[&str] = &[
    "entreprises",
    "personnes-publiques",
    "associations",
    "cabinets",
];

/// Résout un `kind` optionnel en catégorie stockée. `None` en entrée → `None`
/// (pas de filtre) ; kind inconnu → 422.
fn resolve_kind(kind: Option<&str>) -> Result<Option<&'static str>> {
    match kind {
        None => Ok(None),
        Some(k) => Ok(Some(entities::kind_to_category(k).ok_or_else(|| {
            ApiError::Unprocessable(validation::enum_error(&["query", "kind"], k, ENTITY_KINDS))
        })?)),
    }
}

/// Query `/api/suggest` : `q` (min 2 codepoints), `mode` (`jurisprudence` |
/// `textes` | `annuaire`, défaut `jurisprudence`).
#[derive(Debug, Deserialize)]
struct SuggestQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

async fn suggest_endpoint(
    State(state): State<AppState>,
    Query(p): Query<SuggestQuery>,
) -> Result<Response> {
    let q = p.q.unwrap_or_default();
    if q.chars().count() < entities::ENTITY_SEARCH_MIN_QUERY {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &q,
            entities::ENTITY_SEARCH_MIN_QUERY as u64,
        )));
    }
    let body =
        search::suggest::suggest(&state, &q, p.mode.as_deref().unwrap_or("jurisprudence")).await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Query `/api/entities/search` : `q` (préfixe, min 2 codepoints), `kind`
/// (catégorie optionnelle), `limit` (1–50, défaut 10).
#[derive(Debug, Deserialize)]
struct EntitySearchQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_entity_search_limit")]
    limit: i64,
}
fn default_entity_search_limit() -> i64 {
    10
}

async fn entities_search(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Query(p): Query<EntitySearchQuery>,
) -> Result<Response> {
    let q = p.q.unwrap_or_default();
    let q_len = q.chars().count();
    if q_len < entities::ENTITY_SEARCH_MIN_QUERY {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &q,
            entities::ENTITY_SEARCH_MIN_QUERY as u64,
        )));
    }
    if p.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            1,
        )));
    }
    if p.limit > entities::ENTITY_SEARCH_MAX_LIMIT {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            entities::ENTITY_SEARCH_MAX_LIMIT,
        )));
    }
    let category = resolve_kind(p.kind.as_deref())?;
    let authenticated = user_id.is_some();
    let names_persons = category.is_some_and(entities::names_natural_persons);
    if names_persons {
        if !authenticated {
            return Err(ApiError::Unauthorized);
        }
        let budget_key = crate::ratelimit::caller_key(user_id.as_deref(), &headers);
        if state
            .budgets
            .take(crate::ratelimit::Cost::Annuaire, budget_key.as_ref())
            .await
            == crate::ratelimit::Verdict::Exhausted
        {
            return Err(ApiError::TooManyRequests);
        }
    }
    let body = entities::entity_search(&state, &q, category, authenticated, p.limit).await?;
    if names_persons {
        return Ok(no_store(body));
    }
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Query `/api/entities/directory`: `kind` (required, one of [`DIRECTORY_KINDS`]),
/// `page` (>= 1, default 1), `page_size` (1 to 100, default 20).
#[derive(Debug, Deserialize)]
struct EntityDirectoryQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_entity_page")]
    page: i64,
    #[serde(default = "default_entity_page_size")]
    page_size: i64,
}

async fn entities_directory(
    State(state): State<AppState>,
    Query(p): Query<EntityDirectoryQuery>,
) -> Result<Response> {
    let kind = p
        .kind
        .as_deref()
        .ok_or_else(|| ApiError::Unprocessable(validation::missing(&["query", "kind"])))?;
    let category = entities::kind_to_category(kind)
        .filter(|c| !entities::names_natural_persons(c))
        .ok_or_else(|| {
            ApiError::Unprocessable(validation::enum_error(
                &["query", "kind"],
                kind,
                DIRECTORY_KINDS,
            ))
        })?;
    if p.page < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "page"],
            &p.page.to_string(),
            1,
        )));
    }
    if p.page_size < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "page_size"],
            &p.page_size.to_string(),
            1,
        )));
    }
    if p.page_size > 100 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "page_size"],
            &p.page_size.to_string(),
            100,
        )));
    }
    if p.page.saturating_mul(p.page_size) > entities::ENTITY_DIRECTORY_MAX_DEPTH {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "page"],
            &p.page.to_string(),
            entities::ENTITY_DIRECTORY_MAX_DEPTH / p.page_size,
        )));
    }
    let body = entities::entity_directory(&state, category, p.page, p.page_size).await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

async fn entities_stats(State(state): State<AppState>) -> Result<Response> {
    let body = entities::annuaire_stats(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

#[derive(Debug, Deserialize)]
struct SimilarQuery {
    limit: Option<String>,
}

async fn decision_similar(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Query(q): Query<SimilarQuery>,
    Query(l): Query<LangQuery>,
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
    let body = decisions::similar_decisions(&state, &decision_id, limit, l.reader()?).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

fn decision_filename(detail: &lj_dtos::DecisionDetail) -> String {
    let raw = detail.jurisdiction_name.clone().unwrap_or_else(|| {
        serde_json::to_value(detail.jurisdiction_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    });
    let jur = lj_dtos::abbreviate_jurisdiction_name(&raw);

    let date_str = detail
        .date_lecture
        .as_deref()
        .map(lj_dtos::format_date_fr)
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
        // Les n° RG judiciaires contiennent un `/` (« 25/01119 »), interdit dans un nom
        // de fichier.
        parts.join(" ").replace('/', "_")
    }
}

async fn decision_download_pdf(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
    Query(q): Query<LangQuery>,
) -> Result<Response> {
    let budget_key = crate::ratelimit::caller_key(user_id.as_deref(), &headers);
    if state
        .budgets
        .take(crate::ratelimit::Cost::Export, budget_key.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return Err(ApiError::TooManyRequests);
    }
    let result = decisions::get_decision(&state, &decision_id, q.lang()?).await?;
    let filename = decision_filename(&result);
    let content = tokio::task::spawn_blocking(move || pdf_export::build_decision_pdf(&result))
        .await
        .map_err(|e| ApiError::Internal(format!("pdf render task: {e}")))?;
    Ok(attachment_response(
        content,
        "application/pdf",
        &format!("attachment; filename=\"{filename}.pdf\""),
    ))
}

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

fn operations(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/health/deep", get(health_deep))
        .layer(middleware::from_fn_with_state(state, admin_only))
}

async fn admin_only(State(state): State<AppState>, requete: Request, next: Next) -> Response {
    if !usage::is_first_party(requete.headers(), &state.settings) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(requete).await
}

/// `/texte/…` became `/norm/…`, under `/api` as under `/internal`: a permanent
/// redirect, query kept.
async fn texte_moved(axum::extract::OriginalUri(uri): axum::extract::OriginalUri) -> Response {
    let (prefix, rest) = uri.path().split_once("/texte/").unwrap_or(("", ""));
    let dest = match uri.query() {
        Some(q) => format!("{prefix}/norm/{rest}?{q}"),
        None => format!("{prefix}/norm/{rest}"),
    };
    axum::response::Redirect::permanent(&dest).into_response()
}

/// MCP tool parity in REST, nothing else: the tools' argument names, their outputs.
fn publique(state: AppState) -> Router<AppState> {
    let textes = Router::new()
        .route("/{code}", get(legi_root_public))
        .route("/{code}/section/{cid}", get(legi_section_cid_public))
        .route("/{code}/{num}", get(legi_article_public))
        .route("/{code}/{num}/{*rest}", get(legi_section_public))
        .with_state(state.clone());
    let lectures = Router::new()
        .route("/decision/{decision_id}", get(decision_public))
        .nest_service(
            "/norm",
            middleware::from_fn(detach_jurisdiction).layer(textes),
        )
        .route("/article/{uid}", get(legi_article_by_uid_public))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::api_keys::lecture,
        ));
    Router::new()
        .route(
            "/search",
            get(search_public_endpoint).layer(middleware::from_fn(legacy_search_params)),
        )
        .route(
            "/search-textes",
            get(search_textes_public_endpoint).layer(middleware::from_fn(legacy_norm_params)),
        )
        .merge(lectures.layer(middleware::from_fn(legacy_norm_params)))
        .layer(middleware::from_fn_with_state(
            state,
            crate::api_keys::garde,
        ))
        .route("/texte/{*rest}", get(texte_moved))
}

/// One `Auth` token per network, before any session exists.
async fn auth_budget(State(state): State<AppState>, requete: Request, next: Next) -> Response {
    let cle = crate::ratelimit::caller_key(None, requete.headers());
    if state
        .budgets
        .take(crate::ratelimit::Cost::Auth, cle.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return ApiError::TooManyRequests.into_response();
    }
    next.run(requete).await
}

// ── Référentiel LEGI (`/api/norm/*`) ───────────────────────────────────────

/// Les sous-ressources littérales `citing`/`related` sont déclarées avant la
/// capture `{date}` : matchit privilégie le segment statique sur le paramètre.
fn legi_router() -> Router<AppState> {
    Router::new()
        .route("/{code}", get(legi_code_summary))
        .route("/{code}/sommaire", get(legi_code_toc))
        .route("/{code}/section/{*path}", get(legi_section_reading))
        .route("/{code}/section-citing/{*path}", get(legi_section_citing))
        .route("/{code}/{num}", get(legi_article_in_force))
        .route("/{code}/{num}/citing", get(legi_article_citing))
        .route("/{code}/{num}/related", get(legi_article_related))
        .route("/{code}/{num}/{date}", get(legi_article_at_date))
        // Bornes du comparateur : dates ISO de fenêtre de version, `initiale` = borne
        // ouverte.
        .route("/{code}/{num}/compare/{de}/{a}", get(legi_article_compare))
}

/// The jurisdiction a text URL names in its first segment, detached before the
/// router sees it: `/{juridiction}/{code}` and `/{code}/{num}` share an arity,
/// so the segment cannot be a route parameter without making every text path
/// ambiguous. No text slug is two letters, nor `intl`, which is what makes the
/// first segment readable on its own.
#[derive(Clone, Copy)]
struct UrlJurisdiction(&'static str);

fn url_juris(ext: &Option<Extension<UrlJurisdiction>>) -> Option<&'static str> {
    ext.as_ref().map(|Extension(j)| j.0)
}

/// Mounted by `nest_service`, the only seam where a layer still runs before the
/// matching: `Router::layer` fires once the route is already chosen, on
/// parameters read from the path this rewrites.
async fn detach_jurisdiction(mut req: Request, next: Next) -> Response {
    let path = req.uri().path().trim_start_matches('/').to_string();
    let (first, tail) = path.split_once('/').unwrap_or((path.as_str(), ""));
    let Some(code) = lj_norm::jurisdictions::from_url_segment(first) else {
        return next.run(req).await;
    };
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let mut parts = req.uri().clone().into_parts();
    let Ok(rewritten) = format!("/{tail}{query}").parse() else {
        return next.run(req).await;
    };
    parts.path_and_query = Some(rewritten);
    let Ok(uri) = Uri::from_parts(parts) else {
        return next.run(req).await;
    };
    *req.uri_mut() = uri;
    req.extensions_mut().insert(UrlJurisdiction(code));
    next.run(req).await
}

async fn legi_code_summary(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = legi::code_summary(&state, &code, url_juris(&juris)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_code_toc(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path(code): Path<String>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = q.date.as_deref().map(parse_legi_date).transpose()?;
    let body = legi::code_toc(&state, &code, url_juris(&juris), date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_section_reading(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, path)): Path<(String, String)>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = q.date.as_deref().map(parse_legi_date).transpose()?;
    let body = legi::law_section(&state, &code, url_juris(&juris), &path, date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_in_force(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, num)): Path<(String, String)>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let body =
        legi::article_in_force(&state, &code, url_juris(&juris), &num, q.place.as_deref()).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_at_date(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, num, date)): Path<(String, String, String)>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = parse_legi_date(&date)?;
    let body = legi::article_at_date(
        &state,
        &code,
        url_juris(&juris),
        &num,
        date,
        q.place.as_deref(),
    )
    .await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// The place vocabulary a reader picks from.
async fn places(State(state): State<AppState>) -> Result<Response> {
    let body = legi::places(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// What applies in the reader's place today.
async fn place_corpus(
    State(state): State<AppState>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let body = legi::place_corpus(&state, q.place.as_deref(), None).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// The same corpus read on a past or future day.
async fn place_corpus_at_date(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = parse_legi_date(&date)?;
    let body = legi::place_corpus(&state, q.place.as_deref(), Some(date)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// The four doors the `get_norm` description gives, addressed the way the
/// `url` of a hit reads: the text alone, one of its divisions, one article, and
/// a Légifrance id of any of the three.
async fn legi_root_public(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    OriginalUri(uri): OriginalUri,
    juris: Option<Extension<UrlJurisdiction>>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Response> {
    refuse_unknown_params(uri.query().unwrap_or(""), &PUBLIC_NORM_PARAMS)?;
    let code = legi::resolve_code(&state, &code, url_juris(&juris)).await?;
    let web = &state.settings.web_base_url;
    let body = match legi::law_root(&state, &code, url_juris(&juris)).await? {
        legi::LawRoot::Body(summary) => crate::mcp_presenters::present_law_text(&summary, web),
        legi::LawRoot::Toc(summary, toc) => {
            crate::mcp_presenters::present_law_text_root(&summary, &toc, web)
        }
    };
    api_read(
        &state,
        user_id.as_deref(),
        &headers,
        &uri,
        lj_dtos::ReadKind::Text,
        &body.url,
    );
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_section_public(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    OriginalUri(uri): OriginalUri,
    juris: Option<Extension<UrlJurisdiction>>,
    headers: HeaderMap,
    Path((code, first, rest)): Path<(String, String, String)>,
    Query(q): Query<AsOfQuery>,
) -> Result<Response> {
    refuse_unknown_params(uri.query().unwrap_or(""), &PUBLIC_NORM_PARAMS)?;
    let code = legi::resolve_code(&state, &code, url_juris(&juris)).await?;
    // The page url of an article at a date, or of its comparator, names the article.
    let dated = parse_legi_date(&rest).is_ok().then_some(rest.as_str());
    if dated.is_some() || rest.starts_with("compare/") {
        let date = q.as_of.as_deref().or(dated);
        let article = article_public(&state, &code, url_juris(&juris), &first, date).await?;
        api_read(
            &state,
            user_id.as_deref(),
            &headers,
            &uri,
            lj_dtos::ReadKind::Article,
            &article.url,
        );
        return Ok(with_cache(CACHE_DECISION, article));
    }
    let path = format!("{first}/{rest}");
    let read = (user_id.as_deref(), &headers, &uri);
    section_public(
        &state,
        read,
        &code,
        url_juris(&juris),
        &path,
        q.as_of.as_deref(),
    )
    .await
}

async fn section_public(
    state: &AppState,
    (user, headers, uri): (Option<&str>, &HeaderMap, &Uri),
    code: &str,
    juris: Option<&str>,
    path: &str,
    date: Option<&str>,
) -> Result<Response> {
    let date = date.map(parse_legi_date).transpose()?;
    let section = legi::law_section(state, code, juris, path, date).await?;
    let body = crate::mcp_presenters::present_law_section(&section, &state.settings.web_base_url);
    api_read(
        state,
        user,
        headers,
        uri,
        lj_dtos::ReadKind::Section,
        &body.url,
    );
    Ok(with_cache(CACHE_DECISION, body))
}

/// A section named by its DILA id answers at the path of its node.
async fn legi_section_cid_public(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, cid)): Path<(String, String)>,
    uri: axum::extract::OriginalUri,
) -> Result<Response> {
    let code = legi::resolve_code(&state, &code, url_juris(&juris)).await?;
    let address = legi::section_address(&state, &code, url_juris(&juris), &cid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let dest = match uri.query() {
        Some(q) => format!("/api{address}?{q}"),
        None => format!("/api{address}"),
    };
    Ok(axum::response::Redirect::permanent(&dest).into_response())
}

/// One segment names an article, or a first-level section when no article
/// answers it.
async fn legi_article_public(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    OriginalUri(uri): OriginalUri,
    juris: Option<Extension<UrlJurisdiction>>,
    headers: HeaderMap,
    Path((code, num)): Path<(String, String)>,
    Query(q): Query<AsOfQuery>,
) -> Result<Response> {
    refuse_unknown_params(uri.query().unwrap_or(""), &PUBLIC_NORM_PARAMS)?;
    let code = legi::resolve_code(&state, &code, url_juris(&juris)).await?;
    match article_public(&state, &code, url_juris(&juris), &num, q.as_of.as_deref()).await {
        Ok(article) => {
            api_read(
                &state,
                user_id.as_deref(),
                &headers,
                &uri,
                lj_dtos::ReadKind::Article,
                &article.url,
            );
            Ok(with_cache(CACHE_DECISION, article))
        }
        Err(ApiError::NotFound) => {
            let read = (user_id.as_deref(), &headers, &uri);
            section_public(
                &state,
                read,
                &code,
                url_juris(&juris),
                &num,
                q.as_of.as_deref(),
            )
            .await
        }
        Err(e) => Err(e),
    }
}

async fn legi_article_by_uid_public(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(uid): Path<String>,
    Query(q): Query<AsOfQuery>,
) -> Result<Response> {
    refuse_unknown_params(uri.query().unwrap_or(""), &PUBLIC_NORM_PARAMS)?;
    match lj_norm::text_identity::legifrance_id(&uid) {
        Some((lj_norm::text_identity::IdKind::Text, _)) => {
            let (code, juris) = legi::text_ref_by_uid(&state, &uid)
                .await?
                .ok_or(ApiError::NotFound)?;
            let read = (user_id.as_deref(), &headers, &uri);
            text_root_public(&state, read, &code, Some(&juris)).await
        }
        Some((lj_norm::text_identity::IdKind::Section, _)) => {
            let mut texts = legi::section_texts_by_cid(&state, &uid).await?;
            if texts.len() != 1 {
                return Err(ApiError::NotFound);
            }
            let (code, juris) = texts.remove(0);
            let address = legi::section_address(&state, &code, Some(&juris), &uid)
                .await?
                .ok_or(ApiError::NotFound)?;
            let dest = match &q.as_of {
                Some(d) => format!("/api{address}?as_of={d}"),
                None => format!("/api{address}"),
            };
            Ok(axum::response::Redirect::permanent(&dest).into_response())
        }
        _ => {
            let mut targets = legi::article_refs_by_source_uid(&state, &uid).await?;
            if targets.len() != 1 {
                return Err(ApiError::NotFound);
            }
            let (code, num) = targets.remove(0);
            if num.is_empty() {
                let read = (user_id.as_deref(), &headers, &uri);
                return text_root_public(&state, read, &code, None).await;
            }
            let article = article_public(&state, &code, None, &num, q.as_of.as_deref()).await?;
            api_read(
                &state,
                user_id.as_deref(),
                &headers,
                &uri,
                lj_dtos::ReadKind::Article,
                &article.url,
            );
            Ok(with_cache(CACHE_DECISION, article))
        }
    }
}

/// A text read whole by its id: its body, else its table of contents.
async fn text_root_public(
    state: &AppState,
    (user, headers, uri): (Option<&str>, &HeaderMap, &Uri),
    code: &str,
    juris: Option<&str>,
) -> Result<Response> {
    let web = &state.settings.web_base_url;
    let body = match legi::law_root(state, code, juris).await? {
        legi::LawRoot::Body(summary) => crate::mcp_presenters::present_law_text(&summary, web),
        legi::LawRoot::Toc(summary, toc) => {
            crate::mcp_presenters::present_law_text_root(&summary, &toc, web)
        }
    };
    api_read(
        state,
        user,
        headers,
        uri,
        lj_dtos::ReadKind::Text,
        &body.url,
    );
    Ok(with_cache(CACHE_DECISION, body))
}

fn api_read(
    state: &AppState,
    user: Option<&str>,
    headers: &HeaderMap,
    uri: &Uri,
    kind: lj_dtos::ReadKind,
    document: &str,
) {
    let identity = lj_identity::Identity::resolve(user, headers);
    let caller = usage::Caller::http(
        ActivitySource::Api,
        user.is_some(),
        lj_dtos::SearchContext::User,
        headers,
        &state.settings,
    )
    .pseudonymise(&identity, state.pseudonymiser.as_ref(), headers);
    crate::reads::served(
        kind,
        crate::reads::Surface::Api,
        &caller,
        uri.path(),
        document,
    );
}

/// Unlike the internal door, the public one takes any spelling of an article
/// number: nothing hands it a `num_key` it did not compose itself.
async fn article_public(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    date: Option<&str>,
) -> Result<crate::mcp_presenters::McpLawArticle> {
    let num = lj_norm::article_key::article_key(num);
    let article = match date.map(parse_legi_date).transpose()? {
        Some(date) => legi::article_at_date(state, code, juris, &num, date, None).await?,
        None => legi::article_in_force(state, code, juris, &num, None).await?,
    };
    Ok(crate::mcp_presenters::present_law_article(
        &article,
        &state.settings.web_base_url,
    ))
}

async fn legi_article_compare(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, num, de, a)): Path<(String, String, String, String)>,
) -> Result<Response> {
    let body = legi::article_compare(&state, &code, url_juris(&juris), &num, &de, &a).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_citing(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, num)): Path<(String, String)>,
    Query(p): Query<LegiCitingQuery>,
    Query(l): Query<LangQuery>,
) -> Result<Response> {
    p.validate()?;
    let body = legi::article_citing(
        &state,
        &code,
        url_juris(&juris),
        &num,
        p.date.as_deref(),
        p.place.as_deref(),
        p.limit,
        p.offset,
        l.reader()?,
    )
    .await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_section_citing(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, path)): Path<(String, String)>,
    Query(p): Query<LegiCitingQuery>,
    Query(l): Query<LangQuery>,
) -> Result<Response> {
    p.validate()?;
    let body = legi::section_citing(
        &state,
        &code,
        url_juris(&juris),
        &path,
        p.date.as_deref(),
        p.limit,
        p.offset,
        l.reader()?,
    )
    .await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_related(
    State(state): State<AppState>,
    juris: Option<Extension<UrlJurisdiction>>,
    Path((code, num)): Path<(String, String)>,
) -> Result<Response> {
    let body = legi::article_co_cited(&state, &code, url_juris(&juris), &num).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Date de consultation Chronolégi (`?date=YYYY-MM-DD`) des sommaires/sections ;
/// absente = en vigueur aujourd'hui.
///
/// `place` is the reader's place (`PF`), read by the article doors only.
#[derive(Debug, Deserialize)]
struct LegiDateQuery {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    place: Option<String>,
}

/// The consultation date of the public doors, named as the `get_norm` argument.
#[derive(Debug, Deserialize)]
struct AsOfQuery {
    #[serde(default)]
    as_of: Option<String>,
}

/// Pagination des décisions citantes (`limit` 1–100, `offset` ≥ 0, fenêtre
/// `offset + limit ≤ 100`) + `date` de la version servie (ISO `YYYY-MM-DD`,
/// absente = en vigueur) qui borne les citantes. Sur une division, `date` choisit
/// la composition du sous-arbre agrégé.
#[derive(Debug, Deserialize)]
struct LegiCitingQuery {
    #[serde(default = "default_citing_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    place: Option<String>,
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
        if self.offset + self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                100 - self.limit,
            )));
        }
        Ok(())
    }
}

/// `offset` stops at 99, as on decisions: the reachable window ends there, and the
/// referential is not walked page by page.
async fn search_textes_endpoint(
    state: State<AppState>,
    user: OptionalUser,
    headers: HeaderMap,
    query: Query<ArticleSearchQuery>,
    raw: RawQuery,
) -> Result<Response> {
    search_textes_served(state, user, headers, query, raw, Surface::Internal).await
}

async fn search_textes_public_endpoint(
    state: State<AppState>,
    user: OptionalUser,
    headers: HeaderMap,
    query: Query<ArticleSearchQuery>,
    raw: RawQuery,
) -> Result<Response> {
    search_textes_served(state, user, headers, query, raw, Surface::Public).await
}

async fn search_textes_served(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Query(p): Query<ArticleSearchQuery>,
    RawQuery(raw): RawQuery,
    surface: Surface,
) -> Result<Response> {
    if surface == Surface::Public {
        refuse_unknown_params(raw.as_deref().unwrap_or(""), &PUBLIC_NORM_SEARCH_PARAMS)
            .map_err(journaliser_refus)?;
    }
    p.validate().map_err(journaliser_refus)?;
    let TextesArgs {
        places,
        jurisdictions,
        at,
        valid_from,
    } = textes_args(&p, raw.as_deref())?;
    let context = search_context(p.context.as_deref(), surface);
    let identity = lj_identity::Identity::resolve(user_id.as_deref(), &headers);
    let budget_key = crate::ratelimit::key_of(&identity);
    let caller = usage::Caller::http(
        surface.source(),
        user_id.is_some(),
        context,
        &headers,
        &state.settings,
    )
    .pseudonymise(&identity, state.pseudonymiser.as_ref(), &headers);
    // With no query the public door lists texts from the default place, as the
    // connector does; the web page lists them from its reader's, through the cache.
    let body = match (surface, p.query.trim()) {
        (Surface::Public, "") => {
            legi::list_textes(
                &state,
                p.code.as_deref(),
                &jurisdictions,
                p.scope.as_deref(),
                valid_from,
                at,
                p.limit,
                p.offset,
                caller,
                budget_key.as_ref(),
            )
            .await?
        }
        (Surface::Public, _) => {
            legi::search_textes_counted(
                &state,
                &p.query,
                p.code.as_deref(),
                &places,
                &jurisdictions,
                p.source.as_deref(),
                p.scope.as_deref(),
                at,
                valid_from,
                p.limit,
                p.offset,
                p.mode,
                caller,
                budget_key.as_ref(),
            )
            .await?
        }
        (Surface::Internal, _) => {
            legi::search_textes(
                &state,
                &p.query,
                p.code.as_deref(),
                &places,
                &jurisdictions,
                p.source.as_deref(),
                p.scope.as_deref(),
                at,
                valid_from,
                p.limit,
                p.offset,
                p.mode,
                caller,
                budget_key.as_ref(),
            )
            .await?
        }
    };
    match surface {
        Surface::Internal => Ok(with_cache(CACHE_SEARCH, body)),
        Surface::Public => Ok(with_cache(
            CACHE_SEARCH,
            crate::mcp_presenters::present_law_search(
                &p.query,
                &body,
                &state.settings.web_base_url,
            ),
        )),
    }
}

/// Total and facets of the same parameters as `/internal/search-textes`, asked
/// after its hits: the cached answer is found again from the parameters. Records
/// no search, the list has.
async fn search_textes_decompte_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Query(p): Query<ArticleSearchQuery>,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    p.validate().map_err(journaliser_refus)?;
    let TextesArgs {
        places,
        jurisdictions,
        at,
        valid_from,
    } = textes_args(&p, raw.as_deref())?;
    let identity = lj_identity::Identity::resolve(user_id.as_deref(), &headers);
    let budget_key = crate::ratelimit::key_of(&identity);
    let counted = legi::count_textes(
        &state,
        &p.query,
        p.code.as_deref(),
        &places,
        &jurisdictions,
        p.source.as_deref(),
        p.scope.as_deref(),
        at,
        valid_from,
        p.mode,
        search_context(p.context.as_deref(), Surface::Internal),
        budget_key.as_ref(),
    )
    .await?;
    Ok(with_cache(CACHE_SEARCH, counted))
}

struct TextesArgs {
    places: Vec<String>,
    jurisdictions: Vec<String>,
    at: Option<chrono::NaiveDate>,
    valid_from: Option<chrono::NaiveDate>,
}

/// The places read from and the orders searched, both repeated: what applies in
/// any place is served, the default reader's when none is named.
fn textes_args(p: &ArticleSearchQuery, raw: Option<&str>) -> Result<TextesArgs> {
    let pairs = parse_qs(raw.unwrap_or(""));
    let repeated = |key: &str| -> Vec<String> {
        pairs
            .iter()
            .filter(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.clone())
            .collect()
    };
    let at = p.as_of.as_deref().map(parse_legi_date).transpose()?;
    let valid_from = p
        .valid_from
        .as_deref()
        .map(|d| crate::legi::query_date("valid_from", d))
        .transpose()?;
    if valid_from.is_some_and(|from| from > at.unwrap_or_else(|| chrono::Utc::now().date_naive())) {
        return Err(ApiError::Unprocessable(validation::value_error(
            &["query", "valid_from"],
            p.valid_from.as_deref().unwrap_or_default(),
            "valid_from must not be after as_of, which defaults to today",
        )));
    }
    Ok(TextesArgs {
        places: repeated("place"),
        jurisdictions: repeated("jurisdiction")
            .into_iter()
            .map(lj_norm::jurisdictions::union_as_eu)
            .collect(),
        at,
        valid_from,
    })
}

// ── Annonces ────────────────────────────────────────────────────────────────

const JOURNALS_MAX_LIMIT: u32 = 100;
const ANNONCES_MAX_OFFSET: u32 = 5_000;

/// Parse la query string du moteur d'annonces (`/api/annonces` et les flux) —
/// mêmes clés que l'URL de la page, listes en paramètres répétés
/// (`?zone=75&zone=92`). Seuls les jetons de `fond` connus passent.
/// A département code is upper-case, the two scopes that are not places are
/// lower-case, and the query string is typed by hand either way.
fn zone_canonique(valeur: &str) -> String {
    match valeur.to_lowercase().as_str() {
        bas @ ("fr" | "eu" | "sans") => bas.to_string(),
        _ => valeur.to_uppercase(),
    }
}

fn parse_journals_query(raw: &str) -> Result<lj_dtos::JournalSearchRequest> {
    let mut req = lj_dtos::JournalSearchRequest::default();
    for (key, value) in parse_qs(raw) {
        match key.as_str() {
            "q" => req.q = Some(value),
            "fond" => req.fond.push(value),
            "zone" => req.zone.push(zone_canonique(&value)),
            // L'égalité ODSQL des fonds proxy est sensible à la casse : la valeur passe
            // telle quelle.
            "famille" => req.famille.push(value),
            "cpv" => req.cpv.push(value),
            "metier" => req.metier.push(value),
            "echeanceJours" => {
                req.echeance_jours = Some(value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(
                        &["query", "echeanceJours"],
                        &value,
                    ))
                })?)
            }
            "closes" => req.closes = matches!(value.as_str(), "1" | "true" | "yes" | "on"),
            "jurisdiction" => req.jurisdiction = Some(value),
            "home" => req.home = Some(crate::jurisdictions::parse_home(&value)?),
            "sort" => {
                req.sort = de_enum::<lj_dtos::JournalSort>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "sort"],
                        &value,
                        &["echeance", "publication", "nouveautes"],
                    ))
                })?
            }
            "limit" => {
                req.limit = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &value))
                })?
            }
            "offset" => {
                req.offset = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "offset"], &value))
                })?
            }
            _ => {}
        }
    }
    if let Some(inconnu) = req
        .fond
        .iter()
        .find(|f| !journals::FONDS.contains(&f.as_str()))
    {
        return Err(ApiError::Unprocessable(validation::enum_error(
            &["query", "fond"],
            inconnu,
            &journals::FONDS,
        )));
    }
    if req.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &req.limit.to_string(),
            1,
        )));
    }
    if req.limit > JOURNALS_MAX_LIMIT {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &req.limit.to_string(),
            i64::from(JOURNALS_MAX_LIMIT),
        )));
    }
    if req.offset > ANNONCES_MAX_OFFSET {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "offset"],
            &req.offset.to_string(),
            i64::from(ANNONCES_MAX_OFFSET),
        )));
    }
    Ok(req)
}

/// Moteur facetté du pot Annonces (`GET /api/annonces`).
async fn journals_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let req = parse_journals_query(raw.as_deref().unwrap_or(""))?;
    let body = journals::search(&state, &req).await?;
    let caller = usage::Caller::http(
        ActivitySource::Web,
        user_id.is_some(),
        lj_dtos::SearchContext::User,
        &headers,
        &state.settings,
    )
    .pseudonymise(
        &lj_identity::Identity::resolve(user_id.as_deref(), &headers),
        state.pseudonymiser.as_ref(),
        &headers,
    );
    let filters = journals_filters(&req).to_string();
    caller.record_search(
        lj_dtos::SearchEngine::Annonces,
        req.q.as_deref().unwrap_or("").trim(),
        usage::Shape {
            page: usage::page_number(i64::from(req.offset), i64::from(req.limit)).unwrap_or(1),
            filters: &filters,
        },
        Some(body.total),
    );
    Ok(with_cache(CACHE_SEARCH, body))
}

/// The whole filter minus the window, plus the page number: the same keys as
/// the `/annonces` URL.
fn journals_filters(req: &lj_dtos::JournalSearchRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(req).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.remove("q");
        obj.remove("limit");
        obj.remove("offset");
        if let Some(page) = usage::page_number(i64::from(req.offset), i64::from(req.limit)) {
            obj.insert("page".to_string(), page.into());
        }
    }
    value
}

async fn journals_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = journals::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn annonce_detail(
    State(state): State<AppState>,
    Path(uid): Path<String>,
) -> Result<Response> {
    let body = journals::detail(&state, &uid).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn notice_detail(State(state): State<AppState>, Path(uid): Path<String>) -> Result<Response> {
    let body = crate::notices::notice(&state, &uid).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Alert feeds, mounted at the root: `/journal-search.rss` (new notices) and
/// `/journal-search.ics` (deadlines). An alert is a filter state, the same query
/// string as the page, no account.
fn journals_feeds_router() -> Router<AppState> {
    Router::new()
        .route("/journal-search.rss", get(journals_rss))
        .route("/journal-search.ics", get(journals_ics))
}

/// Requête d'un flux : le filtre de la page, la fenêtre forcée au plafond du flux
/// et l'ordre imposé par le média (nouveautés pour un lecteur, échéance pour un
/// agenda).
fn feed_request(raw: &str, sort: lj_dtos::JournalSort) -> Result<lj_dtos::JournalSearchRequest> {
    let mut req = parse_journals_query(raw)?;
    req.sort = sort;
    req.limit = journals::FEED_MAX_ITEMS;
    req.offset = 0;
    req.closes = false;
    Ok(req)
}

async fn journals_rss(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    let req = feed_request(raw, lj_dtos::JournalSort::Nouveautes)?;
    let hits = journals::hits_flux(&state, &req).await?;
    let base = state.settings.web_base_url.trim_end_matches('/');
    let self_url = if raw.is_empty() {
        format!("{base}/journal-search.rss")
    } else {
        format!("{base}/journal-search.rss?{raw}")
    };
    Ok(feed_response(
        journals::rss(&req, &hits, &self_url, base),
        "application/rss+xml; charset=utf-8",
    ))
}

async fn journals_ics(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Result<Response> {
    let mut req = feed_request(raw.as_deref().unwrap_or(""), lj_dtos::JournalSort::Echeance)?;
    // Un agenda ne parle que des fonds qui ouvrent un délai.
    let demandes = std::mem::take(&mut req.fond);
    req.fond = journals::FONDS
        .iter()
        .copied()
        .filter(|f| lj_dtos::journal_fond_a_echeance(f))
        .filter(|f| demandes.is_empty() || demandes.iter().any(|d| d == f))
        .map(str::to_string)
        .collect();
    // An empty list means "every fond" downstream, so it cannot reach the query.
    let hits = if req.fond.is_empty() {
        Vec::new()
    } else {
        journals::hits_flux(&state, &req).await?
    };
    let base = state.settings.web_base_url.trim_end_matches('/');
    Ok(feed_response(
        journals::ics(&req, &hits, base),
        "text/calendar; charset=utf-8",
    ))
}

/// Réponse d'un flux : corps typé, cache CDN court et `noindex`.
fn feed_response(body: String, content_type: &'static str) -> Response {
    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&CACHE_SEARCH.header_value()) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    headers.insert(
        header::HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex"),
    );
    resp
}

/// Query `/api/codes` : `scope=head` borne aux familles de tête (codes,
/// constitutions).
#[derive(Deserialize)]
struct CodesCatalogueQuery {
    #[serde(default)]
    scope: Option<String>,
}

async fn codes_catalogue(
    State(state): State<AppState>,
    Query(p): Query<CodesCatalogueQuery>,
) -> Result<Response> {
    let body = legi::code_catalogue(&state, p.scope.as_deref() == Some("head")).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn corpus_stats(State(state): State<AppState>) -> Result<Response> {
    let body = stats::corpus_stats(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn reader_jurisdictions(State(state): State<AppState>) -> Result<Response> {
    let refs = crate::referential::referential(&state).await?;
    Ok(with_cache(
        CACHE_DECISION,
        crate::jurisdictions::known(&refs),
    ))
}

async fn open_source_list() -> Response {
    with_cache(CACHE_DECISION, crate::open_source::response())
}

async fn jurisdictions_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = jurisdiction_hubs::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn jurisdiction_hub(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = jurisdiction_hubs::hub(&state, &code).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Query `/api/juridictions/{code}/{annee}` : `page` ≥ 1 (défaut 1).
#[derive(Deserialize)]
struct JurisdictionYearQuery {
    #[serde(default)]
    page: Option<u32>,
}

async fn jurisdiction_year(
    State(state): State<AppState>,
    Path((code, annee)): Path<(String, i32)>,
    Query(p): Query<JurisdictionYearQuery>,
) -> Result<Response> {
    let body = jurisdiction_hubs::year_page(&state, &code, annee, p.page.unwrap_or(1)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn normes_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = norm_hubs::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn norm_hub(
    State(state): State<AppState>,
    Path((juris, fond)): Path<(String, String)>,
) -> Result<Response> {
    let body = norm_hubs::hub(&state, &juris, &fond).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn norm_year(
    State(state): State<AppState>,
    Path((juris, fond, annee)): Path<(String, String, String)>,
    Query(p): Query<JurisdictionYearQuery>,
) -> Result<Response> {
    let year = match annee.as_str() {
        norm_hubs::UNDATED_TOKEN => None,
        y => Some(y.parse::<i32>().map_err(|_| ApiError::NotFound)?),
    };
    let body = norm_hubs::year_page(&state, &juris, &fond, year, p.page.unwrap_or(1)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// The query of `/api/search-textes`: `query`, `code` (a slug), the
/// `jurisdiction`/`nature`/`source` facets, `as_of` (ISO `YYYY-MM-DD`, the
/// versions valid on that date, today's when absent), `valid_from` (ISO, the
/// earliest entry into force of those versions), `limit` 1–100, `offset` 0–99.
#[derive(Debug, Deserialize)]
struct ArticleSearchQuery {
    #[serde(default)]
    query: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    as_of: Option<String>,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    mode: lj_dtos::SearchMode,
    /// `teaser` = fetch machine des ponts croisés : pas d'historique. Absent ou autre
    /// valeur = recherche utilisateur.
    #[serde(default)]
    context: Option<String>,
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
        if self.offset > 99 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                99,
            )));
        }
        Ok(())
    }
}

/// Parse le segment `{date}` d'URL (ISO `YYYY-MM-DD`). Format invalide → 422 à la
/// frontière HTTP.
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

// ── Clés de la surface publique ─────────────────────────────────────────────

async fn list_api_keys(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    let items = crate::api_keys::list(&state, &sub).await?;
    Ok(Json(lj_dtos::ApiKeysResponse { items }).into_response())
}

/// Le secret n'est lisible que dans cette réponse : il n'est pas stocké en
/// clair, donc rien ne le resservira.
async fn create_api_key(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Json(body): Json<lj_dtos::ApiKeyCreate>,
) -> Result<Response> {
    let label = body.label.trim();
    if label.is_empty() || label.chars().count() > 64 {
        return Err(ApiError::BadRequest("libelle_invalide".into()));
    }
    let cree = crate::api_keys::create(&state, &sub, label).await?;
    Ok((StatusCode::CREATED, Json(cree)).into_response())
}

async fn revoke_api_key(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(key_id): Path<String>,
) -> Result<Response> {
    if crate::api_keys::revoke(&state, &sub, &key_id).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The kinds the client's beacon sends, by name: one the server does not
    /// read is a 422 nobody sees.
    #[test]
    fn every_kind_the_client_sends_is_read() {
        for sent in [
            "hydration",
            "hydration_ok",
            "route_404",
            "recherche_refusee",
            "stylesheet_missing",
        ] {
            let kind: ClientIncidentKind = serde_json::from_value(json!(sent)).expect(sent);
            assert_eq!(kind.as_str(), sent);
        }
    }

    /// An axis declared in the DTO, filtered by the store and offered as a facet
    /// still does nothing until this parser reads its key.
    #[test]
    fn chaque_axe_multivalue_de_l_url_atteint_la_requete() {
        let req = parse_journals_query(
            "fond=marche&zone=973&famille=TRAVAUX&cpv=45&cpv=nature_marche:SERVICES&metier=etudes",
        )
        .expect("query");
        assert_eq!(req.fond, ["marche"]);
        assert_eq!(req.zone, ["973"]);
        assert_eq!(req.famille, ["TRAVAUX"]);
        assert_eq!(req.cpv, ["45", "nature_marche:SERVICES"]);
        assert_eq!(req.metier, ["etudes"]);
    }

    #[test]
    fn the_directory_lists_every_kind_but_the_natural_persons() {
        for kind in ENTITY_KINDS {
            let category = entities::kind_to_category(kind).expect("known kind");
            assert_eq!(
                DIRECTORY_KINDS.contains(kind),
                !entities::names_natural_persons(category),
                "{kind} is listable and a natural person, or neither"
            );
        }
    }

    #[test]
    fn search_query_rejects_malformed_date() {
        let err = parse_search_query("query=x&date_from=not-a-date", false).unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable, eu {err:?}");
        };
        assert_eq!(
            obj,
            json!({
                "type": "date_from_datetime_parsing",
                "loc": ["query", "date_from"],
                "msg": "Input should be a valid date or datetime, invalid character in year",
                "input": "not-a-date",
                "ctx": { "error": "invalid character in year" }
            })
        );
    }

    #[test]
    fn search_query_rejects_out_of_range_date() {
        let err = parse_search_query("query=x&date_to=2300-01-01", false).unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["type"], json!("less_than_equal"));
        assert_eq!(obj["loc"], json!(["query", "date_to"]));
    }

    #[test]
    fn search_query_accepts_valid_date() {
        let req =
            parse_search_query("query=x&date_from=2024-01-01&date_to=2024-12-31", false).unwrap();
        assert_eq!(req.date_from.as_deref(), Some("2024-01-01"));
        assert_eq!(req.date_to.as_deref(), Some("2024-12-31"));
    }

    #[test]
    fn search_query_rejects_unknown_enum_token() {
        let err = parse_search_query("query=x&solution=JAF", false).unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable, eu {err:?}");
        };
        assert_eq!(obj["type"], json!("enum"));
        assert_eq!(obj["loc"], json!(["query", "solution"]));
        assert!(obj["msg"].as_str().unwrap().contains("'DISMISSED'"));

        let err = parse_search_query("query=x&jurisdiction_type=TRIBUNAL", false).unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["loc"], json!(["query", "jurisdiction_type"]));
        assert!(obj["msg"].as_str().unwrap().contains("'TA'"));
    }

    #[test]
    fn search_query_accepts_valid_enum_tokens() {
        let req = parse_search_query(
            "query=x&solution=DISMISSED&jurisdiction_type=TA&formation=FR.TA.JUGE_REFERES",
            false,
        )
        .unwrap();
        assert_eq!(req.solution.as_deref().map(<[_]>::len), Some(1));
        assert_eq!(
            req.formation.as_deref(),
            Some(&["FR.TA.JUGE_REFERES".to_string()][..])
        );
    }

    /// Both search routes stop paging at the same rank: the public surface is a
    /// search, not an export.
    #[test]
    fn both_search_routes_cap_offset_at_99() {
        let deepest = |offset: i64| ArticleSearchQuery {
            query: "x".to_string(),
            offset,
            limit: 20,
            code: None,
            source: None,
            scope: None,
            as_of: None,
            valid_from: None,
            mode: lj_dtos::SearchMode::Auto,
            context: None,
        };
        deepest(99).validate().unwrap();
        let ApiError::Unprocessable(obj) = deepest(100).validate().unwrap_err() else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["type"], json!("less_than_equal"));
        assert_eq!(obj["loc"], json!(["query", "offset"]));

        parse_search_query("query=x&offset=99", false).unwrap();
        let ApiError::Unprocessable(obj) =
            parse_search_query("query=x&offset=100", false).unwrap_err()
        else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["type"], json!("less_than_equal"));
        assert_eq!(obj["loc"], json!(["query", "offset"]));
    }

    /// The public doors refuse what their contract does not name, so the two
    /// lists are one.
    #[test]
    fn the_public_doors_take_the_parameters_openapi_names() {
        let spec: serde_json::Value =
            serde_json::from_str(include_str!("../../lj-web/public/openapi.json")).unwrap();
        let named = |path: &str| -> std::collections::BTreeSet<String> {
            spec["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p["in"] == "query")
                .filter_map(|p| p["name"].as_str().map(str::to_string))
                .collect()
        };
        let ours = |known: &[&str]| known.iter().map(|k| k.to_string()).collect();
        assert_eq!(named("/api/search"), ours(&PUBLIC_SEARCH_PARAMS));
        assert_eq!(
            named("/api/search-textes"),
            ours(&PUBLIC_NORM_SEARCH_PARAMS)
        );
        assert_eq!(
            named("/api/norm/{jurisdiction}/{code}/{article}"),
            ours(&PUBLIC_NORM_PARAMS)
        );
        assert_eq!(named("/api/article/{uid}"), ours(&PUBLIC_NORM_PARAMS));
        let refused =
            refuse_unknown_params("query=bail&foo=1&limit=3&bar=2", &PUBLIC_SEARCH_PARAMS)
                .unwrap_err();
        let ApiError::Unprocessable(objs) = refused else {
            panic!("expected Unprocessable");
        };
        assert_eq!(objs[0]["loc"], json!(["query", "foo"]));
        assert_eq!(objs[1]["loc"], json!(["query", "bar"]));
    }

    #[test]
    fn the_former_rest_names_are_read_under_the_current_ones() {
        assert_eq!(
            rename_legacy_params(
                "q=bail+commercial&dateFrom=2020-01-01&limit=3",
                &LEGACY_SEARCH_PARAMS
            ),
            Some((
                "query=bail+commercial&date_from=2020-01-01&limit=3".to_string(),
                vec!["q", "dateFrom"]
            ))
        );
        assert_eq!(
            rename_legacy_params("date=2020-01-01", &LEGACY_NORM_PARAMS),
            Some(("as_of=2020-01-01".to_string(), vec!["date"]))
        );
        assert_eq!(
            rename_legacy_params("query=bail", &LEGACY_SEARCH_PARAMS),
            None
        );
        for (_, new) in LEGACY_SEARCH_PARAMS {
            assert!(PUBLIC_SEARCH_PARAMS.contains(&new), "{new}");
        }
        for (_, new) in LEGACY_NORM_PARAMS {
            assert!(PUBLIC_NORM_SEARCH_PARAMS.contains(&new), "{new}");
        }
    }

    #[test]
    fn the_public_door_reranks_unless_refused() {
        assert!(parse_search_query("query=x", true).unwrap().ai_mode);
        assert!(
            !parse_search_query("query=x&ai_rerank=false", true)
                .unwrap()
                .ai_mode
        );
        assert!(!parse_search_query("query=x", false).unwrap().ai_mode);
    }

    #[test]
    fn the_teaser_marker_is_ignored_on_the_public_surface() {
        use lj_dtos::SearchContext::{Teaser, User};
        assert_eq!(search_context(Some("teaser"), Surface::Internal), Teaser);
        assert_eq!(search_context(Some("teaser"), Surface::Public), User);
        assert_eq!(search_context(None, Surface::Internal), User);
        assert_eq!(search_context(Some("Teaser"), Surface::Internal), User);
    }

    /// `/norm/fr/code-civil/1240` and `/norm/code-civil/1240/2020-01-01` have
    /// the same arity: the detached segment decides which one the router reads,
    /// and it only decides it if it runs first.
    #[tokio::test]
    async fn the_detached_segment_reaches_the_router_before_the_matching() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt as _;

        async fn article(
            juris: Option<Extension<UrlJurisdiction>>,
            Path((code, num)): Path<(String, String)>,
        ) -> String {
            format!("{}|{code}|{num}", url_juris(&juris).unwrap_or("-"))
        }

        let app: Router = Router::new().nest_service(
            "/norm",
            middleware::from_fn(detach_jurisdiction)
                .layer(Router::new().route("/{code}/{num}", get(article))),
        );

        let lu = |chemin: &'static str| {
            let app = app.clone();
            async move {
                let reponse = app
                    .oneshot(
                        HttpRequest::builder()
                            .uri(chemin)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let corps = axum::body::to_bytes(reponse.into_body(), 1024)
                    .await
                    .unwrap();
                String::from_utf8(corps.to_vec()).unwrap()
            }
        };

        assert_eq!(lu("/norm/fr/code-civil/1240").await, "FR|code-civil|1240");
        assert_eq!(lu("/norm/eu/rgpd/17").await, "EU|rgpd|17");
        assert_eq!(lu("/norm/code-civil/1240").await, "-|code-civil|1240");
    }
}
