//! Endpoint MCP (port de `mcp_server.py`).
//!
//! Serveur MCP distant standard (Streamable HTTP, JSON response) montable dans
//! l'app axum sous `/mcp`. Outils exposés : `search_decisions`, `get_decision`,
//! `list_my_activity`.
//!
//! rmcp 0.1.5 n'embarque pas de transport *Streamable HTTP* côté serveur (seul
//! SSE est fourni) ; on porte donc la couche JSON-RPC à la main au-dessus
//! d'axum, conformément à la note du contrat (« si rmcp insuffisant : transport
//! JSON-RPC/HTTP manuel »). Les corps/sorties d'outils sont assurés par
//! [`crate::mcp_presenters`] ; la logique métier par [`crate::search`] /
//! [`crate::decisions`].

use axum::{
    extract::{Request, State},
    http::header::{AUTHORIZATION, WWW_AUTHENTICATE},
    http::{HeaderMap, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use serde_json::{json, Value};
use tracing::Instrument;

use crate::bookmarks::fetch_bookmarks;
use crate::decision_views::{fetch_views, record_decision_view};
use crate::error::ApiError;
use crate::mcp_presenters::{
    present_bookmarks, present_decision_detail, present_law_article, present_law_search,
    present_reading_history, present_saved_searches, present_search_response, McpActivityResponse,
};
use crate::search_history::{fetch_history, record_search};
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "librejustice";
/// `serverInfo.version` : version PROPRE du serveur (semver, alignée sur la
/// version du crate), conforme au versioning du registre MCP
/// (modelcontextprotocol.io/registry/versioning — « align server version with
/// package version »). On NE miroite PAS la version du SDK `mcp` Python (1.27.0),
/// qui fuite la version de la lib et non celle du serveur : déviation assumée
/// vs l'oracle sur ce champ (le reste de l'enveloppe reste à parité).
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Instructions serveur (port verbatim de `FastMCP(instructions=...)`).
const SERVER_INSTRUCTIONS: &str =
    "Search French and European case law: all French courts plus, among \
others, the CNDA, the Conseil constitutionnel, CNIL sanctions, the \
CEDH and the CJUE. The engine understands meaning, not just keywords: \
phrase the query as a natural question or a list of descriptive terms; \
synonyms and reformulations are handled. Typical workflow: \
search_decisions for a shortlist, then get_decision to read candidates \
in full. Previews only orient: never judge relevance, quote, or state \
what a decision holds from one, as the matched passage may be a \
party's argument, not the court's ruling. Iterate by refining the \
query or filters. Open any `url` with the matching tool: /decision/ \
links with get_decision, /texte/ links with get_legal_text. When \
mentioning a decision, hyperlink its `title` (or a conventional \
citation) to its `url`.";

/// Construit le service MCP adossé à l'état app.
///
/// rmcp 0.1.5 ne fournit pas de transport Streamable HTTP serveur ; le montage
/// effectif passe par [`mcp_router`] (router axum). Cette fonction reste le
/// point de validation/initialisation prévu par le contrat ; elle vérifie que
/// l'état est exploitable et renvoie `Ok(())`.
pub fn build_mcp_service(_state: AppState) -> anyhow::Result<()> {
    Ok(())
}

/// Router axum exposant l'endpoint MCP (JSON-RPC Streamable HTTP), à `.merge()`
/// dans [`crate::routes::create_app`] quand `enable_mcp`.
///
/// Le handler est servi sur `/mcp/` **et** `/mcp` : les clients MCP (ChatGPT,
/// Claude.ai) POSTent l'URL telle que saisie par l'utilisateur et ne suivent
/// pas de redirection sur POST — un 4xx sur `initialize` les fait basculer en
/// sonde SSE legacy (GET), qui échoue aussi (« MCP SSE probe returned 405 »).
pub fn mcp_router(state: AppState) -> Router {
    Router::new()
        .route("/mcp/", post(handle_rpc))
        .route("/mcp", post(handle_rpc))
        .layer(from_fn_with_state(state.clone(), mcp_auth))
        .with_state(state)
}

// ── Auth MCP (port de `_McpAuthMiddleware`) ──────────────────────────────────

/// Utilisateur MCP résolu, porté dans les extensions de requête. `None` quand
/// l'auth MCP est désactivée (`mcp_require_auth=false`, tests / dev local) —
/// parité avec `_mcp_user_id` qui rend `None` si le middleware n'a rien posé.
#[derive(Debug, Clone)]
struct McpUser(Option<String>);

/// Réponse 401 avec le challenge `WWW-Authenticate` (RFC 9728 + MCP 2025-03-26)
/// qui déclenche l'auto-discovery OAuth chez Claude.ai / ChatGPT (port verbatim
/// du corps `{"error": …}` + en-tête de `_McpAuthMiddleware`).
fn mcp_unauthorized(challenge: &str, error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, challenge.to_string())],
        Json(json!({ "error": error })),
    )
        .into_response()
}

/// Résout le `user_id` d'un access token MCP via la table `mcp_tokens`
/// (port de la requête du middleware Python). `None` si le token est absent de
/// la table ou expiré.
async fn lookup_mcp_user(state: &AppState, token: &str) -> Result<Option<String>, ApiError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let row = conn
        .query_opt(
            "SELECT user_id FROM mcp_tokens WHERE access_token = $1 AND expires_at > now()",
            &[&token],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("mcp token lookup: {e}")))?;
    Ok(row.map(|r| r.get::<_, String>(0)))
}

/// Bump `users.last_seen_at` pour l'activité MCP (best-effort). Le user MCP est
/// résolu via `mcp_tokens`, hors du chemin Supabase qui bumpe déjà `last_seen_at` ;
/// sans ce bump, le champ est aveugle à l'usage MCP.
async fn bump_last_seen(state: &AppState, user_id: &str) {
    let Ok(conn) = state.pool.get().await else {
        return;
    };
    if let Err(e) = conn
        .execute(
            "UPDATE users SET last_seen_at = now() WHERE sub = $1",
            &[&user_id],
        )
        .await
    {
        tracing::warn!("bump last_seen (mcp) failed: {e}");
    }
}

/// Middleware d'auth MCP : si `mcp_require_auth`, vérifie le Bearer token contre
/// `mcp_tokens` et résout le `user_id` (401 + challenge sinon) ; sinon laisse
/// passer en anonyme. Le `user_id` résolu (ou `None`) est injecté en extension
/// pour les handlers d'outils. Parité stricte avec `_McpAuthMiddleware`.
async fn mcp_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mut user: Option<String> = None;
    if state.settings.mcp_require_auth {
        let base = crate::oauth::request_base_url(
            req.uri(),
            req.headers(),
            &state.settings.public_base_url,
        );
        let challenge = format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
        );
        let auth = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let Some(token) = auth.strip_prefix("Bearer ") else {
            return mcp_unauthorized(&challenge, "auth_required");
        };
        match lookup_mcp_user(&state, token).await {
            Ok(Some(uid)) => {
                bump_last_seen(&state, &uid).await;
                user = Some(uid);
            }
            Ok(None) => return mcp_unauthorized(&challenge, "invalid_token"),
            Err(e) => return e.into_response(),
        }
    }
    req.extensions_mut().insert(McpUser(user));
    next.run(req).await
}

// ── Couche JSON-RPC ──────────────────────────────────────────────────────────

/// Catégorie machine-lisible du contrat d'erreur outil : dit au modèle la
/// nature de l'échec, donc la conduite à tenir (corriger l'appel, chercher
/// autrement, se connecter, retenter).
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// Arguments invalides : corriger l'appel, un retry à l'identique échouera.
    Validation,
    /// Absent du corpus — ce qui ne prouve jamais que la ressource n'existe pas.
    NotFound,
    /// Authentification requise.
    Auth,
    /// Panne interne ou transitoire : un retry à l'identique peut réussir.
    Internal,
}

impl ErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::Auth => "auth",
            Self::Internal => "internal",
        }
    }
}

/// Erreur JSON-RPC remontée comme `error` dans l'enveloppe (codes standard MCP)
/// ou, pour un échec d'outil, rendue en contrat d'erreur structuré par
/// [`tool_error_content`] : `category` + `retryable` + `hint` (prochaine action).
#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
    category: ErrorCategory,
    /// `true` si un retry à l'identique peut réussir (panne transitoire).
    retryable: bool,
    /// Prochaine action suggérée au modèle (outil à essayer, correction).
    hint: Option<String>,
}

impl RpcError {
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            category: ErrorCategory::Validation,
            retryable: false,
            hint: None,
        }
    }
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            category: ErrorCategory::Validation,
            retryable: false,
            hint: None,
        }
    }
    /// Erreur applicative outil (équivalent `ToolError` côté Python) : code
    /// d'erreur interne MCP, retryable (pool, upstream — transitoire a priori).
    fn tool_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Internal,
            retryable: true,
            hint: None,
        }
    }
    /// Ressource absente du corpus : non-retryable, et le `hint` doit rappeler
    /// que l'absence du corpus ne prouve pas l'inexistence.
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::NotFound,
            retryable: false,
            hint: None,
        }
    }
    /// Authentification requise (outils personnels).
    fn auth(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Auth,
            retryable: false,
            hint: None,
        }
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Point d'entrée HTTP : décode l'enveloppe JSON-RPC, route vers la méthode.
async fn handle_rpc(
    State(state): State<AppState>,
    Extension(McpUser(user)): Extension<McpUser>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    let session = mcp_session_hash(&headers);

    // Notifications (pas d'`id`) : on accuse réception sans corps (202).
    let is_notification = body.get("id").is_none();

    match dispatch(&state, method, params, user.as_deref(), session.as_deref()).await {
        Ok(result) => {
            if is_notification {
                return StatusCode::ACCEPTED.into_response();
            }
            Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
        }
        Err(err) => {
            if is_notification {
                return StatusCode::ACCEPTED.into_response();
            }
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": err.code, "message": err.message},
            }))
            .into_response()
        }
    }
}

/// Corrélateur de session MCP (ADR 0252) : hash court, non réversible
/// (`DefaultHasher`), du header `Mcp-Session-Id` — jeton **par conversation**,
/// éphémère, jamais l'identité de l'utilisateur (borne ADR 0039). Absent si le
/// client n'envoie pas de session id → pas de regroupement, aucune PII.
fn mcp_session_hash(headers: &HeaderMap) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let raw = headers.get("mcp-session-id")?.to_str().ok()?;
    if raw.is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish()))
}

/// Routage des méthodes MCP standard.
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(state, params, user, session).await,
        // Resources : liste vide assumée. La primitive est application-controlled
        // (Claude Desktop exige une sélection manuelle, ChatGPT ne les lit pas) ;
        // pour exposer des données au modèle, la voie est un tool (cf. ADR 0244).
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
        "prompts/get" => get_prompt(&params),
        other => Err(RpcError::method_not_found(other)),
    }
}

/// Réponse d'`initialize` : capacités serveur + métadonnées d'implémentation.
fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        // Capacités émises verbatim par le SDK `mcp` côté serveur (defaults
        // `ServerCapabilities`) : parité d'enveloppe avec FastMCP.
        "capabilities": {
            "experimental": {},
            "prompts": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false},
            "tools": {"listChanged": false},
        },
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "instructions": SERVER_INSTRUCTIONS,
    })
}

// ── Outils : appel ───────────────────────────────────────────────────────────

/// Dispatch `tools/call` : décode `name` + `arguments`, exécute l'outil, emballe
/// le résultat structuré dans `content` (texte JSON) + `structuredContent`.
async fn call_tool(
    state: &AppState,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Span parent d'appel d'outil (ADR 0252) : attributs domaine RGPD-safe —
    // booléen connecté/anonyme, jamais l'identité (borne ADR 0039). Enveloppe
    // l'exécution : les spans store fan-out (`article_at_date`,
    // `similar_decisions`…) deviennent descendants et partagent le `traceID` ;
    // `mcp.session` regroupe les appels d'une même conversation.
    let span = tracing::info_span!(
        "mcp_tool_call",
        librejustice.mcp.tool = name,
        librejustice.mcp.authenticated = user.is_some(),
        librejustice.mcp.session = tracing::field::Empty,
        librejustice.mcp.arg = tracing::field::Empty,
    );
    if let Some(s) = session {
        span.record("librejustice.mcp.session", s);
    }
    // Argument compact, tronqué — non-PII (url de décision, ref d'article + date ;
    // la query de recherche est déjà exportée par le span `search`).
    let arg: String = arguments.to_string().chars().take(256).collect();
    span.record("librejustice.mcp.arg", arg.as_str());

    async move {
        // Sémantique MCP : un échec d'EXÉCUTION d'outil (outil inconnu ou erreur
        // pendant l'appel) n'est PAS une erreur JSON-RPC mais un *résultat* avec
        // `isError: true` et le message dans `content` (parité SDK `mcp`). Seules
        // les erreurs de PROTOCOLE (name manquant) restent des erreurs JSON-RPC.
        let result = match name {
            "search_decisions" => tool_search_decisions(state, arguments, user).await,
            "get_decision" => tool_get_decision(state, arguments, user).await,
            "list_my_activity" => tool_list_my_activity(state, arguments, user).await,
            "get_legal_text" => tool_get_legal_text(state, arguments).await,
            "search_legal_texts" => tool_search_legal_texts(state, arguments, user).await,
            other => {
                return Ok(tool_error_content(
                    &RpcError::invalid_params(format!("Unknown tool: {other}")).with_hint(
                        "Valid tools: search_decisions, get_decision, list_my_activity, \
                         get_legal_text, search_legal_texts.",
                    ),
                ))
            }
        };

        match result {
            Ok((text, structured)) => Ok(json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
                "isError": false,
            })),
            Err(e) => Ok(tool_error_content(&e)),
        }
    }
    .instrument(span)
    .await
}

/// Résultat `tools/call` en erreur applicative : `isError: true` + contrat
/// d'erreur structuré en `content` texte (sémantique MCP, pas une erreur
/// JSON-RPC). `category` dit la nature de l'échec, `retryable` si un retry à
/// l'identique a un sens, `hint` la prochaine action — pour qu'un modèle
/// corrige son appel au lieu de re-tenter à l'aveugle ou de conclure à tort.
fn tool_error_content(err: &RpcError) -> Value {
    let mut body = json!({
        "error": err.message,
        "category": err.category.as_str(),
        "retryable": err.retryable,
    });
    if let Some(hint) = &err.hint {
        body["hint"] = json!(hint);
    }
    json!({
        "content": [{"type": "text", "text": body.to_string()}],
        "isError": true,
    })
}

/// Sérialise une sortie d'outil en `(content texte, structuredContent)` —
/// même JSON dans les deux canaux (le bloc texte est exigé par la spec pour
/// les clients sans `structuredContent`), en compact : le bloc texte est ce
/// que les hôtes injectent au modèle, chaque octet compte.
fn tool_ok<T: serde::Serialize>(value: &T) -> Result<(String, Value), RpcError> {
    let text = serde_json::to_string(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    let structured =
        serde_json::to_value(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    Ok((text, structured))
}

async fn tool_search_decisions(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let mut req = build_search_request(&args)?;
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    // Un code hors référentiel matcherait silencieusement zéro décision, et le
    // flux MCP n'expose pas la facette juridiction pour le découvrir.
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(unknown_jurisdiction_code(bad, &refs));
    }
    // Même garantie pour les axes à catégorie contrôlée : un token hors
    // vocabulaire matcherait silencieusement zéro décision.
    for (field, values) in [("chamber", &req.chamber), ("publication", &req.publication)] {
        if let Some(bad) = values
            .as_deref()
            .and_then(|vs| refs.find_unknown_token(field, vs))
        {
            return Err(RpcError::invalid_params(format!(
                "unknown {field} '{bad}'; valid values: {}",
                refs.facet_tokens(field).join(", ")
            )));
        }
    }
    // Filtres instrument/article : les colonnes portent des uids de catalogue
    // internes, pas des noms — un nom passé tel quel matcherait silencieusement
    // zéro. Chaque valeur (slug ou nom libre) est résolue via le slug ;
    // inconnue → erreur corrective avec les slugs les plus proches.
    resolve_instrument_filters(state, &mut req).await?;
    let response = crate::search::search(
        state,
        &req,
        lj_dtos::ActivitySource::Mcp,
        user.is_some(),
        lj_dtos::SearchContext::User,
    )
    .await
    .map_err(|e| RpcError::tool_error(e.to_string()))?;
    // Persistance d'activité best-effort (source `mcp`) : enregistrée uniquement
    // si un utilisateur MCP est résolu.
    if let Some(user_id) = user {
        record_search(
            &state.pool,
            user_id,
            &req.query,
            crate::search_history::filters_from_request(&req),
            lj_dtos::ActivitySource::Mcp,
            lj_dtos::SearchEngine::Decisions,
        )
        .await;
    }
    tool_ok(&present_search_response(
        &response,
        &state.settings.web_base_url,
    ))
}

async fn tool_get_decision(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("missing tool argument: url"))?;
    // La clé est le dernier segment de l'URL publique `/decision/{clé}`.
    let id = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("not a decision url: {url}")))?;
    let detail = crate::decisions::get_decision(state, id)
        .await
        .map_err(|e| match e {
            ApiError::NotFound => RpcError::not_found(format!("no decision at url={url}"))
                .with_hint(
                    "Decision urls cannot be composed or guessed: take the url verbatim \
                     from a search_decisions hit or an inline citation link. If this one \
                     did come from a hit, the decision may have been unpublished since.",
                ),
            other => RpcError::tool_error(other.to_string()),
        })?;
    // Consultation tracée best-effort (source `mcp`) quand un utilisateur MCP
    // est résolu.
    if let Some(user_id) = user {
        record_decision_view(&state.pool, user_id, id, lj_dtos::ActivitySource::Mcp).await;
    }
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    tool_ok(&present_decision_detail(
        &detail,
        &state.settings.web_base_url,
        &refs,
    ))
}

/// `list_my_activity` : agrège les tranches demandées (recherches/signets/
/// lectures). Personnel → exige un utilisateur MCP authentifié (parité
/// `_require_mcp_user` : message verbatim, remonté en `isError` par `call_tool`).
async fn tool_list_my_activity(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let user_id = user.ok_or_else(|| {
        RpcError::auth(
            "authentication required: connect your LibreJustice account to this MCP \
             connector to access your activity.",
        )
    })?;
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("all");
    if !matches!(kind, "searches" | "bookmarks" | "readingHistory" | "all") {
        return Err(RpcError::invalid_params(format!("invalid kind: {kind}")));
    }
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
    let web = &state.settings.web_base_url;
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;

    let mut out = McpActivityResponse::default();
    if matches!(kind, "searches" | "all") {
        let (entries, _) = fetch_history(&state.pool, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.searches = Some(present_saved_searches(&entries));
    }
    if matches!(kind, "bookmarks" | "all") {
        let (items, _) = fetch_bookmarks(&state.pool, &refs, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.bookmarks = Some(present_bookmarks(&items, web, &refs));
    }
    if matches!(kind, "readingHistory" | "all") {
        let (views, _) = fetch_views(&state.pool, &refs, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.reading_history = Some(present_reading_history(&views, web, &refs));
    }
    tool_ok(&out)
}

// ── Outil law-at-date (ADR 0097) ──────────────────────────────────────────────

/// `get_legal_text` : article de référentiel à une date (ou en vigueur si
/// `date` absente), par `url` `/texte/{code}/{article}`. La réponse inclut la
/// timeline des versions (champ `versions`). 404 (code/article inconnu) →
/// `isError`.
async fn tool_get_legal_text(state: &AppState, args: Value) -> Result<(String, Value), RpcError> {
    let url = required_str(&args, "url")?;
    let date: Option<String> = deser(args.get("date").filter(|v| !v.is_null()).cloned())?;

    // `/loi/` accepté en entrée : des conversations MCP passées portent des
    // liens à l'ancien schéma (renommage `/loi` → `/texte` 2026-07) — le tool
    // parse la chaîne, le 308 HTTP ne le couvre pas.
    let mut segs = url
        .split("/texte/")
        .nth(1)
        .or_else(|| url.split("/loi/").nth(1))
        .unwrap_or_default()
        .trim_end_matches('/')
        .splitn(2, '/');
    let (code, num) = match (segs.next().filter(|s| !s.is_empty()), segs.next()) {
        (Some(code), Some(num)) => (code.to_string(), num.to_string()),
        _ => {
            return Err(RpcError::invalid_params(format!(
                "not an article url (expected …/texte/{{code}}/{{article}}): {url}"
            )))
        }
    };
    let num = lj_core::article_key::article_key(&num);

    // L'alphabet servi par `/texte` est le slug : un nom libre (« Code civil »)
    // est slugifié ; inconnu → erreur avec les slugs les plus proches.
    let code = {
        let conn = state
            .pool
            .get()
            .await
            .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let slug = law_slug(&code);
        if !repo
            .law_slug_exists(&slug)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?
        {
            return Err(unknown_instrument(&repo, "code", &code).await);
        }
        slug
    };

    let detail = match date {
        Some(d) => {
            let parsed = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                .map_err(|_| RpcError::invalid_params(format!("invalid date: {d}")))?;
            crate::legi::article_at_date(state, &code, &num, parsed).await
        }
        None => crate::legi::article_in_force(state, &code, &num).await,
    }
    .map_err(map_legi_error)?;
    tool_ok(&present_law_article(&detail, &state.settings.web_base_url))
}

/// `search_legal_texts` : recherche plein-texte d'articles de référentiel
/// (ADR 0114), filtrable par `code`/`jurisdiction`. Renvoie le `total` exact et
/// une page de hits (extrait surligné + `url` + `num`), à chaîner ensuite vers
/// `get_legal_text` pour le texte complet à date.
async fn tool_search_legal_texts(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let query = required_str(&args, "query")?;
    if query.chars().count() > 512 {
        return Err(RpcError::invalid_params(
            "query: String should have at most 512 characters",
        ));
    }
    let opt = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    // `limit` : `Field(ge=1, le=20)`, défaut 10 (aligné sur `search_decisions`).
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(10);
    if !(1..=20).contains(&limit) {
        return Err(RpcError::invalid_params(
            "limit: Input should be between 1 and 20",
        ));
    }

    // Même alphabet que `get_legal_text` : nom libre slugifié, inconnu →
    // erreur corrective avec les slugs les plus proches.
    let code = match opt("code") {
        Some(c) => {
            let conn = state
                .pool
                .get()
                .await
                .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
            let repo = lj_store::repository::DecisionRepository::new(&conn);
            let slug = law_slug(c);
            if !repo
                .law_slug_exists(&slug)
                .await
                .map_err(|e| RpcError::tool_error(e.to_string()))?
            {
                return Err(unknown_instrument(&repo, "code", c).await);
            }
            Some(slug)
        }
        None => None,
    };

    let response = crate::legi::search_textes(
        state,
        &query,
        code.as_deref(),
        opt("jurisdiction"),
        None,
        None,
        None,
        limit,
        0,
    )
    .await
    .map_err(map_legi_error)?;

    // Persistance d'activité best-effort (ADR 0251, moteur `textes`) — même
    // règle que `search_decisions` ; `get_legal_text` (lookup) reste hors
    // historique.
    if let Some(user_id) = user {
        let mut filters = serde_json::Map::new();
        if let Some(c) = code.as_deref() {
            filters.insert("code".to_string(), Value::String(c.to_string()));
        }
        if let Some(j) = opt("jurisdiction") {
            filters.insert("jurisdiction".to_string(), Value::String(j.to_string()));
        }
        record_search(
            &state.pool,
            user_id,
            &query,
            Value::Object(filters),
            lj_dtos::ActivitySource::Mcp,
            lj_dtos::SearchEngine::Textes,
        )
        .await;
    }

    tool_ok(&present_law_search(
        &query,
        &response,
        &state.settings.web_base_url,
    ))
}

/// Erreur pour un `jurisdiction_code` hors référentiel, avec suggestions
/// (message construit par [`crate::referential::unknown_jurisdiction_code_msg`]).
fn unknown_jurisdiction_code(code: &str, refs: &crate::referential::Referential) -> RpcError {
    RpcError::invalid_params(crate::referential::unknown_jurisdiction_code_msg(
        code, refs,
    ))
}

/// Slug d'un nom de texte légal, dans l'alphabet des `legal_text.slug` :
/// minuscules, accents pliés, toute séquence non alphanumérique réduite à `-`.
fn law_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.to_lowercase().chars() {
        let folded = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'î' | 'ï' => 'i',
            'ó' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'œ' => {
                out.push('o');
                'e'
            }
            c => c,
        };
        if folded.is_ascii_alphanumeric() {
            out.push(folded);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Résout les filtres `legal_instrument` / `legal_article` (slug ou nom libre,
/// slugifié) vers l'alphabet des colonnes (`text_uid` de catalogue, numéro en
/// forme composite). Valeur inconnue → erreur corrective avec les slugs les
/// plus proches — jamais un filtre qui matche silencieusement zéro.
async fn resolve_instrument_filters(
    state: &AppState,
    req: &mut lj_dtos::SearchRequest,
) -> Result<(), RpcError> {
    if req.legal_instrument.is_none() && req.legal_article.is_none() {
        return Ok(());
    }
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    if let Some(values) = req.legal_instrument.as_mut() {
        for v in values.iter_mut() {
            match repo
                .resolve_instrument_uid(&law_slug(v))
                .await
                .map_err(|e| RpcError::tool_error(e.to_string()))?
            {
                Some(uid) => *v = uid,
                None => return Err(unknown_instrument(&repo, "legal_instrument", v).await),
            }
        }
    }
    if let Some(values) = req.legal_article.as_mut() {
        for v in values.iter_mut() {
            let Some((instrument, num)) = v.split_once('|') else {
                return Err(RpcError::invalid_params(format!(
                    "legal_article '{v}' must be \"<instrument>|<article>\" \
                     (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\")"
                )));
            };
            let instrument = instrument.trim();
            match repo
                .resolve_instrument_uid(&law_slug(instrument))
                .await
                .map_err(|e| RpcError::tool_error(e.to_string()))?
            {
                Some(uid) => *v = format!("{uid}|{}", lj_core::article_key::article_key(num)),
                None => return Err(unknown_instrument(&repo, "legal_article", instrument).await),
            }
        }
    }
    Ok(())
}

/// Erreur corrective pour un instrument hors catalogue : slugs candidats par
/// similarité trigramme (titre + slug, tolérante aux fautes).
async fn unknown_instrument(
    repo: &lj_store::repository::DecisionRepository<'_>,
    field: &str,
    value: &str,
) -> RpcError {
    let suggestions = repo.suggest_instruments(value, 5).await.unwrap_or_default();
    let hint = if suggestions.is_empty() {
        String::new()
    } else {
        format!(
            "; did you mean: {}",
            suggestions
                .iter()
                .map(|(slug, title)| format!("\"{slug}\" ({title})"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    RpcError::invalid_params(format!("unknown {field} '{value}'{hint}")).with_hint(
        "Pass a slug (from the legal_instrument facet of a previous search) \
         or an exact text name.",
    )
}

/// Extrait un argument chaîne requis non vide (erreur JSON-RPC sinon).
fn required_str(args: &Value, key: &str) -> Result<String, RpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params(format!("missing tool argument: {key}")))
}

/// Mappe une erreur des handlers LEGI en `RpcError` (NotFound → message dédié).
fn map_legi_error(e: ApiError) -> RpcError {
    match e {
        ApiError::NotFound => RpcError::not_found("law article or code not found").with_hint(
            "Find the article with search_legal_texts, or compose the url as \
             /texte/{code-slug}/{article-key} (lowercase key: \"l761-1\" for L. 761-1).",
        ),
        other => RpcError::tool_error(other.to_string()),
    }
}

/// Construit un [`SearchRequest`] à partir des arguments MCP (`ai_rerank` →
/// `ai_mode`, dates ISO conservées telles quelles ; filtres référentiels
/// solution/procedure/office/legal_domain/jurisdiction_code/publication, ADR 0146).
fn build_search_request(args: &Value) -> Result<lj_dtos::SearchRequest, RpcError> {
    // Parité serveur du `additionalProperties: false` annoncé par le schéma :
    // un filtre inconnu ignoré en silence fausserait la recherche sans signal.
    const FIELDS: [&str; 18] = [
        "query",
        "jurisdiction_type",
        "solution",
        "procedure",
        "office",
        "legal_domain",
        "jurisdiction_code",
        "chamber",
        "legal_instrument",
        "legal_article",
        "significance",
        "publication",
        "date_from",
        "date_to",
        "mode",
        "sort",
        "limit",
        "ai_rerank",
    ];
    if let Some(obj) = args.as_object() {
        if let Some(unknown) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(RpcError::invalid_params(format!(
                "unknown field '{unknown}'; valid fields: {}",
                FIELDS.join(", ")
            )));
        }
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("missing tool argument: query"))?
        .to_string();
    // Parité du schéma annoncé (`query.maxLength = 512`, `Field(max_length=512)`
    // côté oracle) ET du chemin HTTP : le port omettait ce cap, si bien qu'une
    // query géante partait en embedding + rerank LLM (DoS léger).
    if query.chars().count() > 512 {
        return Err(RpcError::invalid_params(
            "query: String should have at most 512 characters",
        ));
    }

    // Désérialisation tolérante champ par champ via serde_json (cf. [`deser`]),
    // en réutilisant le mapping camelCase des enums DTO.
    let from_field = |key: &str| args.get(key).filter(|v| !v.is_null()).cloned();

    // Dates : `datetime.date.fromisoformat()` + bornes `ge`/`le` côté oracle →
    // validées ici (format + plage), sinon une date malformée atteignait le SQL.
    let date_from: Option<String> = deser(from_field("date_from"))?;
    let date_to: Option<String> = deser(from_field("date_to"))?;
    validate_mcp_date("date_from", &date_from)?;
    validate_mcp_date("date_to", &date_to)?;
    // `limit` : `Field(ge=1, le=20)` côté oracle.
    let limit: u32 = deser(from_field("limit"))?.unwrap_or(10);
    if !(1..=20).contains(&limit) {
        return Err(RpcError::invalid_params(
            "limit: Input should be between 1 and 20",
        ));
    }

    Ok(lj_dtos::SearchRequest {
        query,
        jurisdiction_type: deser(from_field("jurisdiction_type"))?,
        solution: deser(from_field("solution"))?,
        procedure: deser(from_field("procedure"))?,
        office: deser(from_field("office"))?,
        legal_domain: deser(from_field("legal_domain"))?,
        jurisdiction_code: deser(from_field("jurisdiction_code"))?,
        chamber: deser(from_field("chamber"))?,
        legal_instrument: deser(from_field("legal_instrument"))?,
        legal_article: deser(from_field("legal_article"))?,
        significance: deser(from_field("significance"))?,
        publication: deser(from_field("publication"))?,
        date_from,
        date_to,
        mode: deser(from_field("mode"))?.unwrap_or(lj_dtos::SearchMode::Auto),
        sort: deser(from_field("sort"))?.unwrap_or(lj_dtos::SortOrder::Relevance),
        limit,
        offset: 0,
        ai_mode: deser(from_field("ai_rerank"))?.unwrap_or(true),
    })
}

/// Valide une date d'argument MCP (`date_from`/`date_to`) — parité du
/// `datetime.date.fromisoformat()` + bornes `ge`/`le` de l'oracle. L'erreur
/// remonte en `invalid_params` (donc `isError` côté client, via [`call_tool`]).
fn validate_mcp_date(field: &str, raw: &Option<String>) -> Result<(), RpcError> {
    let Some(s) = raw else { return Ok(()) };
    crate::search::parse_search_date(s)
        .map(|_| ())
        .map_err(|e| {
            let detail = match e {
                crate::search::DateError::Parse(m) => format!("invalid {field}: {m}"),
                crate::search::DateError::TooEarly => {
                    format!("{field} must be >= {}", crate::search::DATE_GE)
                }
                crate::search::DateError::TooLate => {
                    format!("{field} must be <= {}", crate::search::DATE_LE)
                }
            };
            RpcError::invalid_params(detail)
        })
}

/// Désérialise une valeur optionnelle vers `T` (erreur JSON-RPC `invalid_params`
/// en cas de type incompatible).
fn deser<T: serde::de::DeserializeOwned>(v: Option<Value>) -> Result<Option<T>, RpcError> {
    match v {
        None => Ok(None),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(|e| RpcError::invalid_params(format!("invalid argument: {e}"))),
    }
}

// ── Outils : schémas ─────────────────────────────────────────────────────────

/// Liste des définitions d'outils (`tools/list`). Chaque schéma est inline, sans
/// `title`, avec `additionalProperties: false` au niveau racine (port de
/// `_postprocess_tool_schemas`).
fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "search_decisions",
            "title": "Search Decisions",
            "description": SEARCH_DECISIONS_DESC,
            "inputSchema": search_decisions_schema(),
            "outputSchema": search_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_decision",
            "title": "Get Decision",
            "description": GET_DECISION_DESC,
            "inputSchema": get_decision_schema(),
            "outputSchema": get_decision_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "list_my_activity",
            "title": "List My Activity",
            "description": LIST_ACTIVITY_DESC,
            "inputSchema": list_activity_schema(),
            "outputSchema": activity_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_legal_text",
            "title": "Get Legal Text",
            "description": GET_LEGAL_TEXT_DESC,
            "inputSchema": get_legal_text_schema(),
            "outputSchema": get_legal_text_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "search_legal_texts",
            "title": "Search Legal Texts",
            "description": SEARCH_LEGAL_TEXTS_DESC,
            "inputSchema": search_legal_texts_schema(),
            "outputSchema": search_legal_texts_output_schema(),
            "annotations": read_only_annotations(),
        }),
    ]
}

/// Annotations communes : tous les outils sont en lecture seule. Le log
/// d'activité de `search_decisions`/`get_decision` (`record_search`,
/// `record_decision_view`) est une journalisation privée du compte — pas une
/// modification d'environnement au sens MCP — et un `readOnlyHint: false`
/// coûte une confirmation par appel dans les clients tiers.
fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

// ── Prompts MCP ──────────────────────────────────────────────────────────────
//
// Workflows invocables par l'utilisateur depuis le client (menu prompts /
// slash-command), servis à TOUT client connecté au serveur distant — là où les
// skills du plugin ne servent que ceux qui l'installent. Titres/descriptions et
// messages en français : c'est du texte face utilisateur, inséré dans SA
// conversation. Les trois workflows reprennent les cas d'usage de la soumission
// ChatGPT (`publish/soumissions/chatgpt-app-copy-paste.md`, dernière version) ;
// les exemples suivent ses « prompts vitrine », calés sur les requêtes réelles
// des utilisateurs (droit des étrangers en tête, frais irrépétibles verbatim).

/// Liste des prompts (`prompts/list`).
fn prompt_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "recherche_jurisprudence",
            "title": "Recherche de jurisprudence",
            "description": "Recherche des décisions sur une question juridique, les lit en texte intégral et rend une synthèse sourcée.",
            "arguments": [{
                "name": "question",
                "description": "Question ou sujet juridique, juridiction et période comprises si utiles (ex. « décisions du TA de Paris annulant une OQTF pour atteinte à l'intérêt supérieur de l'enfant »)",
                "required": true,
            }],
        }),
        json!({
            "name": "article_a_date",
            "title": "Article en vigueur à une date",
            "description": "Retrouve le texte exact d'un article de loi tel qu'il s'appliquait à une date donnée.",
            "arguments": [
                {
                    "name": "article",
                    "description": "L'article et son texte (ex. « article 1240 du code civil »)",
                    "required": true,
                },
                {
                    "name": "date",
                    "description": "Date de consultation YYYY-MM-DD (ex. la date des faits) ; vide = version en vigueur",
                    "required": false,
                },
            ],
        }),
        json!({
            "name": "textes_applicables",
            "title": "Textes applicables à un sujet",
            "description": "Identifie les articles de loi qui régissent un sujet et les lit dans leur version pertinente.",
            "arguments": [{
                "name": "sujet",
                "description": "Le sujet ou la situation juridique (ex. « frais irrépétibles en procédure civile »)",
                "required": true,
            }],
        }),
    ]
}

/// `prompts/get` : instancie le prompt demandé avec ses arguments. Le message
/// rendu encode la méthode anti-hallucination (lire le texte intégral avant de
/// citer, version à date, avouer l'introuvable).
fn get_prompt(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing prompt name"))?;
    let arg = |key: &str| -> Result<String, RpcError> {
        params
            .get("arguments")
            .and_then(|a| a.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| RpcError::invalid_params(format!("missing prompt argument: {key}")))
    };
    let (description, text) = match name {
        "recherche_jurisprudence" => {
            let question = arg("question")?;
            (
                "Recherche de jurisprudence avec lecture des décisions en texte intégral.",
                format!(
                    "Recherche la jurisprudence pertinente sur : {question}.\n\n\
                     Méthode : lance search_decisions — juridiction, période et tri \
                     (récence) dans les filtres structurés, la question réservée au \
                     problème de droit ; reformule ou filtre si la première salve déçoit. \
                     Puis ouvre chaque décision candidate avec get_decision \
                     avant de la citer — un extrait ou un résumé ne suffit jamais pour \
                     affirmer ce qu'une décision juge. Vérifie appellateFate avant de citer \
                     une décision comme autorité. Rends une synthèse sourcée : pour chaque \
                     décision retenue, juridiction, date, numéro, apport, et son url en lien. \
                     Si rien de probant ne ressort, dis-le : le corpus n'est pas exhaustif."
                ),
            )
        }
        "article_a_date" => {
            let article = arg("article")?;
            let quand = match params
                .get("arguments")
                .and_then(|a| a.get("date"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                Some(d) => format!("en vigueur au {d} (passe date={d} à get_legal_text)"),
                None => "actuellement en vigueur".to_string(),
            };
            (
                "Texte exact d'un article de loi à une date donnée (law-at-date).",
                format!(
                    "Donne le texte exact de {article}, dans sa version {quand}.\n\n\
                     Méthode : get_legal_text par l'url /texte/{{code}}/{{article}} (passe par \
                     search_legal_texts si le numéro est incertain). Cite le texte intégral \
                     de la version servie, précise ses dates de validité, et signale les \
                     versions voisines si la date demandée tombe près d'un changement."
                ),
            )
        }
        "textes_applicables" => {
            let sujet = arg("sujet")?;
            (
                "Textes et articles applicables à un sujet, lus dans la bonne version.",
                format!(
                    "Identifie les textes qui régissent : {sujet}.\n\n\
                     Méthode : search_legal_texts pour constituer la liste (affine avec la \
                     facette code), puis ouvre chaque article pertinent avec get_legal_text — \
                     avec date si le litige relève d'une version antérieure. Cite chaque \
                     article par numéro et code, avec sa version et son url."
                ),
            )
        }
        other => return Err(RpcError::invalid_params(format!("unknown prompt: {other}"))),
    };
    Ok(json!({
        "description": description,
        "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
    }))
}

/// Enum brut d'un type sérialisable (codes attendus dans le schéma MCP).
fn enum_values(values: &[&str]) -> Value {
    json!(values)
}

/// Codes sérialisés d'un enum de facette DTO (miroir du seed 0100, ADR 0146) —
/// source unique : les variantes `ALL` de `lj_dtos::schema`, pas de liste copiée.
fn facet_enum_codes<T: serde::Serialize>(all: &[T]) -> Value {
    json!(all
        .iter()
        .filter_map(|v| serde_json::to_value(v).ok())
        .collect::<Vec<Value>>())
}

/// Variante pour un vocabulaire porté en `&[&str]` (suffixes d'uid).
fn facet_enum_codes_str(values: &[&str]) -> Value {
    json!(values)
}

/// Champ `Optional[T]` tel que Pydantic le sérialise : `{"anyOf": [inner,
/// {"type": "null"}], "default": null, <extra…>}`. `extra` apporte les clés
/// soeurs de `anyOf` (`description` côté inputSchema, `format` côté
/// outputSchema). Parité avec la sortie `model_json_schema` (Python).
fn nullable(inner: Value, extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("anyOf".into(), json!([inner, {"type": "null"}]));
    m.insert("default".into(), Value::Null);
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    Value::Object(m)
}

fn search_decisions_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": 512,
                "description": SEARCH_QUERY_DESC,
            },
            "jurisdiction_type": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": enum_values(JURISDICTION_TYPE_CODES)}}),
                json!({"description": "Restrict to one or more court categories: TJ (tribunal judiciaire), CA (cour d'appel), CC (Cour de cassation), TCOM (tribunal des activités économiques), TA (tribunal administratif), CAA (cour administrative d'appel), CE (Conseil d'État), CNDA (asylum), CONSTIT (Conseil constitutionnel), TC (Tribunal des conflits), CNIL (sanctions), CEDH and CJUE (European courts)."}),
            ),
            "solution": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Solution::ALL)}}),
                json!({"description": "Filter by the ruling of the operative part (référentiel solution, ADR 0146). REJET / IRRECEVABILITE / DESISTEMENT / NON_LIEU_A_STATUER are procedural or negative endings; CONFIRMATION / INFIRMATION* / REFORMATION are appeal outcomes; CASSATION* is cassation-specific; ANNULATION covers administrative annulment; SATISFACTION_TOTALE / SATISFACTION_PARTIELLE cover first-instance civil rulings granting the claim."}),
            ),
            "procedure": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Procedure::ALL)}}),
                json!({"description": "Filter by procedural track (référés, QPC, EU referral, révision, tierce opposition…). Absent value = ordinary contentious procedure."}),
            ),
            "office": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Office::ALL)}}),
                json!({"description": "Filter by specialised judge/office (JLD, JAF, JCP, JEX, juge des enfants, premier président, magistrat désigné). Absent value = ordinary bench."}),
            ),
            "legal_domain": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Domain::ALL)}}),
                json!({"description": "Filter by legal domain (curated domain tree, ADR 0146): 9 roots (CIVIL, COMMERCIAL, PUBLIC, SOCIAL, FISCAL, PROPRIETE_INTELLECTUELLE, EUROPEEN, CRIMINEL, CONSTITUTIONNEL) and their leaves (e.g. CIVIL_DROIT_LOCATIF). Selecting a root also matches all its leaves."}),
            ),
            "publication": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes_str(PUBLICATION_CODES)}}),
                json!({"description": "Filter by publication level (any-of, référentiel publication): PUBLIE_BULLETIN / INEDIT_BULLETIN (Cour de cassation), PUBLIE_LEBON / MENTIONNE_LEBON / INEDIT_LEBON (administrative), AUTRE (no publication statement in the source)."}),
            ),
            "significance": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Significance::ALL)}}),
                json!({"description": "Filter by jurisprudential significance (any-of, ADR 0167), derived from publication codes at strongest rank: MAJEURE (rapport annuel / recueil Lebon), IMPORTANTE (bulletin, tables du Lebon, lettres de chambre, communiqués), LIMITEE (unpublished), INDETERMINEE (no publication statement — lower courts, European courts)."}),
            ),
            "jurisdiction_code": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 40}}),
                json!({"description": "Restrict to one or more precise court units by referential code. Code shapes: \"cc\" (Cour de cassation), \"ce\" (Conseil d'État), \"cnda\", \"cedh\", \"cjue\"; \"ca_<city>\", \"caa_<city>\", \"ta_<city>\", \"tj_<city>\", \"tcom_<city>\" (e.g. \"ca_paris\", \"ta_marseille\", \"tj_paris\", \"tcom_lyon\"). An unknown code fails listing the nearest valid codes; when unsure, guess with the city name in the code and let the error correct you. Each code is a court; the chamber is a separate axis (see chamber); use jurisdiction_type for the broad category."}),
            ),
            "chamber": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 40}}),
                json!({"description": "Filter by chamber category (any-of, uniform across orders, e.g. \"CIVILE\", \"SOCIALE\", \"COMMERCIALE\", \"CRIMINELLE\", \"ETRANGERS\", \"PRUD_HOMALE\", \"INSTRUCTION\"). Codes come from the chamber facet of a previous search; an unknown code fails listing the valid codes."}),
            ),
            "legal_instrument": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 100}}),
                json!({"description": "Restrict to decisions citing one or more given codes or statutes. Accepts a slug from facets.legal_instrument (e.g. \"code-civil\") or an exact text name resolved server-side (e.g. \"Code civil\"); an unknown value fails listing the closest slugs with their titles, typos included."}),
            ),
            "legal_article": nullable(
                json!({"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 140}}),
                json!({"description": "Restrict to decisions citing a specific article of a specific code, as a composite key \"<instrument>|<article>\" where <instrument> is a slug or an exact text name, resolved like legal_instrument (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\"). The instrument prefix is required — the same article number exists in several codes."}),
            ),
            "date_from": nullable(
                json!({"type": "string"}),
                json!({"description": "Earliest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "date_to": nullable(
                json!({"type": "string"}),
                json!({"description": "Latest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "mode": {
                "type": "string",
                "enum": ["auto", "lexical", "semantic"],
                "default": "auto",
                "description": SEARCH_MODE_DESC,
            },
            "sort": {
                "type": "string",
                "enum": ["relevance", "date_desc", "date_asc"],
                "default": "relevance",
                "description": "Result ordering. Use 'relevance' (default) unless the user wants chronological order.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (1–20, default 10).",
            },
            "ai_rerank": {
                "type": "boolean",
                "default": true,
                "description": "When enabled (default), reorders results by actual relevance to the query using an LLM reranker (cf. ADR 0041). Keep on for agentic use — shortlist quality is significantly higher. Cost: a few seconds of extra latency. Disable only for high-rate exploratory searches where latency matters more than ranking quality.",
            },
        },
    })
}

fn get_decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "minLength": 1,
                "description": "Decision URL, from search_decisions hits or an inline citation link.",
            },
        },
    })
}

fn list_activity_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["searches", "bookmarks", "readingHistory", "all"],
                "default": "all",
                "description": "Which tab to list: 'searches', 'bookmarks', 'readingHistory', or 'all' (default) for the three at once.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "default": 50,
                "description": "Maximum number of entries per tab (1–100, default 50).",
            },
        },
    })
}

/// Schéma d'entrée de `get_legal_text` : `url` requise + `date` optionnelle.
fn get_legal_text_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "description": LAW_URL_DESC,
            },
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD) — returns the version in force at that date. Omit for the version currently in force."}),
            ),
        },
    })
}

/// Schéma d'entrée de `search_legal_texts` (`query` requis + filtres
/// `code`/`jurisdiction`/`nature`/`source` optionnels + `limit`).
fn search_legal_texts_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": 512,
                "description": "French query over statutory articles. Matches article titles (boosted) and bodies; alias expansion handles acronyms and usual names.",
            },
            "code": nullable(
                json!({"type": "string"}),
                json!({"description": "Restrict to one code/text by its URL slug (\"code-civil\", as in facets.code) or exact name. An unknown code errors back with the closest slugs. Omit to search the whole navigable referential."}),
            ),
            "jurisdiction": nullable(
                json!({"type": "string"}),
                json!({"description": "Filter by country/legal order, as an ISO 3166 alpha-2 country code: \"FR\" (France, the bulk of the corpus) or a foreign code (\"SN\", \"DZ\", \"MA\", \"VN\", \"PE\", …); plus \"UE\" for EU law and \"INTL\" for treaties/international law."}),
            ),
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (1–20, default 10).",
            },
        },
    })
}

// ── Outils : schémas de sortie (`outputSchema`, parité Pydantic) ──────────────

/// Schéma de sortie de `search_decisions` (= `McpSearchResponse`).
fn search_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "hits"],
        "properties": {
            "query": {"type": "string"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "description": HIT_METADATA_DESC,
                    "required": ["title", "url", "chars", "jurisdictionType"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "aiSummary": nullable(json!({"type": "string"}), json!({"description": "AI-written summary of what the decision is about (semantic search) — a machine paraphrase, NEVER the court's words: nothing in it is quotable. Exactly one of aiSummary/snippet is served per hit."})),
                        "snippet": nullable(json!({"type": "string"}), json!({"description": "Verbatim passage of the decision text where the query terms matched (keyword search). Exactly one of aiSummary/snippet is served per hit."})),
                        "chars": {"type": "integer"},
                        "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
                        "jurisdictionType": {"type": "string"},
                        "jurisdictionCode": nullable(json!({"type": "string"}), json!({})),
                        "chamber": nullable(json!({"type": "string"}), json!({})),
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "procedure": nullable(json!({"type": "string"}), json!({})),
                        "office": nullable(json!({"type": "string"}), json!({})),
                        "legalDomain": nullable(json!({"type": "string"}), json!({})),
                        "publication": nullable(json!({"type": "string"}), json!({})),
                    },
                },
            },
            "facets": {
                "type": "object",
                "description": "Per filter name, a map of filter value to decision count under the current query (jurisdiction_type, jurisdiction_code, chamber, office, legal_domain, solution, significance, publication, date_lecture_year, legal_instrument). Reuse keys verbatim as filter values. jurisdiction_code is capped to the top 15 courts (other_courts counts the rest), legal_instrument to the top 10 statutes.",
                "additionalProperties": true,
            },
        },
    })
}

/// Schéma de sortie de `get_decision` (= `McpDecisionDetail`).
fn get_decision_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": HIT_METADATA_DESC,
        "required": ["title", "url", "jurisdictionType", "text"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
            "jurisdictionType": {"type": "string"},
            "jurisdictionCode": nullable(json!({"type": "string"}), json!({})),
            "chamber": nullable(json!({"type": "string"}), json!({})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "procedure": nullable(json!({"type": "string"}), json!({})),
            "office": nullable(json!({"type": "string"}), json!({})),
            "legalDomain": nullable(json!({"type": "string"}), json!({})),
            "publication": nullable(json!({"type": "string"}), json!({})),
            "appellateFate": nullable(json!({"type": "string"}), json!({"description": "Fate of THIS decision before the court that reviewed it, precomputed from caseChronology: 'INFIRMATION — Cour d'appel de Paris, 1 juillet 2025 (url)'. INFIRMATION = this decision was reversed and no longer stands; CONFIRMATION = upheld. Absent = no recourse known to the corpus, which never proves the decision is final."})),
            "caseChronology": {
                "type": "array",
                "description": "Linked prior and subsequent decisions of the same case (appeal, pourvoi, renvoi), most recent first, current decision included. The fate of a judgment reads in the solution of the decision that reviewed it (INFIRMATION = reversed, CONFIRMATION = upheld). Absent chronology never proves no recourse exists.",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "current"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "current": {"type": "boolean"},
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "linkToNext": nullable(json!({"type": "string"}), json!({"description": "Link nature to the step below (the attacked decision): APPEL_DE | POURVOI_CONTRE | RENVOI_APRES_CASSATION."})),
                    },
                },
            },
            "commentaires": {
                "type": "array",
                "description": COMMENTAIRES_DESC,
                "items": commentaire_schema(),
            },
            "text": {"type": "string"},
        },
    })
}

/// Schéma d'un commentaire institutionnel (= `lj_dtos::Commentaire`, ADR
/// 0204/0212), partagé entre `get_decision` et `get_legal_text`.
fn commentaire_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["kind"],
        "properties": {
            "kind": {"type": "string", "description": "analyse = official court-written abstract, full content in `body`. conclusions = the rapporteur public's conclusions, `url` only (copyright-protected, existence + official link). note = linked institutional document (report, avocat général's opinion, press release, doctrine note), `url` + `title` + `publisher`."},
            "author": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "body": nullable(json!({"type": "string"}), json!({})),
            "title": nullable(json!({"type": "string"}), json!({})),
            "publisher": nullable(json!({"type": "string"}), json!({})),
            "access": nullable(json!({"type": "string"}), json!({"description": "libre | abonnes"})),
            "rubriques": {"type": "array", "items": {"type": "string"}, "description": "Official classification headings (analyse)."},
            "renvois": {"type": "array", "items": {"type": "string"}, "description": "Doctrinal cross-references of the analyse (« Cf. CE, … »)."},
            "url": nullable(json!({"type": "string"}), json!({"description": "External link, to hand to the user — not openable by an MCP tool."})),
        },
    })
}

/// Référence décision inline (= `McpDecisionRef`). Apparaît deux fois dans
/// l'`outputSchema` d'activité (sous `bookmarks` et `readingHistory`) après
/// inlining des `$ref`, comme côté Python.
fn decision_ref_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": DECISION_REF_DESC,
        "required": ["title", "url"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "summary": nullable(json!({"type": "string"}), json!({})),
            "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "bookmarkedAt": nullable(json!({"type": "string"}), json!({"format": "date-time"})),
            "viewCount": nullable(json!({"type": "integer"}), json!({})),
            "lastSource": nullable(json!({"type": "string"}), json!({})),
            "lastViewedAt": nullable(json!({"type": "string"}), json!({"format": "date-time"})),
        },
    })
}

/// Schéma de sortie de `list_my_activity` (= `McpActivityResponse`).
fn activity_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": ACTIVITY_RESPONSE_DESC,
        "properties": {
            "searches": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "searches"],
                    "properties": {
                        "total": {"type": "integer"},
                        "searches": {
                            "type": "array",
                            "items": {
                                "additionalProperties": false,
                                "type": "object",
                                "required": ["query", "source", "searchedAt"],
                                "properties": {
                                    "query": {"type": "string"},
                                    "filters": nullable(json!({"additionalProperties": true, "type": "object"}), json!({})),
                                    "source": {"type": "string"},
                                    "searchedAt": {"format": "date-time", "type": "string"},
                                },
                            },
                        },
                    },
                }),
                json!({}),
            ),
            "bookmarks": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "bookmarks"],
                    "properties": {
                        "total": {"type": "integer"},
                        "bookmarks": {"type": "array", "items": decision_ref_schema()},
                    },
                }),
                json!({}),
            ),
            "readingHistory": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "decisions"],
                    "properties": {
                        "total": {"type": "integer"},
                        "decisions": {"type": "array", "items": decision_ref_schema()},
                    },
                }),
                json!({}),
            ),
        },
    })
}

/// Schéma de sortie de `get_legal_text` (= `McpLawArticle`).
fn get_legal_text_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["code", "num", "title", "url", "etat", "dateDebut", "versions"],
        "properties": {
            "code": {"type": "string"},
            "num": {"type": "string"},
            "title": {"type": "string"},
            "url": {"type": "string"},
            "etat": {"type": "string"},
            "dateDebut": {"type": "string", "format": "date"},
            "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "text": nullable(json!({"type": "string"}), json!({"description": "Full text; cross-references to other articles render as inline markdown links (/texte/ urls, open with get_legal_text)."})),
            "sourceUrl": nullable(json!({"type": "string"}), json!({})),
            "nota": nullable(json!({"type": "string"}), json!({})),
            "commentaires": {
                "type": "array",
                "description": COMMENTAIRES_DESC,
                "items": commentaire_schema(),
            },
            "versions": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["etat", "dateDebut"],
                    "properties": {
                        "etat": {"type": "string"},
                        "dateDebut": {"type": "string", "format": "date"},
                        "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
                    },
                },
            },
        },
    })
}

/// Schéma de sortie de `search_legal_texts` (= `McpLawSearchResponse`).
fn search_legal_texts_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits", "facets"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "snippet", "source"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"},
                        "titlePath": nullable(json!({"type": "string"}), json!({})),
                        "source": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "description": "Per filter name, a map of filter value to article count under the current query (code, jurisdiction). Reuse keys verbatim as filter values. Each axis is capped to its top 10.",
                "additionalProperties": true,
            },
        },
    })
}

// ── Codes d'enum (alignés sur les renames serde lj-core) ─────────────────────

const JURISDICTION_TYPE_CODES: &[&str] = &[
    "TA", "CAA", "CE", "CONSTIT", "TC", "CC", "CA", "TJ", "TCOM", "CEDH", "CJUE", "CNDA", "CNIL",
];

/// Suffixes d'uid `publication:*` (facette de référence à 6 valeurs, seed 0100).
const PUBLICATION_CODES: &[&str] = &[
    "PUBLIE_BULLETIN",
    "INEDIT_BULLETIN",
    "PUBLIE_LEBON",
    "MENTIONNE_LEBON",
    "INEDIT_LEBON",
    "AUTRE",
];

// ── Descriptions verbatim (port des docstrings/Field MCP) ─────────────────────

const SEARCH_DECISIONS_DESC: &str = "Search decisions by meaning and keywords combined. Returns a \
shortlist: `title`, `url`, an overview (`aiSummary`: AI-written \
summary of what the decision is about, a machine paraphrase never \
quotable as the court's words; or `snippet`: the verbatim passage \
where your keywords matched) and metadata; get_decision reads the \
full text. Put constraints in the structured filters (jurisdiction, \
dates, articles, codes), keep the query for the legal issue. Values \
within one filter are OR'd; different filters are AND'd. The response \
carries a `facets` block: per filter name, a map of filter value to \
decision count under the current query. Reuse those keys verbatim to \
refine (a code from `facets.jurisdiction_code`, a slug from \
`facets.legal_instrument`). Hit metadata fields carry the same tokens \
under the same names: a hit's `jurisdictionCode`, `chamber` or \
`solution` passes back verbatim into the matching filter.";

const GET_DECISION_DESC: &str = "Fetch the full text and metadata of a decision by its `url`. The \
text carries inline markdown links to cited articles (/texte/, open with \
get_legal_text) and cited decisions (/decision/, open with \
get_decision); a citation spanning several articles (« articles 3 à 6 \
», « et suivants ») links its first article and appends the others as \
labelled links right after the span. `appellateFate` states in one line what became of THIS \
decision on review (INFIRMATION = reversed, it no longer stands; \
CONFIRMATION = upheld) — read it before citing the decision as \
authority. `caseChronology` lists the linked prior AND subsequent \
decisions of the same case (appeal, pourvoi, renvoi) known to the \
corpus. An absent fate or chronology never proves no recourse exists — \
only that none is linked in the corpus. `commentaires` carries the \
institutional commentary (official analyses inline, links to the \
rapporteur public's conclusions and related court documents).";

/// Description partagée du bloc `commentaires` (décision ADR 0204, norme
/// ADR 0212) dans les deux `outputSchema`.
const COMMENTAIRES_DESC: &str =
    "Institutional commentary anchored on this document: court-written \
analyses served inline (`body`), plus outbound links (`url`) to the \
rapporteur public's conclusions and to related institutional documents \
(reports, opinions, press releases). Context, never the ruling itself \
— quote the decision text, not a commentaire, as the court's words.";

/// Description partagée des objets décision (hit de recherche et détail) :
/// la règle « champ = filtre, valeur = token » y est dite une fois.
const HIT_METADATA_DESC: &str = "Metadata fields carry search_decisions filter tokens: each field \
matches the filter of the same name (jurisdictionType → \
jurisdiction_type, legalDomain → legal_domain…) and its value passes \
back verbatim to filter.";

const GET_LEGAL_TEXT_DESC: &str =
    "Fetch a statutory article as it read on a given date, by its `url` \
(any /texte/ link). Returns the version in force at `date` (omit for \
today): full text, status, validity dates, and the timeline of all \
versions — say which version you quote. The text carries inline \
markdown links to cross-referenced articles (/texte/, open with \
get_legal_text; when served at a date, the links point to the same \
date). `commentaires` carries institutional commentary anchored on the \
article (analyses inline, links otherwise). Covers French codes and \
statutes, plus curated foreign codes and treaties.";

const SEARCH_LEGAL_TEXTS_DESC: &str =
    "Find statutory articles from their subject or wording when the article \
number is unknown; returns a ranked shortlist with highlighted \
snippets and the exact `total`. Query in French, descriptive terms (« \
délai de recours contentieux refus implicite »); put the code in the \
`code` filter (slug or exact name), keep the query for the subject. \
The response carries a `facets` block (`code`, `jurisdiction`): per \
filter name, a map of filter value to article count — reuse those \
keys verbatim to refine. Chain a hit into get_legal_text with its \
`url`, plus `date` when the dispute is governed by an earlier version.";

const LAW_URL_DESC: &str = "Article URL `/texte/{code}/{article}`: take it from a \
search_legal_texts hit or an inline citation link, or compose it — \
{code} is the code slug (\"code-civil\", as in facets.legal_instrument), \
{article} the lowercase article key (\"l761-1\" for L. 761-1, \"1240\" \
for 1240). An unknown code errors back with the closest slugs.";

/// Description inline de `McpDecisionRef` dans l'outputSchema d'activité (port
/// verbatim de la docstring, retours à la ligne `\n` inclus — Pydantic les
/// conserve tels quels dans le schéma émis).
const DECISION_REF_DESC: &str =
    "Référence décision pour signets/consultations : assez pour citer ou
\
chaîner vers ``get_decision`` via ``url``, sans le texte intégral.";

/// Description de `McpActivityResponse` (port verbatim de la docstring du modèle,
/// retours à la ligne inclus).
const ACTIVITY_RESPONSE_DESC: &str =
    "Sortie unique de ``list_my_activity`` : chaque tranche n'est remplie que\n\
si elle a été demandée (``kind`` ciblé ou ``all``). Un seul modèle à plat\n\
(plutôt qu'une union par ``kind``) évite une enveloppe ``result``. Noter\n\
qu'après inlining des ``$ref`` par ``_clean_schema`` (cf. mcp_server),\n\
``McpDecisionRef`` apparaît deux fois dans le schéma émis — une fois sous\n\
``bookmarks``, une fois sous ``readingHistory``.";

const LIST_ACTIVITY_DESC: &str =
    "List the authenticated user's own activity, most recent first. `kind` \
selects which tab: 'searches' (past queries with their structured \
`filters` and `source`), 'bookmarks' or 'readingHistory' (decisions, \
each with `title`, a `summary` and the `url` to chain into \
get_decision), or 'all' to get the three at once. Only the requested \
tab(s) are populated. In readingHistory, `lastSource` == 'web' means \
the user opened the decision manually at least once (a genuine read); \
'mcp' means it was only ever opened through this connector. Requires a \
connected account.";

const SEARCH_QUERY_DESC: &str = "French query, the primary input. Two regimes — pick the right \
tool for the job: \
(a) Natural language or descriptive keywords for legal-issue \
searches; the engine handles synonyms and reformulations. \
Examples: \
« responsabilité hôpital infection nosocomiale » ; \
« étranger malade soins inaccessibles dans son pays d'origine » ; \
« licenciement discrimination syndicale charge de la preuve ». \
(b) Quoted exact phrases and ET / OU / SAUF operators for a \
named entity (company, municipality, person), a precise legal \
formula, or the direction of a holding: semantic matching \
ignores negations (« n'est pas X » ranks like « est X »), and \
only an exact phrase targets which way the court ruled. Examples: \
« \"Société Générale\" » ; \
« \"commune de Saint-Denis\" SAUF Réunion » ; \
« \"force majeure\" ET épidémie ». \
Using operators or quotes switches the engine to keyword-only \
matching for the whole query (no synonyms), so do not mix the \
two regimes.";

const SEARCH_MODE_DESC: &str = "'auto' is the right choice in almost all cases — it picks \
the regime from the query shape. \
auto: combines meaning and keyword matching on plain queries; \
switches to keyword-only when the query contains ET / OU / \
SAUF or quoted phrases. \
lexical: force keyword-only matching. The right choice when \
searching for a named entity (company, municipality, person), \
a precise legal formula, or an exact quote — cuts semantic \
noise. \
semantic: force the meaning+keyword hybrid even on quoted or \
operator queries (where auto would fall back to keyword-only).";

#[cfg(test)]
mod tests {
    use super::*;

    /// Alphabet des `legal_text.slug` : le nom libre d'un texte doit tomber
    /// sur le slug servi par `/texte/{slug}`.
    #[test]
    fn law_slug_matches_catalog_alphabet() {
        assert_eq!(law_slug("Code civil"), "code-civil");
        assert_eq!(
            law_slug("Code de procédure civile"),
            "code-de-procedure-civile"
        );
        assert_eq!(
            law_slug("Code de l'entrée et du séjour des étrangers et du droit d'asile"),
            "code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile"
        );
        // Slug déjà canonique : point fixe.
        assert_eq!(law_slug("code-civil"), "code-civil");
    }

    #[test]
    fn search_schema_is_strict_and_inlined() {
        let schema = search_decisions_schema();
        // additionalProperties:false au niveau racine (strict-mode).
        assert_eq!(schema["additionalProperties"], json!(false));
        // Pas de `title` au niveau racine ni dans les propriétés.
        assert!(schema.get("title").is_none());
        assert!(schema["properties"]["query"].get("title").is_none());
        // query requis.
        assert_eq!(schema["required"], json!(["query"]));
        // Pas de $ref / $defs résiduels (schéma inline).
        let dumped = serde_json::to_string(&schema).unwrap();
        assert!(!dumped.contains("$ref"));
        assert!(!dumped.contains("$defs"));
    }

    #[test]
    fn tool_list_exposes_all_tools() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "search_decisions",
                "get_decision",
                "list_my_activity",
                "get_legal_text",
                "search_legal_texts",
            ]
        );
    }

    #[test]
    fn build_request_maps_referential_filters_and_ai_rerank() {
        let args = json!({
            "query": "bail commercial",
            "publication": ["PUBLIE_BULLETIN"],
            "jurisdiction_type": ["CC", "CA"],
            "solution": ["REJET", "CASSATION"],
            "legal_domain": ["CIVIL_DROIT_LOCATIF"],
            "jurisdiction_code": ["ca_paris"],
            "ai_rerank": false,
            "limit": 5,
        });
        let req = build_search_request(&args).unwrap();
        assert_eq!(req.query, "bail commercial");
        assert_eq!(req.publication, Some(vec!["PUBLIE_BULLETIN".to_string()]));
        assert_eq!(
            req.solution,
            Some(vec![lj_dtos::Solution::Rejet, lj_dtos::Solution::Cassation])
        );
        assert_eq!(
            req.legal_domain,
            Some(vec![lj_dtos::Domain::CivilDroitLocatif])
        );
        assert_eq!(req.jurisdiction_code, Some(vec!["ca_paris".to_string()]));
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![
                lj_dtos::JurisdictionType::Cc,
                lj_dtos::JurisdictionType::Ca
            ])
        );
        assert!(!req.ai_mode);
        assert_eq!(req.limit, 5);
        // Défauts Python : mode auto, sort relevance, offset 0.
        assert_eq!(req.mode, lj_dtos::SearchMode::Auto);
        assert_eq!(req.sort, lj_dtos::SortOrder::Relevance);
        assert_eq!(req.offset, 0);
    }

    #[test]
    fn build_request_defaults_ai_rerank_true_and_limit_10() {
        let req = build_search_request(&json!({"query": "x"})).unwrap();
        assert!(req.ai_mode);
        assert_eq!(req.limit, 10);
    }

    #[test]
    fn build_request_rejects_empty_query() {
        assert!(build_search_request(&json!({"query": ""})).is_err());
        assert!(build_search_request(&json!({})).is_err());
    }

    #[test]
    fn unknown_jurisdiction_code_suggests_by_source_label_and_typo() {
        let refs = crate::referential::Referential::new(
            vec![],
            vec![lj_store::repository::JurisdictionRow {
                code: "tj_paris".into(),
                source_code: "tj75056".into(),
                jurisdiction_type: "TJ".into(),
                city: Some("Paris".into()),
                label: "Tribunal judiciaire de Paris".into(),
            }],
        );
        // Ancien code (location Judilibre, pré-ADR 0201) → renvoi exact.
        let err = unknown_jurisdiction_code("tj75056", &refs);
        assert!(err.message.contains("tj_paris"));
        assert!(err.message.contains("Tribunal judiciaire de Paris"));
        // Fragment de libellé (ville).
        let err = unknown_jurisdiction_code("tribunal_paris", &refs);
        assert!(err.message.contains("tj_paris"));
        // Typo sans fragment de libellé → repli distance d'édition sur le code.
        let err = unknown_jurisdiction_code("tj_parsi", &refs);
        assert!(err.message.contains("tj_paris"));
    }

    #[test]
    fn build_request_rejects_unknown_fields() {
        // Un filtre mal nommé doit échouer franchement, pas être ignoré.
        let err = build_search_request(&json!({
            "query": "bail",
            "jurisdiction_name": ["ca_paris"],
        }))
        .unwrap_err();
        assert!(err.message.contains("jurisdiction_name"));
    }

    /// Contrat d'erreur outil : `category` + `retryable` (+ `hint`) en JSON dans
    /// le bloc texte, pour que le modèle sache corriger / basculer / retenter.
    #[test]
    fn tool_error_content_carries_error_contract() {
        let err = RpcError::not_found("no decision at url=x")
            .with_hint("Take the url from a search_decisions hit.");
        let content = tool_error_content(&err);
        assert_eq!(content["isError"], json!(true));
        let body: Value =
            serde_json::from_str(content["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"], json!("no decision at url=x"));
        assert_eq!(body["category"], json!("not_found"));
        assert_eq!(body["retryable"], json!(false));
        assert!(body["hint"].as_str().unwrap().contains("search_decisions"));

        let internal = tool_error_content(&RpcError::tool_error("pool: timeout"));
        let body: Value =
            serde_json::from_str(internal["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["category"], json!("internal"));
        assert_eq!(body["retryable"], json!(true));
        assert!(body.get("hint").is_none());

        let auth = tool_error_content(&RpcError::auth("authentication required"));
        let body: Value =
            serde_json::from_str(auth["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["category"], json!("auth"));
    }

    #[test]
    fn prompts_list_and_get_render_messages() {
        let prompts = prompt_definitions();
        let names: Vec<&str> = prompts
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "recherche_jurisprudence",
                "article_a_date",
                "textes_applicables",
            ]
        );
        let got = get_prompt(&json!({
            "name": "article_a_date",
            "arguments": {"article": "article 1240 du code civil", "date": "2018-01-01"},
        }))
        .unwrap();
        assert_eq!(got["messages"][0]["role"], json!("user"));
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("article 1240 du code civil"));
        assert!(text.contains("2018-01-01"));
        // `date` optionnelle : le message bascule sur la version en vigueur.
        let now = get_prompt(&json!({
            "name": "article_a_date",
            "arguments": {"article": "article 700 du CPC"},
        }))
        .unwrap();
        assert!(now["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("actuellement en vigueur"));
        // Argument requis manquant / prompt inconnu → invalid_params.
        assert!(get_prompt(&json!({"name": "recherche_jurisprudence", "arguments": {}})).is_err());
        assert!(get_prompt(&json!({"name": "nope"})).is_err());
    }

    #[test]
    fn initialize_carries_protocol_and_instructions() {
        let res = initialize_result();
        assert_eq!(res["protocolVersion"], json!(PROTOCOL_VERSION));
        assert_eq!(res["serverInfo"]["name"], json!(SERVER_NAME));
        assert!(res["instructions"]
            .as_str()
            .unwrap()
            .contains("French and European case law"));
    }
}
