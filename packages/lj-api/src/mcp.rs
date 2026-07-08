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
    http::StatusCode,
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use serde_json::{json, Value};

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
    "Search French case law from administrative courts (TA, CAA, CE) and \
civil courts (CC, CA, TJ, TCOM). \
The engine understands meaning, not just keywords: phrase the query \
as a natural question or a list of descriptive terms — synonyms and \
reformulations are handled. \
Typical workflow: call search_decisions to get a shortlist (previews \
give orientation but are not enough to judge actual relevance), then \
call get_decision on candidates to read the full text. Iterate by \
refining the query or filters. \
Each result carries a public `url` to the full decision on \
librejustice.fr and an opaque `id` used only to chain into \
get_decision — the id is internal and not meant for display. \
When mentioning a decision, hyperlink its `title` (or a \
conventional citation derived from the metadata) to that `url`.";

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

/// Erreur JSON-RPC remontée comme `error` dans l'enveloppe (codes standard MCP).
#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    /// Erreur applicative outil (équivalent `ToolError` côté Python) : code
    /// d'erreur interne MCP.
    fn tool_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }
}

/// Point d'entrée HTTP : décode l'enveloppe JSON-RPC, route vers la méthode.
async fn handle_rpc(
    State(state): State<AppState>,
    Extension(McpUser(user)): Extension<McpUser>,
    Json(body): Json<Value>,
) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (pas d'`id`) : on accuse réception sans corps (202).
    let is_notification = body.get("id").is_none();

    match dispatch(&state, method, params, user.as_deref()).await {
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

/// Routage des méthodes MCP standard.
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    user: Option<&str>,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(state, params, user).await,
        // FastMCP expose nativement les capacités resources/prompts (vides ici) :
        // un client qui les liste reçoit une liste vide, pas un method_not_found.
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
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
async fn call_tool(state: &AppState, params: Value, user: Option<&str>) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Sémantique MCP : un échec d'EXÉCUTION d'outil (outil inconnu ou erreur
    // pendant l'appel) n'est PAS une erreur JSON-RPC mais un *résultat* avec
    // `isError: true` et le message dans `content` (parité SDK `mcp`). Seules les
    // erreurs de PROTOCOLE (name manquant) restent des erreurs JSON-RPC.
    let result = match name {
        "search_decisions" => tool_search_decisions(state, arguments, user).await,
        "get_decision" => tool_get_decision(state, arguments, user).await,
        "list_my_activity" => tool_list_my_activity(state, arguments, user).await,
        "get_law_article" => tool_get_law_article(state, arguments).await,
        "search_law_articles" => tool_search_law_articles(state, arguments).await,
        other => return Ok(tool_error_content(format!("Unknown tool: {other}"))),
    };

    match result {
        Ok((text, structured)) => {
            // `content` lisible = JSON pretty-print du modèle (indent 2, ordre des
            // champs, `null` inclus — parité byte avec `pydantic_core.to_json`
            // côté FastMCP) ; `structuredContent` porte l'objet typé.
            Ok(json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
                "isError": false,
            }))
        }
        // Préfixe verbatim du SDK ; le détail diffère du texte Pydantic (non
        // reproduit — détail d'implémentation, cf. `serverInfo.version`).
        Err(e) => Ok(tool_error_content(format!(
            "Error executing tool {name}: {}",
            e.message
        ))),
    }
}

/// Résultat `tools/call` en erreur applicative : `isError: true` + message en
/// `content` texte (sémantique MCP, pas une erreur JSON-RPC).
fn tool_error_content(text: String) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true,
    })
}

/// Sérialise une sortie d'outil en `(content texte, structuredContent)`. Le
/// texte est pretty-print (indent 2, ordre des champs du modèle, `null` inclus)
/// pour la parité byte avec FastMCP ; la valeur typée porte le `structuredContent`.
fn tool_ok<T: serde::Serialize>(value: &T) -> Result<(String, Value), RpcError> {
    let text =
        serde_json::to_string_pretty(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    let structured =
        serde_json::to_value(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    Ok((text, structured))
}

async fn tool_search_decisions(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let req = build_search_request(&args)?;
    let response = crate::search::search(state, &req)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    // Persistance d'activité best-effort (parité `record_search` côté Python,
    // source `mcp`) : enregistrée uniquement si un utilisateur MCP est résolu.
    if let Some(user_id) = user {
        record_search(&state.pool, user_id, &req, lj_dtos::ActivitySource::Mcp).await;
    }
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    tool_ok(&present_search_response(
        &response,
        &state.settings.web_base_url,
        &refs,
    ))
}

async fn tool_get_decision(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("missing tool argument: id"))?;
    let detail = crate::decisions::get_decision(state, id)
        .await
        .map_err(|e| {
            // NotFound côté Python → ToolError "decision not found".
            match e {
                ApiError::NotFound => {
                    RpcError::tool_error(format!("decision not found for id={id}"))
                }
                other => RpcError::tool_error(other.to_string()),
            }
        })?;
    // Consultation tracée best-effort (parité `record_decision_view`, source
    // `mcp`) quand un utilisateur MCP est résolu.
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
        RpcError::tool_error(
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

/// `get_law_article` : article de référentiel à une date (ou en vigueur si
/// `date` absente), par `code` (slug ou nom libre, résolu) + `num`. La réponse
/// inclut la timeline des versions (champ `versions`). 404 (code/article
/// inconnu) → `isError`.
async fn tool_get_law_article(state: &AppState, args: Value) -> Result<(String, Value), RpcError> {
    let code = required_str(&args, "code")?;
    let num = required_str(&args, "num")?;
    let date: Option<String> = deser(args.get("date").filter(|v| !v.is_null()).cloned())?;

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

/// `search_law_articles` : recherche plein-texte d'articles de référentiel
/// (ADR 0114), filtrable par `code`/`jurisdiction`. Renvoie le `total` exact et
/// une page de hits (extrait surligné + `url` + `num`), à chaîner ensuite vers
/// `get_law_article` pour le texte complet à date.
async fn tool_search_law_articles(
    state: &AppState,
    args: Value,
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

    let response = crate::legi::search_textes(
        state,
        &query,
        opt("code"),
        opt("jurisdiction"),
        None,
        None,
        limit,
        0,
    )
    .await
    .map_err(map_legi_error)?;

    tool_ok(&present_law_search(
        &query,
        &response,
        &state.settings.web_base_url,
    ))
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
        ApiError::NotFound => RpcError::tool_error("law article or code not found"),
        other => RpcError::tool_error(other.to_string()),
    }
}

/// Construit un [`SearchRequest`] à partir des arguments MCP (`ai_rerank` →
/// `ai_mode`, dates ISO conservées telles quelles ; filtres référentiels
/// solution/voie/office/legal_domain/jurisdiction_code/publication, ADR 0146).
fn build_search_request(args: &Value) -> Result<lj_dtos::SearchRequest, RpcError> {
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
        juridiction_type: deser(from_field("juridiction_type"))?,
        solution: deser(from_field("solution"))?,
        voie: deser(from_field("voie"))?,
        office: deser(from_field("office"))?,
        legal_domain: deser(from_field("legal_domain"))?,
        jurisdiction_code: deser(from_field("jurisdiction_code"))?,
        legal_instrument: deser(from_field("legal_instrument"))?,
        legal_article: deser(from_field("legal_article"))?,
        portee: deser(from_field("portee"))?,
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
            "annotations": activity_logging_annotations(),
        }),
        json!({
            "name": "get_decision",
            "title": "Get Decision",
            "description": GET_DECISION_DESC,
            "inputSchema": get_decision_schema(),
            "outputSchema": get_decision_output_schema(),
            "annotations": activity_logging_annotations(),
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
            "name": "get_law_article",
            "title": "Get Law Article",
            "description": GET_LAW_ARTICLE_DESC,
            "inputSchema": get_law_article_schema(),
            "outputSchema": get_law_article_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "search_law_articles",
            "title": "Search Law Articles",
            "description": SEARCH_LAW_ARTICLES_DESC,
            "inputSchema": search_law_articles_schema(),
            "outputSchema": search_law_articles_output_schema(),
            "annotations": read_only_annotations(),
        }),
    ]
}

/// Annotations communes des outils en lecture seule (port de `_READ_ONLY_TOOL`).
fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

/// Annotations des outils qui, au-delà de la lecture, écrivent un log d'activité
/// privé de l'utilisateur (`record_search` append, `record_decision_view`
/// upsert, gatés par `track_activity`). Donc : pas strictement `readOnly`, et
/// non `idempotent` (chaque appel ajoute/rafraîchit une entrée). Aucune écriture
/// externe ou publique (`openWorld: false`), rien de destructif. La revue
/// ChatGPT Apps exige que les trois hints reflètent le comportement réel.
fn activity_logging_annotations() -> Value {
    json!({
        "readOnlyHint": false,
        "destructiveHint": false,
        "idempotentHint": false,
        "openWorldHint": false,
    })
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
            "juridiction_type": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": enum_values(JURIDICTION_TYPE_CODES)}}),
                json!({"description": "Restrict to one or more court categories. Code glosses: TA = tribunal administratif, CAA = cour administrative d'appel, CE = Conseil d'État, CC = Cour de cassation, CA = cour d'appel, TJ = tribunal judiciaire, TCOM = tribunal de commerce."}),
            ),
            "solution": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Solution::ALL)}}),
                json!({"description": "Filter by the ruling of the operative part (référentiel solution, ADR 0146). REJET / IRRECEVABILITE / DESISTEMENT / NON_LIEU_A_STATUER are procedural or negative endings; CONFIRMATION / INFIRMATION* / REFORMATION are appeal outcomes; CASSATION* is cassation-specific; ANNULATION covers administrative annulment; SATISFACTION_TOTALE / SATISFACTION_PARTIELLE cover first-instance civil rulings granting the claim."}),
            ),
            "voie": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Voie::ALL)}}),
                json!({"description": "Filter by procedural track (référés, QPC, EU referral, révision, tierce opposition…). Absent value = ordinary contentious procedure."}),
            ),
            "office": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Office::ALL)}}),
                json!({"description": "Filter by specialised judge/office (JLD, JAF, JCP, JEX, juge des enfants, premier président, magistrat désigné). Absent value = ordinary bench."}),
            ),
            "legal_domain": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Domaine::ALL)}}),
                json!({"description": "Filter by legal domain (curated domain tree, ADR 0146): 9 roots (CIVIL, COMMERCIAL, PUBLIC, SOCIAL, FISCAL, PROPRIETE_INTELLECTUELLE, EUROPEEN, CRIMINEL, CONSTITUTIONNEL) and their leaves (e.g. CIVIL_DROIT_LOCATIF). Selecting a root also matches all its leaves."}),
            ),
            "publication": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes_str(PUBLICATION_CODES)}}),
                json!({"description": "Filter by publication level (any-of, référentiel publication): PUBLIE_BULLETIN / INEDIT_BULLETIN (Cour de cassation), PUBLIE_LEBON / MENTIONNE_LEBON / INEDIT_LEBON (administrative), AUTRE (no publication statement in the source)."}),
            ),
            "portee": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Portee::ALL)}}),
                json!({"description": "Filter by jurisprudential significance (any-of, ADR 0167), derived from publication codes at strongest rank: MAJEURE (rapport annuel / recueil Lebon), IMPORTANTE (bulletin, tables du Lebon, lettres de chambre, communiqués), LIMITEE (unpublished), INDETERMINEE (no publication statement — lower courts, European courts)."}),
            ),
            "jurisdiction_code": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 40}}),
                json!({"description": "Restrict to one or more precise jurisdiction units by referential code (e.g. \"ca_paris\", \"tj76351\", \"cass_soc\", \"ce\"). Use juridiction_type for the broad category; codes come from the juridiction facet of a previous search."}),
            ),
            "legal_instrument": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 100}}),
                json!({"description": "Restrict to decisions citing one or more given codes or statutes (e.g. \"Code civil\", \"Code de procédure civile\", \"Code de justice administrative\")."}),
            ),
            "legal_article": nullable(
                json!({"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 140}}),
                json!({"description": "Restrict to decisions citing a specific article of a specific code, as a composite key \"<instrument>|<article>\" (e.g. \"Code civil|1240\", \"Code de procédure civile|700\", \"Code de justice administrative|L761-1\"). The instrument prefix is required — the same article number exists in several codes."}),
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
        "required": ["id"],
        "properties": {
            "id": {
                "type": "string",
                "minLength": 1,
                "description": "Public ID of the decision, as returned in search_decisions.hits[].id.",
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

/// Schéma d'entrée de `get_law_article` (`{code, num}` + `date` optionnelle).
fn get_law_article_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["code", "num"],
        "properties": {
            "code": {
                "type": "string",
                "minLength": 1,
                "description": LAW_CODE_DESC,
            },
            "num": {
                "type": "string",
                "minLength": 1,
                "description": LAW_NUM_DESC,
            },
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD) — returns the version in force at that date. Omit for the version currently in force."}),
            ),
        },
    })
}

/// Schéma d'entrée de `search_law_articles` (`query` requis + filtres
/// `code`/`jurisdiction`/`nature`/`source` optionnels + `limit`).
fn search_law_articles_schema() -> Value {
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
                json!({"description": "Restrict to one code/text by its URL slug (e.g. \"code-civil\"). Omit to search the whole navigable referential."}),
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
        "required": ["query", "total", "hits"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["id", "title", "url", "preview", "chars", "jurisdiction"],
                    "properties": {
                        "id": {"type": "string"},
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "preview": {"type": "string"},
                        "chars": {"type": "integer"},
                        "jurisdiction": {"type": "string"},
                        "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "voie": nullable(json!({"type": "string"}), json!({})),
                        "office": nullable(json!({"type": "string"}), json!({})),
                        "legalDomain": nullable(json!({"type": "string"}), json!({})),
                        "publication": nullable(json!({"type": "string"}), json!({})),
                    },
                },
            },
        },
    })
}

/// Schéma de sortie de `get_decision` (= `McpDecisionDetail`).
fn get_decision_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["title", "url", "text"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "text": {"type": "string"},
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
        "required": ["id", "title", "url", "jurisdiction"],
        "properties": {
            "id": {"type": "string"},
            "title": {"type": "string"},
            "url": {"type": "string"},
            "summary": nullable(json!({"type": "string"}), json!({})),
            "jurisdiction": {"type": "string"},
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

/// Schéma de sortie de `get_law_article` (= `McpLawArticle`).
fn get_law_article_output_schema() -> Value {
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
            "text": nullable(json!({"type": "string"}), json!({})),
            "sourceUrl": nullable(json!({"type": "string"}), json!({})),
            "nota": nullable(json!({"type": "string"}), json!({})),
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

/// Schéma de sortie de `search_law_articles` (= `McpLawSearchResponse`).
fn search_law_articles_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "snippet", "code", "codeTitle", "num", "source"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"},
                        "code": {"type": "string"},
                        "codeTitle": {"type": "string"},
                        "num": {"type": "string"},
                        "titlePath": nullable(json!({"type": "string"}), json!({})),
                        "source": {"type": "string"},
                    },
                },
            },
        },
    })
}

// ── Codes d'enum (alignés sur les renames serde lj-core) ─────────────────────

const JURIDICTION_TYPE_CODES: &[&str] = &["TA", "CAA", "CE", "CC", "CA", "TJ", "TCOM"];

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
shortlist with `title`, public `url`, `preview` (in semantic search, \
a summary of what the decision is about; in keyword search, the \
matched passage showing where your terms appear), metadata, and \
an opaque `id` to chain into get_decision — not the full text (use \
get_decision for that). \
Prefer the structured filters (jurisdiction, dates, articles, codes) \
over expressing constraints in the query text. \
Filter logic: multiple values on the same filter are OR'd; different \
filters are AND'd together.";

const GET_DECISION_DESC: &str = "Fetch the full text of a decision by its `id` (from \
search_decisions.hits[].id), together with `title` and public \
`url`. Call after search_decisions to read in full any decision \
whose preview looks promising.";

const GET_LAW_ARTICLE_DESC: &str = "Fetch a French statutory article as it read on a given date \
(law-at-date), by code and number. Returns its full text, status, the \
dates bounding that version, and the version timeline. Omit `date` for \
the version currently in force. Usually chained from \
search_law_articles: pass its `code` and `num`.";

const SEARCH_LAW_ARTICLES_DESC: &str =
    "Search French statutory articles by meaning and keywords; returns a \
ranked shortlist plus the exact `total`. Prefer the structured filters \
(`code`, `jurisdiction`) over constraints in the query text. Chain a \
hit into get_law_article with its `code` and \
`num` to read the full text at a given date.";

const LAW_CODE_DESC: &str = "The code or text to look up. Accepts either the URL slug \
(e.g. \"code-civil\", \"code-de-procedure-civile\") or a free-form name \
(e.g. \"code civil\", \"Code de procédure civile\") — the server resolves \
it canonically, falling back to a fuzzy title match.";

const LAW_NUM_DESC: &str = "Article number as cited (e.g. \"1240\", \"L131-4\", \"L. 761-1\"), \
or the `num` returned by search_law_articles. Canonicalised \
server-side, so spacing/punctuation variants resolve to the same article.";

/// Description inline de `McpDecisionRef` dans l'outputSchema d'activité (port
/// verbatim de la docstring, retours à la ligne `\n` inclus — Pydantic les
/// conserve tels quels dans le schéma émis).
const DECISION_REF_DESC: &str =
    "Référence décision pour signets/consultations : assez pour citer ou\n\
chaîner vers ``get_decision`` via ``id``, sans le texte intégral.";

/// Description de `McpActivityResponse` (port verbatim de la docstring du modèle,
/// retours à la ligne inclus).
const ACTIVITY_RESPONSE_DESC: &str =
    "Sortie unique de ``list_my_activity`` : chaque tranche n'est remplie que\n\
si elle a été demandée (``kind`` ciblé ou ``all``). Un seul modèle à plat\n\
(plutôt qu'une union par ``kind``) évite une enveloppe ``result``. Noter\n\
qu'après inlining des ``$ref`` par ``_clean_schema`` (cf. mcp_server),\n\
``McpDecisionRef`` apparaît deux fois dans le schéma émis — une fois sous\n\
``bookmarks``, une fois sous ``readingHistory``.";

const LIST_ACTIVITY_DESC: &str = "List the authenticated user's own activity, most recent first. \
`kind` selects which tab: 'searches' (past queries with their \
structured `filters` and `source`), 'bookmarks' or 'readingHistory' \
(decisions, each with `title`, `url`, a `summary` and the opaque \
`id` to chain into get_decision), or 'all' to get the three at \
once. Only the requested tab(s) are populated. In readingHistory, \
`lastSource` == 'web' means the user opened the decision manually \
at least once (a genuine read); 'mcp' means it was only ever \
opened through this connector. Requires a connected account.";

const SEARCH_QUERY_DESC: &str = "French query, the primary input. Two regimes — pick the right \
tool for the job: \
(a) Natural language or descriptive keywords for legal-issue \
searches — the engine handles synonyms and reformulations, no \
need to enumerate variants. Examples: \
« responsabilité hôpital infection nosocomiale » ; \
« étranger malade soins inaccessibles dans son pays d'origine » ; \
« licenciement discrimination syndicale charge de la preuve ». \
(b) Quoted exact phrases and ET / OU / SAUF operators when \
targeting a specific named entity (company, municipality, \
person) or a precise legal formula — exact matching cuts \
semantic noise. Examples: \
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
                "get_law_article",
                "search_law_articles",
            ]
        );
    }

    #[test]
    fn build_request_maps_referential_filters_and_ai_rerank() {
        let args = json!({
            "query": "bail commercial",
            "publication": ["PUBLIE_BULLETIN"],
            "juridiction_type": ["CC", "CA"],
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
            Some(vec![lj_dtos::Domaine::CivilDroitLocatif])
        );
        assert_eq!(req.jurisdiction_code, Some(vec!["ca_paris".to_string()]));
        assert_eq!(
            req.juridiction_type,
            Some(vec![
                lj_dtos::JuridictionType::Cc,
                lj_dtos::JuridictionType::Ca
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
    fn initialize_carries_protocol_and_instructions() {
        let res = initialize_result();
        assert_eq!(res["protocolVersion"], json!(PROTOCOL_VERSION));
        assert_eq!(res["serverInfo"]["name"], json!(SERVER_NAME));
        assert!(res["instructions"]
            .as_str()
            .unwrap()
            .contains("French case law"));
    }
}
