//! Remote MCP server (Streamable HTTP, JSON response) mounted under `/mcp`.
//! The JSON-RPC layer is hand-rolled on top of axum.

use axum::{
    extract::{Request, State},
    http::header::{AUTHORIZATION, WWW_AUTHENTICATE},
    http::{HeaderMap, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use lj_mcp_tools::{negotiate_protocol_version, SERVER_INSTRUCTIONS};
use lj_vocab::ZONE_SANS;
use opentelemetry::KeyValue;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::Instrument;

use crate::error::ApiError;
use crate::mcp_presenters::{
    present_decision_detail, present_journal_search, present_law_article, present_law_search,
    present_law_text, present_publication_notice, present_search_response, present_tender_notice,
};
use crate::state::AppState;

#[derive(Clone, Copy)]
struct Provenance<'a> {
    first_party: bool,
    user_agent: Option<&'a str>,
    request: &'a crate::usage::Request,
}

fn mcp_caller(
    user: Option<&str>,
    prov: Provenance<'_>,
    pseudo: Option<&lj_identity::Pseudonymiser>,
) -> crate::usage::Caller {
    crate::usage::Caller {
        source: lj_dtos::ActivitySource::Mcp,
        authenticated: user.is_some(),
        context: lj_dtos::SearchContext::User,
        first_party: prov.first_party,
        caller: mcp_pseudonym(user, pseudo),
        visit: None,
        user_agent: prov.user_agent.map(str::to_owned),
        request: prov.request.clone(),
    }
}

fn mcp_pseudonym(
    user: Option<&str>,
    pseudo: Option<&lj_identity::Pseudonymiser>,
) -> Option<String> {
    pseudo
        .zip(user)
        .and_then(|(p, sub)| p.caller(&lj_identity::Identity::Account(sub.to_string())))
}

const SERVER_NAME: &str = "librejustice";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Served on both `/mcp/` and `/mcp`: MCP clients POST the URL as typed and do
/// not follow a redirect on POST.
pub fn mcp_router(state: AppState) -> Router {
    Router::new()
        .route("/mcp/", post(handle_rpc))
        .route("/mcp", post(handle_rpc))
        .layer(from_fn_with_state(state.clone(), mcp_auth))
        .with_state(state)
}

// ── MCP auth ─────────────────────────────────────────────────────────────────

/// `None` when MCP auth is off (`mcp_require_auth=false`, tests and local dev).
#[derive(Debug, Clone)]
struct McpUser(Option<String>);

/// The `WWW-Authenticate` challenge (RFC 9728 + MCP 2025-03-26) is what triggers
/// OAuth auto-discovery in clients.
fn mcp_unauthorized(challenge: &str, error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, challenge.to_string())],
        Json(json!({ "error": error })),
    )
        .into_response()
}

/// Timing breakdown of the MCP path, written into the response extensions by
/// [`mcp_auth`] and read back by the queue log ([`crate::routes`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct McpPhases {
    pub pool: Duration,
    pub lookup: Duration,
    /// `UPDATE users.last_seen_at`, zero within [`LAST_SEEN_REFRESH_MINUTES`].
    pub bump: Duration,
    /// Everything past auth: JSON-RPC layer and tool.
    pub dispatch: Duration,
    /// Canonical tool, aliases folded. `None` on protocol methods, where
    /// `dispatch` only measures the JSON-RPC envelope.
    pub tool: Option<&'static str>,
}

/// Carries the executed tool up to [`mcp_auth`], the only writer of [`McpPhases`].
#[derive(Debug, Clone, Copy)]
struct CalledTool(Option<&'static str>);

/// The token is read on every call: `users.last_seen_at` is rewritten once in this delay.
const LAST_SEEN_REFRESH_MINUTES: i32 = 60;

/// `None` when the token is absent from the table or expired. The comparison is
/// on the SHA-256 digest: the database no longer holds enough to impersonate a
/// bearer.
async fn lookup_mcp_user(
    state: &AppState,
    token: &str,
    phases: &mut McpPhases,
) -> Result<Option<String>, ApiError> {
    let digest = crate::oauth::sha256(token.as_bytes());
    let digest: &[u8] = &digest;
    let (conn, waited) = lj_pg::db::get_timed(&state.pool).await?;
    phases.pool = waited;
    let t = std::time::Instant::now();
    let row = conn
        .query_opt(
            "SELECT t.user_id, \
                    u.last_seen_at IS NULL \
                 OR u.last_seen_at < now() - make_interval(mins => $2::int) \
               FROM mcp_tokens t JOIN users u ON u.sub = t.user_id \
              WHERE t.access_token_sha256 = $1 AND t.expires_at > now()",
            &[&digest, &LAST_SEEN_REFRESH_MINUTES],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("mcp token lookup: {e}")))?;
    phases.lookup = t.elapsed();
    let Some(row) = row else {
        return Ok(None);
    };
    let user_id: String = row.get(0);
    if row.get::<_, bool>(1) {
        let t = std::time::Instant::now();
        if let Err(e) = conn
            .execute(
                "UPDATE users SET last_seen_at = now() WHERE sub = $1",
                &[&user_id],
            )
            .await
        {
            tracing::warn!("bump last_seen (mcp) failed: {e}");
        }
        phases.bump = t.elapsed();
    }
    Ok(Some(user_id))
}

async fn mcp_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mut user: Option<String> = None;
    let mut phases = McpPhases::default();
    let reseau = crate::ratelimit::caller_key(None, req.headers());
    if state
        .budgets
        .take(crate::ratelimit::Cost::Porte, reseau.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return ApiError::TooManyRequests.into_response();
    }
    if state.settings.mcp_require_auth {
        let base = state.settings.public_base_url.trim_end_matches('/');
        let challenge = format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
        );
        let auth = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let Some(token) = auth.strip_prefix("Bearer ") else {
            tracing::debug!("mcp: challenge served (no Bearer)");
            return mcp_unauthorized(&challenge, "auth_required");
        };
        match lookup_mcp_user(&state, token, &mut phases).await {
            Ok(Some(uid)) => user = Some(uid),
            Ok(None) => {
                tracing::warn!(
                    mcp_token_rejete = "invalid_token",
                    "MCP token refused: invalid_token (missing from the table or expired)"
                );
                return mcp_unauthorized(&challenge, "invalid_token");
            }
            Err(e) => {
                tracing::warn!(
                    mcp_token_rejete = "lookup_echec",
                    error = %e,
                    "MCP token unverifiable: lookup_failed"
                );
                return e.into_response();
            }
        }
    }
    req.extensions_mut().insert(McpUser(user));
    let t = std::time::Instant::now();
    let mut resp = next.run(req).await;
    phases.dispatch = t.elapsed();
    phases.tool = resp.extensions().get::<CalledTool>().and_then(|o| o.0);
    resp.extensions_mut().insert(phases);
    resp
}

// ── JSON-RPC layer ───────────────────────────────────────────────────────────

fn count_call(outil: &'static str, categorie: &'static str) {
    lj_telemetry::meter()
        .u64_counter("mcp.tool.calls")
        .with_description("Appels d'outils MCP, par outil et par catégorie de sortie.")
        .build()
        .add(
            1,
            &[
                KeyValue::new("tool", outil),
                KeyValue::new("category", categorie),
            ],
        );
}

/// What the tool error contract tells the model to do next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// Invalid arguments: fix the call, an identical retry will fail again.
    Validation,
    /// Absent from the corpus, which never proves the resource does not exist.
    NotFound,
    /// The account's allowance for the minute is spent: wait, then retry.
    RateLimited,
    /// Internal or transient failure: an identical retry may succeed.
    Internal,
}

impl ErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
    category: ErrorCategory,
    /// `true` when an identical retry may succeed.
    retryable: bool,
    /// Next action suggested to the model.
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
    fn tool_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Internal,
            retryable: true,
            hint: None,
        }
    }
    /// The `hint` must restate that absence from the corpus does not prove
    /// non-existence.
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::NotFound,
            retryable: false,
            hint: None,
        }
    }
    /// `allowance` names what the minute holds, "30 searches".
    fn rate_limited(allowance: String) -> Self {
        Self {
            code: -32603,
            message: format!("rate limited: {allowance} per minute per account"),
            category: ErrorCategory::RateLimited,
            retryable: true,
            hint: None,
        }
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

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
    let request = crate::usage::Request::read(&headers);
    let prov = Provenance {
        first_party: crate::usage::is_first_party(&headers, &state.settings),
        user_agent: crate::usage::user_agent(&headers),
        request: &request,
    };

    let is_notification = body.get("id").is_none();

    let mut outil = None;
    let result = dispatch(
        &state,
        method,
        params,
        user.as_deref(),
        session.as_deref(),
        prov,
        &mut outil,
    )
    .await;
    let mut resp = if is_notification {
        StatusCode::ACCEPTED.into_response()
    } else {
        match result {
            Ok(result) => {
                Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
            }
            Err(err) => Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": err.code, "message": err.message},
            }))
            .into_response(),
        }
    };
    resp.extensions_mut().insert(CalledTool(outil));
    resp
}

/// Conversation correlator: a hash of the `Mcp-Session-Id` header, never the
/// user's identity.
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

async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
    prov: Provenance<'_>,
    outil: &mut Option<&'static str>,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result(&params)),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": lj_mcp_tools::tool_definitions() })),
        "tools/call" => call_tool(state, params, user, session, prov, outil).await,
        other => Err(RpcError::method_not_found(other)),
    }
}

fn initialize_result(params: &Value) -> Value {
    json!({
        "protocolVersion": negotiate_protocol_version(params["protocolVersion"].as_str()),
        "capabilities": {
            "experimental": {},
            "tools": {"listChanged": false},
        },
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "instructions": SERVER_INSTRUCTIONS,
    })
}

// ── Tools: dispatch ──────────────────────────────────────────────────────────

async fn call_tool(
    state: &AppState,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
    prov: Provenance<'_>,
    outil: &mut Option<&'static str>,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let span = tracing::info_span!(
        "mcp_tool_call",
        librejustice.mcp.tool = name,
        librejustice.mcp.authenticated = user.is_some(),
        librejustice.mcp.origin = crate::usage::origin_value(prov.first_party),
        librejustice.mcp.session = tracing::field::Empty,
        librejustice.mcp.arg = tracing::field::Empty,
        librejustice.mcp.outcome = tracing::field::Empty,
        librejustice.mcp.error_category = tracing::field::Empty,
        librejustice.mcp.error = tracing::field::Empty,
        librejustice.search.caller = mcp_pseudonym(user, state.pseudonymiser.as_ref()).as_deref(),
        user_agent.original = prov.user_agent,
        librejustice.request.headers = prov.request.headers.as_deref(),
        librejustice.request.worker = prov.request.worker.as_deref(),
    );
    if let Some(s) = session {
        span.record("librejustice.mcp.session", s);
    }
    let arg: String = arguments.to_string().chars().take(256).collect();
    span.record("librejustice.mcp.arg", arg.as_str());

    let lecture = read_kind(lj_mcp_tools::canonical_tool_name(name), &arguments);
    let retired = lj_mcp_tools::canonical_tool_name(name) == "search_decisions"
        && crate::search::RETIRED_FILTERS
            .iter()
            .any(|k| arguments.get(*k).is_some_and(|v| !v.is_null()));
    async move {
        // MCP semantics: a tool execution failure is not a JSON-RPC error but a
        // result with `isError: true` and the message in `content`. Only protocol
        // errors stay JSON-RPC errors.
        //
        // The counter label is a literal, never `name`: a client calling made-up
        // names would create one series per name.
        let (label, result) = match lj_mcp_tools::canonical_tool_name(name) {
            "search_decisions" => (
                "search_decisions",
                tool_search_decisions(state, arguments, user, prov).await,
            ),
            "get_decision" => (
                "get_decision",
                tool_get_decision(state, arguments, user).await,
            ),
            "get_norm" => ("get_norm", tool_get_norm(state, arguments, user).await),
            "search_norms" => (
                "search_norms",
                tool_search_norms(state, arguments, user, prov).await,
            ),
            "search_notices" => (
                "search_notices",
                tool_search_notices(state, arguments, user).await,
            ),
            "get_notice" => ("get_notice", tool_get_notice(state, arguments, user).await),
            removed if lj_mcp_tools::REMOVED_NAMES.contains(&removed) => (
                "removed",
                Err(RpcError::invalid_params(format!("{removed} was removed."))),
            ),
            other => (
                "unknown",
                Err(RpcError::invalid_params(format!("Unknown tool: {other}"))),
            ),
        };

        *outil = Some(label);

        let span = tracing::Span::current();
        match result {
            Ok((text, structured)) => {
                span.record("librejustice.mcp.outcome", "ok");
                count_call(label, "ok");
                if let Some(kind) = lecture {
                    let caller = mcp_caller(user, prov, state.pseudonymiser.as_ref());
                    let document = structured.get("url").and_then(Value::as_str);
                    crate::reads::served(
                        kind,
                        crate::reads::Surface::Mcp,
                        &caller,
                        &arg,
                        document.unwrap_or_default(),
                    );
                }
                let mut content = vec![json!({"type": "text", "text": text})];
                if retired {
                    tracing::warn!("mcp: retired filters answered without them");
                    content.push(
                        json!({"type": "text", "text": crate::search::RETIRED_FILTERS_WARNING}),
                    );
                }
                Ok(json!({
                    "content": content,
                    "structuredContent": structured,
                    "isError": false,
                }))
            }
            Err(e) => {
                span.record("librejustice.mcp.outcome", "error");
                span.record("librejustice.mcp.error_category", e.category.as_str());
                span.record("librejustice.mcp.error", e.message.as_str());
                count_call(label, e.category.as_str());
                if e.category == ErrorCategory::Internal {
                    tracing::error!(
                        librejustice.mcp.tool = label,
                        librejustice.mcp.error = e.message.as_str(),
                        "mcp: internal tool failure"
                    );
                }
                Ok(tool_error_content(&e))
            }
        }
    }
    .instrument(span)
    .await
}

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

/// One `Read` token on the OAuth account; with no bearer the caller is not limited.
async fn lecture(state: &AppState, user: Option<&str>) -> Result<(), RpcError> {
    let key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());
    match state
        .budgets
        .take(crate::ratelimit::Cost::Read, key.as_ref())
        .await
    {
        crate::ratelimit::Verdict::Allowed => Ok(()),
        crate::ratelimit::Verdict::Exhausted => Err(RpcError::rate_limited(format!(
            "{} reads",
            crate::ratelimit::READ_PER_MIN
        ))),
    }
}

/// Same JSON in both channels, compact: the text block is what hosts inject
/// into the model.
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
    prov: Provenance<'_>,
) -> Result<(String, Value), RpcError> {
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    let mut args = args;
    fold_retired_chamber(&mut args, &refs);
    let mut req = build_search_request(&args)?;
    // A code outside the referential would silently match zero decisions.
    if let Some(codes) = req.jurisdiction_code.as_deref_mut() {
        relax_jurisdiction_codes(codes, &refs);
    }
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(unknown_jurisdiction_code(bad, &refs));
    }
    if let Some(seats) = req.seat.as_mut() {
        crate::referential::canonicalize_seats(seats);
        for path in seats.iter_mut().filter(|p| !refs.has_seat(p)) {
            if let Some(complet) = crate::referential::seat_completion(path, refs.seat_nodes()) {
                tracing::warn!(
                    librejustice.mcp.relaxed_field = "seat",
                    librejustice.mcp.relaxed_value = path.as_str(),
                    librejustice.mcp.relaxed_slug = complet,
                    "mcp: seat path recovered from its abbreviated segment"
                );
                *path = complet.to_string();
            } else if let Some(node) =
                crate::referential::seat_under_its_order(path, refs.seat_nodes())
            {
                tracing::warn!(
                    librejustice.mcp.relaxed_field = "seat",
                    librejustice.mcp.relaxed_value = path.as_str(),
                    librejustice.mcp.relaxed_slug = node,
                    "mcp: seat path recovered under its legal order"
                );
                *path = node.to_string();
            }
        }
    }
    if let Some(bad) = req.seat.iter().flatten().find(|p| !refs.has_seat(p)) {
        return Err(
            RpcError::invalid_params(crate::referential::unknown_seat_msg(bad, refs.seat_nodes()))
                .with_hint(
                    "Pass paths from the seat facet of a previous search. A path matches \
                 itself and everything below it, so start short and go deeper."
                        .to_string(),
                ),
        );
    }
    if let Some(bad) = req
        .formation
        .iter()
        .flatten()
        .find(|p| !lj_core::formation::is_formation_path(p))
    {
        return Err(RpcError::invalid_params(
            crate::referential::unknown_formation_msg(bad),
        ));
    }
    // Same guarantee for the controlled-vocabulary axes.
    if let Some(bad) = req
        .publication
        .as_deref()
        .and_then(|vs| refs.find_unknown_token("publication", vs))
    {
        let tokens = refs.facet_tokens("publication");
        return Err(RpcError::invalid_params(format!(
            "unknown publication '{bad}'; valid values: {}",
            tokens.join(", ")
        ))
        .with_hint(list_hint("publication", &tokens)));
    }
    // The columns carry catalogue uids, not names.
    resolve_instrument_filters(state, &mut req).await?;
    // Budget per MCP account: the OAuth token is the key, and it is revocable.
    // With no bearer the caller is not identifiable, so not limited.
    let budget_key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());
    // The count is resolved before answering: an agent calls once, and the total
    // is what tells it whether its window covers the matched set.
    let response = crate::search::search_with_count(
        state,
        &req,
        mcp_caller(user, prov, state.pseudonymiser.as_ref()),
        budget_key.as_ref(),
    )
    .await
    .map_err(|e| match e {
        ApiError::TooManyRequests => {
            let mut refus = RpcError::rate_limited(format!(
                "{} searches",
                state.settings.search_budget_per_min
            ));
            if crate::search::budget_cost(&req) == crate::ratelimit::Cost::Reference {
                refus
                    .message
                    .push_str(", a reference lookup counting as half");
                refus = refus.with_hint("Several docket numbers or ECLIs fit in one query.");
            }
            refus
        }
        other => RpcError::tool_error(other.to_string()),
    })?;
    let out = present_search_response(
        &response,
        &state.settings.web_base_url,
        req.seat.as_deref().unwrap_or_default(),
    );
    tool_ok(&out)
}

async fn tool_get_decision(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    lecture(state, user).await?;
    let url = required_str(&args, "url")?;
    // The key is the last segment of the public url `/decision/{juris}/{key}`.
    let id = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("not a decision url: {url}")))?;
    let id = juritext_public_id(state, id).await?;
    let detail = crate::decisions::get_decision(state, &id, None)
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
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    tool_ok(&present_decision_detail(
        &detail,
        &state.settings.web_base_url,
        &refs,
    ))
}

/// A Légifrance `JURITEXT` names the decision one of the three judicial funds
/// wrote it under; a collection cut in sections names several.
async fn juritext_public_id(state: &AppState, key: &str) -> Result<String, RpcError> {
    let is_juritext = key
        .strip_prefix("JURITEXT")
        .is_some_and(|n| n.len() == 12 && n.bytes().all(|b| b.is_ascii_digit()));
    if !is_juritext {
        return Ok(key.to_string());
    }
    let uids: Vec<String> = ["dila-cass", "dila-inca", "dila-capp"]
        .iter()
        .flat_map(|fund| lj_parse::judicial_record_uids(fund, key))
        .collect();
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    let mut found = lj_store::repository::DecisionRepository::new(&conn)
        .decisions_by_source_uid(&uids)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    let mut seen = std::collections::HashSet::new();
    found.retain(|(_, public_id)| seen.insert(public_id.clone()));
    match found.as_slice() {
        [] => Ok(key.to_string()),
        [(_, public_id)] => {
            tracing::warn!(
                librejustice.mcp.relaxed_field = "url",
                librejustice.mcp.relaxed_value = key,
                librejustice.mcp.relaxed_slug = public_id.as_str(),
                "mcp: decision recovered from its Legifrance identifier"
            );
            Ok(public_id.clone())
        }
        several => {
            let web = &state.settings.web_base_url;
            let urls: Vec<String> = several
                .iter()
                .map(|(_, public_id)| format!("{web}/decision/{public_id}"))
                .collect();
            Err(RpcError::invalid_params(format!(
                "{key} is a collection of {} decisions: {}",
                urls.len(),
                urls.join(" ")
            )))
        }
    }
}

// ── Law-at-date tool ─────────────────────────────────────────────────────────

async fn tool_get_norm(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    lecture(state, user)
        .await
        .map_err(|e| e.with_hint("A section's url returns all its articles in one call."))?;
    read_legal_text(state, args).await
}

async fn read_legal_text(state: &AppState, args: Value) -> Result<(String, Value), RpcError> {
    let date = as_of(&args)?;

    let (juris, code, num) = match law_target(&args)? {
        LawTarget::Article { juris, code, num } => (juris, code, num),
        LawTarget::Root { juris, code } => {
            return tool_get_legal_root(state, &code, juris.as_deref()).await
        }
        LawTarget::Section { juris, code, path } => {
            return tool_get_legal_section(state, &code, juris.as_deref(), &path, date.as_deref())
                .await
        }
        LawTarget::SectionOf { juris, code, cid } => {
            return tool_get_section_cid(state, args, &code, juris.as_deref(), &cid).await
        }
        LawTarget::SourceUid(uid) => {
            let (code, num) = resolve_source_uid(state, &uid).await?;
            if num.is_empty() {
                return tool_get_legal_root(state, &code, None).await;
            }
            (None, code, num)
        }
        LawTarget::TextUid(uid) => {
            let (code, juris) = resolve_text_uid(state, &uid).await?;
            return tool_get_legal_root(state, &code, Some(&juris)).await;
        }
        LawTarget::SectionCid(cid) => {
            let (code, juris) = resolve_section_cid(state, &cid).await?;
            return tool_get_section_cid(state, args, &code, Some(&juris), &cid).await;
        }
    };
    let juris = juris.as_deref();
    let segment = num.clone();
    let num = lj_norm::article_key::article_key(&num);

    let code = resolve_law_code(state, &code, juris).await?;

    let at = date
        .as_deref()
        .map(|d| parse_mcp_date("as_of", d))
        .transpose()?;
    let detail = match at {
        Some(parsed) => crate::legi::article_at_date(state, &code, juris, &num, parsed, None).await,
        None => crate::legi::article_in_force(state, &code, juris, &num, None).await,
    };
    let detail = match detail {
        Ok(detail) => detail,
        Err(ApiError::NotFound) => {
            // One segment names a first-level section when no article answers it.
            return match crate::legi::law_section(state, &code, juris, &segment, at).await {
                Ok(section) => tool_ok(&crate::mcp_presenters::present_law_section(
                    &section,
                    &state.settings.web_base_url,
                )),
                Err(_) => Err(article_not_found(state, &code, juris, &num).await),
            };
        }
        Err(other) => return Err(map_legi_error(other)),
    };
    tool_ok(&present_law_article(&detail, &state.settings.web_base_url))
}

/// A section named by its Légifrance id is read at the address of its node,
/// which may be an article or the text itself.
async fn tool_get_section_cid(
    state: &AppState,
    mut args: Value,
    code: &str,
    juris: Option<&str>,
    cid: &str,
) -> Result<(String, Value), RpcError> {
    let slug = resolve_law_code(state, code, juris).await?;
    let address = crate::legi::section_address(state, &slug, juris, cid)
        .await
        .map_err(map_legi_error)?
        .ok_or_else(|| {
            RpcError::not_found(format!("unknown Légifrance id '{cid}'")).with_hint(
                "This section is not in the text's table of contents today: read the text \
                 by its url and follow its `section` entries.",
            )
        })?;
    let Some(obj) = args.as_object_mut() else {
        return Err(RpcError::invalid_params("arguments must be an object"));
    };
    for key in ID_KEYS
        .iter()
        .chain(CODE_KEYS.iter())
        .chain(ARTICLE_KEYS.iter())
    {
        obj.remove(*key);
    }
    obj.insert("url".to_string(), Value::String(address));
    Box::pin(read_legal_text(state, args)).await
}

async fn tool_search_norms(
    state: &AppState,
    args: Value,
    user: Option<&str>,
    prov: Provenance<'_>,
) -> Result<(String, Value), RpcError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.chars().count() > lj_dtos::QUERY_MAX_CHARS {
        return Err(RpcError::invalid_params(query_too_long()));
    }
    let opt = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let limit = clamp_search_limit(&args)?;

    let code = match opt("code") {
        Some(c) => Some(resolve_law_code(state, c, None).await?),
        None => None,
    };

    let at = as_of(&args)?
        .map(|d| parse_mcp_date("as_of", &d))
        .transpose()?;
    if let Some(moved) = ["date_from", "date_to"]
        .into_iter()
        .find(|k| args.get(*k).is_some())
    {
        return Err(
            RpcError::invalid_params(format!("unknown field '{moved}'")).with_hint(
                "Norms are bounded by the entry into force of their versions: \
                 `valid_from` the earliest, `as_of` the latest (today when omitted).",
            ),
        );
    }
    let valid_from = deser_field::<String>(&args, "valid_from")?
        .map(|d| parse_mcp_date("valid_from", &d))
        .transpose()?;
    if valid_from.is_some_and(|from| from > at.unwrap_or_else(|| chrono::Utc::now().date_naive())) {
        return Err(RpcError::invalid_params(
            "valid_from must not be after as_of, which defaults to today",
        ));
    }
    let jurisdictions: Vec<String> = opt("jurisdiction")
        .map(|j| lj_norm::jurisdictions::union_as_eu(j.to_uppercase()))
        .into_iter()
        .collect();
    let scope = opt("scope");
    let caller = mcp_caller(user, prov, state.pseudonymiser.as_ref());
    let budget_key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());

    let response = match query.as_str() {
        "" => {
            crate::legi::list_textes(
                state,
                code.as_deref(),
                &jurisdictions,
                scope,
                valid_from,
                at,
                limit,
                0,
                caller,
                budget_key.as_ref(),
            )
            .await
        }
        query => {
            crate::legi::search_textes_counted(
                state,
                query,
                code.as_deref(),
                // The connector names no place, so it reads from the default one,
                // as `get_norm` does.
                &[],
                &jurisdictions,
                None,
                scope,
                at,
                valid_from,
                limit,
                0,
                // A citation with no code named has no other door: `get_norm`
                // wants the code alongside the article.
                lj_dtos::SearchMode::Auto,
                caller,
                budget_key.as_ref(),
            )
            .await
        }
    }
    .map_err(|e| match e {
        ApiError::TooManyRequests => {
            RpcError::rate_limited(format!("{} searches", state.settings.search_budget_per_min))
        }
        other => map_legi_error(other),
    })?;

    tool_ok(&present_law_search(
        &query,
        &response,
        &state.settings.web_base_url,
    ))
}

/// Facets only: this pot has neither BM25 nor embeddings, and `query` is a word
/// filter.
async fn tool_search_notices(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let strings = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_uppercase)
                    .collect()
            })
            .unwrap_or_default()
    };
    let limit = clamp_search_limit(&args)?;
    lecture(state, user).await?;
    let zone = strings("zone");
    // A code outside the nomenclature filters nothing: a plain error rather than
    // an empty page a model would read as "no tender at all".
    if let Some(bad) = zone.iter().find(|c| {
        c.as_str() != ZONE_SANS.to_uppercase()
            && lj_annonces::zones::Zone::parse(&c.to_lowercase()).is_none()
    }) {
        return Err(RpcError::invalid_params(format!(
            "zone: {bad:?} is not a département code (\"75\", \"2A\", \"973\"), \"fr\" or \"eu\""
        )));
    }
    let zone: Vec<String> = zone
        .into_iter()
        .map(|c| match c.as_str() {
            "FR" | "EU" | "SANS" => c.to_lowercase(),
            _ => c,
        })
        .collect();
    let famille = strings("nature");
    if let Some(bad) = famille
        .iter()
        .find(|f| !lj_mcp_tools::TENDER_NATURES.contains(&f.as_str()))
    {
        return Err(RpcError::invalid_params(format!(
            "nature: {bad:?} is not one of {:?}",
            lj_mcp_tools::TENDER_NATURES
        )));
    }
    let q = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let cpv = strings("cpv");
    let echeance_jours = args
        .get("closing_within_days")
        .and_then(Value::as_u64)
        .map(|j| j as u32);
    let fond = notice_funds(
        args.get("fund").and_then(Value::as_array),
        q.is_some(),
        !famille.is_empty() || !cpv.is_empty() || echeance_jours.is_some(),
    )?;
    let jurisdiction = args
        .get("jurisdiction")
        .and_then(Value::as_str)
        .filter(|j| !j.trim().is_empty())
        .map(str::to_string);
    let req = lj_dtos::JournalSearchRequest {
        q,
        fond,
        zone,
        famille,
        cpv,
        echeance_jours,
        jurisdiction,
        sort: lj_dtos::JournalSort::Echeance,
        limit: limit as u32,
        ..lj_dtos::JournalSearchRequest::default()
    };
    let response = crate::journals::search(state, &req)
        .await
        .map_err(|e| match e {
            ApiError::Unprocessable(v) => RpcError::invalid_params(format!(
                "jurisdiction: {}",
                v["msg"].as_str().unwrap_or_default()
            )),
            other => RpcError::tool_error(other.to_string()),
        })?;

    tool_ok(&present_journal_search(
        &response,
        &state.settings.web_base_url,
    ))
}

/// The journals a `search_notices` call reads. Without `fund`, the Journal officiel
/// answers a question only, its newest acts naming persons, and a filter only
/// procurement runs reads procurement alone.
fn notice_funds(
    fund: Option<&Vec<Value>>,
    question: bool,
    procurement_filter: bool,
) -> Result<Vec<String>, RpcError> {
    let Some(fund) = fund else {
        return Ok(match procurement_filter {
            true => vec![lj_vocab::JOURNAL_FOND_MARCHE.to_string()],
            false => lj_mcp_tools::NOTICE_FUNDS
                .iter()
                .filter(|f| question || **f != lj_vocab::JOURNAL_FOND_JO)
                .map(|f| f.to_string())
                .collect(),
        });
    };
    let fund: Vec<String> = fund
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect();
    match fund
        .iter()
        .find(|f| !lj_mcp_tools::NOTICE_FUNDS.contains(&f.as_str()))
    {
        Some(bad) => Err(RpcError::invalid_params(format!(
            "fund: {bad:?} is not one of {:?}",
            lj_mcp_tools::NOTICE_FUNDS
        ))),
        None => Ok(fund),
    }
}

/// One notice read whole, from its `/notice/{place}/{uid}` url.
async fn tool_get_notice(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    lecture(state, user).await?;
    let url = required_str(&args, "url")?;
    let uid = notice_uid(&url).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "not a notice url (expected …/notice/{{place}}/{{uid}}): {url}"
        ))
    })?;
    let not_found = |e: ApiError| match e {
        ApiError::NotFound => RpcError::not_found(format!("no notice at url={url}")).with_hint(
            "Notice urls cannot be composed or guessed: take the url verbatim from a \
             search_notices hit.",
        ),
        other => RpcError::tool_error(other.to_string()),
    };
    let base = &state.settings.web_base_url;
    match uid.starts_with("marche:") {
        true => tool_ok(&present_tender_notice(
            &crate::journals::detail(state, uid)
                .await
                .map_err(not_found)?,
            base,
        )),
        false => tool_ok(&present_publication_notice(
            &crate::notices::notice(state, uid)
                .await
                .map_err(not_found)?,
            base,
        )),
    }
}

/// The uid a notice url names: everything after its place, slashes included.
fn notice_uid(url: &str) -> Option<&str> {
    let (_, tail) = url.split_once("/notice/")?;
    let (_, uid) = tail.split_once('/')?;
    let uid = uid.split(['?', '#']).next()?.trim_end_matches('/');
    (!uid.is_empty()).then_some(uid)
}

/// Folds the retired `chamber` into `seat`, the court coming from the call.
///
/// A published ChatGPT app is served the metadata snapshot frozen at review, not
/// the live `tools/list`, so its users still send the field the seat axis
/// replaced. One value that does not resolve in the fonds leaves the whole call
/// to the unknown-field error, `FR.CC` plus `CIVILE` being the case that must keep
/// failing, the civil chambers being `FR.CC.C1_CIVILE` to `FR.CC.C3_CIVILE`.
fn fold_retired_chamber(args: &mut Value, refs: &crate::referential::Referential) {
    let Some(chambers) = args.get("chamber").and_then(as_str_list) else {
        return;
    };
    let trunks = seat_trunks(args, refs);
    if chambers.is_empty() || trunks.is_empty() {
        return;
    }
    let mut folded = Vec::new();
    for trunk in &trunks {
        for chamber in &chambers {
            let segment = if chamber.chars().all(|c| c.is_ascii_digit()) {
                format!("C{chamber}")
            } else {
                chamber.to_uppercase()
            };
            let path = format!("{trunk}.{segment}");
            if !refs.has_seat(&path) {
                return;
            }
            folded.push(path);
        }
    }
    tracing::warn!(
        librejustice.mcp.relaxed_field = "chamber",
        librejustice.mcp.relaxed_value = chambers.join(","),
        librejustice.mcp.relaxed_slug = folded.join(","),
        "mcp: removed chamber folded onto the seat path"
    );
    if let Some(obj) = args.as_object_mut() {
        obj.remove("chamber");
        let mut seats = obj.get("seat").and_then(as_str_list).unwrap_or_default();
        seats.extend(folded);
        obj.insert("seat".to_string(), json!(seats));
    }
}

/// Trunks the call already names, the code preferred over the type since it
/// carries the court.
fn seat_trunks(args: &Value, refs: &crate::referential::Referential) -> Vec<String> {
    if let Some(codes) = args.get("jurisdiction_code").and_then(as_str_list) {
        return codes
            .iter()
            .filter_map(|code| {
                let kind = &refs.jurisdiction(code)?.jurisdiction_type;
                Some(lj_core::chamber::jurisdiction_trunk(kind, Some(code)).join("."))
            })
            .collect();
    }
    args.get("jurisdiction_type")
        .and_then(as_str_list)
        .unwrap_or_default()
        .iter()
        .filter(|kind| refs.jurisdiction_type_label(kind).is_some())
        .map(|kind| lj_core::chamber::jurisdiction_trunk(kind, None).join("."))
        .collect()
}

fn as_str_list(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// `ta_clermont-ferrand` for `ta_clermont_ferrand`, `CC` for `cc`, `CA.PARIS` for
/// `ca_paris`: the codes of the referential are all lowercase and carry
/// neither hyphen nor dot, so the normalisation can only
/// turn an invalid value into a valid one, never one court into another.
fn relax_jurisdiction_codes(codes: &mut [String], refs: &crate::referential::Referential) {
    for code in codes.iter_mut().filter(|c| refs.jurisdiction(c).is_none()) {
        let underscored = code.to_lowercase().replace(['-', '.'], "_");
        if refs.jurisdiction(&underscored).is_some() {
            tracing::warn!(
                librejustice.mcp.relaxed_field = "jurisdiction_code",
                librejustice.mcp.relaxed_value = code.as_str(),
                librejustice.mcp.relaxed_slug = underscored.as_str(),
                "mcp: juridiction code recovered from the underscore"
            );
            *code = underscored;
        }
    }
}

/// The hint carries the call shape on two real codes: the whole list runs to a
/// thousand entries and does not enumerate in a message.
fn unknown_jurisdiction_code(code: &str, refs: &crate::referential::Referential) -> RpcError {
    let examples: Vec<&str> = refs.jurisdictions().map(|(c, _)| c).take(2).collect();
    RpcError::invalid_params(crate::referential::unknown_jurisdiction_code_msg(
        code, refs,
    ))
    .with_hint(list_hint("jurisdiction_code", &examples))
}

/// Resolves an instrument value (slug, free name or acronym) to its catalogue
/// `(slug, text_uid)`.
async fn resolve_instrument(
    repo: &lj_store::repository::DecisionRepository<'_>,
    field: &str,
    value: &str,
    juris: Option<&str>,
) -> Result<(String, String), RpcError> {
    use lj_store::repository::InstrumentResolution as R;
    match repo
        .resolve_instrument(value, juris)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?
    {
        R::Direct { slug, text_uid, .. } => Ok((slug, text_uid)),
        R::Rattrape {
            slug,
            text_uid,
            regle: rule,
            ..
        } => {
            tracing::warn!(
                librejustice.mcp.relaxed_field = field,
                librejustice.mcp.relaxed_value = value,
                librejustice.mcp.relaxed_slug = slug.as_str(),
                librejustice.mcp.relaxed_rule = ?rule,
                "mcp: text name recovered from its catalogue slug"
            );
            Ok((slug, text_uid))
        }
        R::Inconnu { candidates } => Err(unknown_instrument(field, value, candidates)),
    }
}

/// Resolves `legal_instrument` / `legal_article` to the alphabet of the columns:
/// catalogue `text_uid`, article number in composite form.
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
            *v = resolve_instrument(&repo, "legal_instrument", v, None)
                .await?
                .1;
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
            let (slug, uid) =
                resolve_instrument(&repo, "legal_article", instrument.trim(), None).await?;
            *v = format!(
                "{uid}|{}",
                served_article_key(&repo, &uid, &slug, num).await?
            );
        }
    }
    Ok(())
}

/// The key the referential holds, `legi::served_version` recovering the same
/// spellings. An unresolved number filters on nothing, which reads as case law
/// being silent on the article rather than as a number naming none.
async fn served_article_key(
    repo: &lj_store::repository::DecisionRepository<'_>,
    uid: &str,
    slug: &str,
    num: &str,
) -> Result<String, RpcError> {
    let key = lj_norm::article_key::article_key(num);
    let mut tries = vec![key.clone()];
    tries.extend(lj_norm::article_key::letter_suffix_separated(&key));
    tries.extend(lj_norm::article_key::without_spoken_word(&key).map(str::to_string));
    tries.extend(lj_norm::article_key::sole_article_alias(&key).map(str::to_string));
    for essai in tries {
        let known = !repo
            .law_article_versions(uid, &essai)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?
            .is_empty();
        if known {
            return Ok(essai);
        }
    }
    Err(
        RpcError::not_found(format!("no article {num} in {slug}")).with_hint(format!(
            "Article numbers are not composable: read the index with get_norm \
             on {slug}, or take the number from a citation link of a decision."
        )),
    )
}

fn unknown_instrument_msg(
    field: &str,
    value: &str,
    candidates: &[lj_store::repository::InstrumentCandidate],
) -> String {
    let shown: Vec<String> = candidates
        .iter()
        .take(5)
        .map(|c| format!("\"{}\" ({})", c.slug, c.title))
        .collect();
    let did_you_mean = if shown.is_empty() {
        String::new()
    } else {
        format!("; did you mean: {}", shown.join(", "))
    };
    format!("unknown {field} '{value}'{did_you_mean}")
}

/// Search-filter side: two list fields, hence the call shape on top of the
/// candidates.
fn unknown_instrument(
    field: &str,
    value: &str,
    candidates: Vec<lj_store::repository::InstrumentCandidate>,
) -> RpcError {
    let msg = unknown_instrument_msg(field, value, &candidates);
    let slugs: Vec<String> = candidates.into_iter().take(5).map(|c| c.slug).collect();
    RpcError::invalid_params(msg).with_hint(format!(
        "Pass slugs (from the legal_instrument facet of a previous search) or \
         exact text names. {}",
        list_hint(field, &slugs)
    ))
}

/// Scalar `code` field: no list shape to give, only the alphabet of its values.
async fn resolve_law_code(
    state: &AppState,
    raw: &str,
    juris: Option<&str>,
) -> Result<String, RpcError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    resolve_instrument(&repo, "code", raw, juris)
        .await
        .map(|(slug, _)| slug)
        .map_err(|e| {
            RpcError::invalid_params(e.message).with_hint(
                "Pass a slug (from the legal_instrument facet of a previous search) \
                 or an exact text name.",
            )
        })
}

/// A text named with no article segment: the table of contents of a text with
/// articles, bodies attached below `INLINE_READING_MAX`.
async fn tool_get_legal_root(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let slug = resolve_law_code(state, code, juris).await?;
    let web = &state.settings.web_base_url;
    match crate::legi::law_root(state, &slug, juris)
        .await
        .map_err(map_legi_error)?
    {
        crate::legi::LawRoot::Body(summary) => tool_ok(&present_law_text(&summary, web)),
        crate::legi::LawRoot::Toc(summary, toc) => tool_ok(
            &crate::mcp_presenters::present_law_text_root(&summary, &toc, web),
        ),
    }
}

/// A section path (`…/livre-i/titre-v`): the division the table of contents
/// addresses. Without it, reading a chapter costs one call per article.
async fn tool_get_legal_section(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    path: &str,
    date: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let slug = resolve_law_code(state, code, juris).await?;
    let at = date.map(|d| parse_mcp_date("as_of", d)).transpose()?;
    let section = crate::legi::law_section(state, &slug, juris, path, at)
        .await
        .map_err(map_legi_error)?;
    tool_ok(&crate::mcp_presenters::present_law_section(
        &section,
        &state.settings.web_base_url,
    ))
}

async fn resolve_source_uid(state: &AppState, uid: &str) -> Result<(String, String), RpcError> {
    let mut targets = crate::legi::article_refs_by_source_uid(state, uid)
        .await
        .map_err(map_legi_error)?;
    match targets.len() {
        1 => Ok(targets.remove(0)),
        0 => Err(
            RpcError::not_found(format!("unknown Légifrance id '{uid}'")).with_hint(
                "Ids from another source than our LEGI import are unknown here — \
             search the article with search_norms, its hits carry the url.",
            ),
        ),
        n => Err(RpcError::invalid_params(format!(
            "Légifrance id '{uid}' designates {n} distinct articles"
        ))
        .with_hint("Pass the url of the one you mean (/norm/{jurisdiction}/{code}/{article}).")),
    }
}

async fn resolve_text_uid(state: &AppState, uid: &str) -> Result<(String, String), RpcError> {
    crate::legi::text_ref_by_uid(state, uid)
        .await
        .map_err(map_legi_error)?
        .ok_or_else(|| {
            RpcError::not_found(format!("unknown text id '{uid}'"))
                .with_hint("This text is not in our referential: search it with search_norms.")
        })
}

async fn resolve_section_cid(state: &AppState, cid: &str) -> Result<(String, String), RpcError> {
    let mut texts = crate::legi::section_texts_by_cid(state, cid)
        .await
        .map_err(map_legi_error)?;
    match texts.len() {
        1 => Ok(texts.remove(0)),
        0 => Err(
            RpcError::not_found(format!("unknown Légifrance id '{cid}'")).with_hint(
                "This section is not in our tables of contents: read the text by its url \
                 and follow its `section` entries.",
            ),
        ),
        n => Err(RpcError::invalid_params(format!(
            "Légifrance id '{cid}' is a section of {n} distinct texts"
        ))
        .with_hint("Pass the section url, /norm/{jurisdiction}/{code}/{section path}.")),
    }
}

/// Three accepted forms: the public url, absolute or relative
/// (`/norm/{juris}/{code}/{article}`), the legacy `/texte/` and `/loi/` prefixes, and the
/// bare path (`code-civil/1240`). The jurisdiction segment is optional: the
/// shape it replaces still resolves, and no text slug is two letters nor `intl`.
///
/// The bare path stays reserved to inputs with no scheme: on an absolute url the
/// prefix is required, otherwise the split would bite on the `//` of `https://`.
///
/// Third member `None`: the code is there, the article is missing.
fn split_article_url(url: &str) -> Option<(Option<&'static str>, &str, Option<&str>)> {
    let path = url
        .split("/norm/")
        .nth(1)
        .or_else(|| url.split("/texte/").nth(1))
        .or_else(|| url.split("/loi/").nth(1))
        .or_else(|| (!url.contains("://")).then(|| url.trim_start_matches('/')))?
        .trim_end_matches('/');
    let mut segs = path.splitn(2, '/');
    let premier = segs.next().filter(|s| !s.is_empty())?;
    let reste = segs.next().filter(|s| !s.is_empty());
    let Some(juris) = lj_norm::jurisdictions::from_url_segment(premier) else {
        return Some((None, premier, reste));
    };
    let mut segs = reste?.splitn(2, '/');
    let code = segs.next().filter(|s| !s.is_empty())?;
    Some((Some(juris), code, segs.next().filter(|s| !s.is_empty())))
}

fn is_iso_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn read_kind(tool: &str, args: &Value) -> Option<lj_dtos::ReadKind> {
    match tool {
        "get_decision" => Some(lj_dtos::ReadKind::Decision),
        "get_notice" => args
            .get("url")
            .and_then(Value::as_str)
            .and_then(notice_uid)
            .filter(|uid| uid.starts_with("marche:"))
            .map(|_| lj_dtos::ReadKind::Annonce),
        "get_norm" => match law_target(args).ok()? {
            LawTarget::Article { .. } | LawTarget::SourceUid(_) => Some(lj_dtos::ReadKind::Article),
            LawTarget::Section { .. } | LawTarget::SectionOf { .. } | LawTarget::SectionCid(_) => {
                Some(lj_dtos::ReadKind::Section)
            }
            LawTarget::Root { .. } | LawTarget::TextUid(_) => Some(lj_dtos::ReadKind::Text),
        },
        _ => None,
    }
}

/// Target designated by the `get_norm` arguments, whichever door was used.
#[derive(Debug, PartialEq, Eq)]
enum LawTarget {
    Article {
        juris: Option<String>,
        code: String,
        num: String,
    },
    /// Text named without an article, served whole when it carries none.
    Root {
        juris: Option<String>,
        code: String,
    },
    Section {
        juris: Option<String>,
        code: String,
        path: String,
    },
    /// The `…/section/{cid}` form of our own urls.
    SectionOf {
        juris: Option<String>,
        code: String,
        cid: String,
    },
    /// Légifrance ids, resolved against the database.
    SourceUid(String),
    TextUid(String),
    SectionCid(String),
}

/// The schema announces only the first of each line; the others are what callers
/// compose on their own.
const CODE_KEYS: [&str; 3] = ["code", "code_name", "codeName"];
const ARTICLE_KEYS: [&str; 4] = ["article", "article_number", "articleNumber", "num"];
const ID_KEYS: [&str; 3] = ["text_id", "textId", "legiarti"];
const AS_OF_KEYS: [&str; 2] = ["as_of", "date"];

fn arg_str<'a>(args: &'a Value, keys: &[&'static str]) -> Option<(&'static str, &'a str)> {
    keys.iter().find_map(|k| {
        args.get(*k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|v| (*k, v))
    })
}

fn law_target(args: &Value) -> Result<LawTarget, RpcError> {
    let url = arg_str(args, &["url"]);
    let id = arg_str(args, &ID_KEYS);
    // Our own section urls carry their LEGISCTA: the url parser reads them.
    let ours = |v: &str| v.contains("/norm/") || v.contains("/texte/") || v.contains("/loi/");
    if let Some((key, kind, id)) = [url, id].into_iter().flatten().find_map(|(k, v)| {
        lj_norm::text_identity::legifrance_id(v)
            .filter(|(kind, _)| {
                *kind == lj_norm::text_identity::IdKind::Article || k != "url" || !ours(v)
            })
            .map(|(kind, id)| (k, kind, id))
    }) {
        relaxed_key(key, "text_id");
        let id = id.to_string();
        return Ok(match kind {
            lj_norm::text_identity::IdKind::Article => LawTarget::SourceUid(id),
            lj_norm::text_identity::IdKind::Section => LawTarget::SectionCid(id),
            lj_norm::text_identity::IdKind::Text => LawTarget::TextUid(id),
        });
    }
    if let Some((key, uid)) = [url, id]
        .into_iter()
        .flatten()
        .find_map(|(k, v)| lj_norm::text_identity::bofip_uid(v).map(|uid| (k, uid)))
    {
        relaxed_key(key, "text_id");
        return Ok(match arg_str(args, &ARTICLE_KEYS) {
            Some((_, num)) => LawTarget::Article {
                juris: None,
                code: uid,
                num: num.to_string(),
            },
            None => LawTarget::TextUid(uid),
        });
    }
    if let Some((_, url)) = url {
        return match split_article_url(url) {
            Some((juris, code, Some(reste))) => {
                let juris = juris.map(str::to_string);
                let code = code.to_string();
                let reste = reste.split(['?', '#']).next().unwrap_or(reste);
                let segs: Vec<&str> = reste.split('/').filter(|s| !s.is_empty()).collect();
                Ok(match segs.as_slice() {
                    ["section", cid] => LawTarget::SectionOf {
                        juris,
                        code,
                        cid: (*cid).to_string(),
                    },
                    [num] | [num, "compare", ..] => LawTarget::Article {
                        juris,
                        code,
                        num: (*num).to_string(),
                    },
                    [num, date] if is_iso_date(date) => LawTarget::Article {
                        juris,
                        code,
                        num: (*num).to_string(),
                    },
                    _ => LawTarget::Section {
                        juris,
                        code,
                        path: segs.join("/"),
                    },
                })
            }
            Some((juris, code, None)) => Ok(LawTarget::Root {
                juris: juris.map(str::to_string),
                code: code.to_string(),
            }),
            None => Err(RpcError::invalid_params(format!(
                "not an article url (expected …/norm/{{jurisdiction}}/{{code}}/{{article}}): {url}"
            ))),
        };
    }
    let Some((code_key, code)) = arg_str(args, &CODE_KEYS) else {
        return Err(
            RpcError::invalid_params("missing tool argument: url").with_hint(
                "Address the article by url, /texte/{jurisdiction}/{code-slug}/{article-key}, \
             e.g. {\"url\": \"/norm/fr/code-civil/1240\"}, or by the pair {code, article}. \
             A Légifrance id (LEGIARTI, LEGITEXT, LEGISCTA…) or a BOFiP id (BOI-…) works too.",
            ),
        );
    };
    relaxed_key(code_key, "code");
    match arg_str(args, &ARTICLE_KEYS) {
        Some((article_key, num)) => {
            relaxed_key(article_key, "article");
            Ok(LawTarget::Article {
                juris: None,
                code: code.to_string(),
                num: num.to_string(),
            })
        }
        None => Ok(LawTarget::Root {
            juris: None,
            code: code.to_string(),
        }),
    }
}

/// The consultation date of `get_norm` and `search_norms`.
fn as_of(args: &Value) -> Result<Option<String>, RpcError> {
    for key in AS_OF_KEYS {
        if let Some(value) = deser_field::<String>(args, key)? {
            relaxed_key(key, "as_of");
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn relaxed_key(used: &'static str, canonical: &'static str) {
    if used != canonical {
        tracing::warn!(
            librejustice.mcp.relaxed_field = canonical,
            librejustice.mcp.relaxed_value = used,
            "mcp: norm argument recovered from its schema spelling"
        );
    }
}

fn query_too_long() -> String {
    format!(
        "query: String should have at most {} characters",
        lj_dtos::QUERY_MAX_CHARS
    )
}

fn required_str(args: &Value, key: &str) -> Result<String, RpcError> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        Some(_) => Err(RpcError::invalid_params(format!(
            "empty tool argument: {key}"
        ))),
        None => Err(RpcError::invalid_params(format!(
            "missing tool argument: {key}"
        ))),
    }
}

/// Keys handed back with an absent one. Twenty fit a hint and a caller walking a
/// numbering it invented, and the whole numbering of a code does not.
const NEAR_KEYS_MAX: i64 = 20;

/// Which of the two halves failed, named where it is known: a bare « law article
/// or code not found » sends the caller correcting the half that was right.
async fn unknown_code(state: &AppState, code: &str) -> RpcError {
    let Ok(conn) = state.pool.get().await else {
        return map_legi_error(ApiError::NotFound);
    };
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let candidates = repo.suggest_instruments(code, 5).await.unwrap_or_default();
    RpcError::not_found(unknown_instrument_msg("code", code, &candidates)).with_hint(
        "The code half of the call is what failed, not the article. Take a slug from \
         search_norms, or compose the url as /texte/{code-slug}/{article-key}.",
    )
}

fn no_such_article(titre: &str, num: &str) -> RpcError {
    RpcError::not_found(format!("\"{titre}\" holds no article \"{num}\"")).with_hint(
        "The code resolved, the article key did not. Read the index with get_norm \
         on the code alone, or take the key from a citation link of a decision.",
    )
}

async fn article_not_found(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
) -> RpcError {
    let Ok(s) = crate::legi::code_summary(state, code, juris).await else {
        return unknown_code(state, code).await;
    };
    if s.article_count == 0 && s.body.is_none() {
        return RpcError::not_found(format!(
            "\"{}\" is in the catalogue with no text: we hold its identity, not its articles",
            s.titre
        ))
        .with_hint(
            "Nothing to fix in the call, the article is nowhere to be read here. \
             Its official publisher carries the text.",
        );
    }
    let Ok((near, total)) =
        crate::legi::article_keys_near(state, &s.legitext, num, NEAR_KEYS_MAX).await
    else {
        return no_such_article(&s.titre, num);
    };
    if near.is_empty() {
        return no_such_article(&s.titre, num);
    }
    let keys: Vec<&str> = near.iter().map(|(k, _)| k.as_str()).collect();
    let texte = lj_vocab::norm_path(&s.jurisdiction, &s.code);
    RpcError::not_found(format!(
        "no article \"{num}\" in \"{}\", which holds {total} in force",
        s.titre
    ))
    .with_hint(format!(
        "Closest keys held: {}. Address one of them as {texte}/{{key}}, or \
         call get_norm on {texte} for the table of contents.",
        keys.join(", ")
    ))
}

fn map_legi_error(e: ApiError) -> RpcError {
    match e {
        ApiError::NotFound => RpcError::not_found("law article or code not found").with_hint(
            "Find the article with search_norms, or compose the url as \
             /texte/{code-slug}/{article-key} (lowercase key: \"l761-1\" for L. 761-1).",
        ),
        ApiError::NoVersionAtDate(msg) => RpcError::not_found(msg).with_hint(
            "The article exists: this is a date problem, not a spelling one. Ask again \
             within the covered span, or omit date to get the version in force.",
        ),
        other => RpcError::tool_error(other.to_string()),
    }
}

/// Largest window served, the one the three search schemas announce.
const SEARCH_LIMIT_MAX: i64 = 20;

/// A wrong type stays an error and an out-of-range window is clamped: a caller
/// refused asks it again unchanged.
fn clamp_search_limit(args: &Value) -> Result<i64, RpcError> {
    let asked: i64 = deser_field(args, "limit")?.unwrap_or(10);
    Ok(asked.clamp(1, SEARCH_LIMIT_MAX))
}

fn build_search_request(args: &Value) -> Result<lj_dtos::SearchRequest, RpcError> {
    // Server-side parity with the `additionalProperties: false` the schema
    // announces: an unknown filter silently ignored would skew the search with no
    // signal.
    const FIELDS: [&str; 17] = [
        "query",
        "jurisdiction_type",
        "solution",
        "legal_domain",
        "jurisdiction_code",
        "seat",
        "formation",
        "legal_instrument",
        "legal_article",
        "publication",
        "date_from",
        "date_to",
        "search_in",
        "mode",
        "sort",
        "limit",
        "ai_rerank",
    ];
    if let Some(obj) = args.as_object() {
        if let Some(unknown) = obj.keys().find(|k| {
            !FIELDS.contains(&k.as_str()) && !crate::search::RETIRED_FILTERS.contains(&k.as_str())
        }) {
            let refus = RpcError::invalid_params(format!(
                "unknown field '{unknown}'; valid fields: {}",
                FIELDS.join(", ")
            ));
            return Err(match moved_field_hint(unknown) {
                Some(hint) => refus.with_hint(hint),
                None => refus,
            });
        }
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("missing tool argument: query"))?
        .to_string();
    // Parity with the announced schema (`query.maxLength`) and the HTTP path.
    if query.chars().count() > lj_dtos::QUERY_MAX_CHARS {
        return Err(RpcError::invalid_params(query_too_long()));
    }

    // Format and range validated here, otherwise a malformed date reaches the SQL.
    let date_from: Option<String> = deser_field(args, "date_from")?;
    let date_to: Option<String> = deser_field(args, "date_to")?;
    validate_mcp_date("date_from", &date_from)?;
    validate_mcp_date("date_to", &date_to)?;
    let limit = clamp_search_limit(args)?;

    Ok(lj_dtos::SearchRequest {
        query,
        jurisdiction_type: deser_list(args, "jurisdiction_type")?,
        solution: deser_list(args, "solution")?,
        legal_domain: deser_list(args, "legal_domain")?,
        jurisdiction_code: deser_list(args, "jurisdiction_code")?,
        seat: deser_list(args, "seat")?,
        home: None,
        formation: deser_list(args, "formation")?,
        legal_instrument: deser_list(args, "legal_instrument")?,
        legal_article: deser_list(args, "legal_article")?,
        publication: deser_list(args, "publication")?,
        date_from,
        date_to,
        search_in: deser_list(args, "search_in")?,
        mode: deser_field(args, "mode")?.unwrap_or(lj_dtos::SearchMode::Auto),
        sort: deser_field(args, "sort")?.unwrap_or(lj_dtos::SortOrder::Relevance),
        limit: limit as u32,
        offset: 0,
        ai_mode: deser_field(args, "ai_rerank")?.unwrap_or(true),
        lang: None,
    })
}

fn validate_mcp_date(field: &str, raw: &Option<String>) -> Result<(), RpcError> {
    match raw {
        Some(s) => parse_mcp_date(field, s).map(|_| ()),
        None => Ok(()),
    }
}

fn parse_mcp_date(field: &str, s: &str) -> Result<chrono::NaiveDate, RpcError> {
    crate::search::parse_search_date(s).map_err(|e| match e {
        crate::search::DateError::Parse(m) => {
            RpcError::invalid_params(format!("invalid {field}: {m}")).with_hint(format!(
                "{field} is a calendar date in strict YYYY-MM-DD form, \
                 e.g. \"2024-03-15\" — zero-padded, no time part."
            ))
        }
        crate::search::DateError::TooEarly => {
            RpcError::invalid_params(format!("{field} must be >= {}", crate::search::DATE_GE))
        }
        crate::search::DateError::TooLate => {
            RpcError::invalid_params(format!("{field} must be <= {}", crate::search::DATE_LE))
        }
    })
}

/// Where the intention of a field callers keep writing lives now. Without it the
/// refusal only lists the valid names, and the caller widens the search rather
/// than moves the filter.
fn moved_field_hint(field: &str) -> Option<&'static str> {
    match field {
        "chamber" | "jurisdiction_name" => Some(
            "A court or a room is a path in `seat`, e.g. \
             {\"seat\": [\"FR.CA.PARIS\", \"FR.CC.COMMERCIALE\", \"FR.CA.DOUAI.C7\"]}, \
             taken from facets.seat.",
        ),
        "docketNumber" => Some(
            "A docket number is searched as text: put it in `query`, quoted, \
             e.g. {\"query\": \"\\\"17/00302\\\"\"}.",
        ),
        "significance" => Some(
            "Publication standing is `publication`, \
             e.g. {\"publication\": [\"PUBLIE_BULLETIN\", \"PUBLIE_LEBON\"]}.",
        ),
        "max_results" => Some("The size of the shortlist is `limit`."),
        "dateEnd" => Some("The date window is `date_from` and `date_to`, in ISO form."),
        _ => None,
    }
}

/// One hint for every list field, whatever the cause of the rejection. It says
/// two things no value message says: that several values combine at once, where
/// "expected one of" reads as "only one", and the shape of the call. `examples`
/// are real values of the field, that the error site has just computed, so
/// always valid.
fn list_hint<S: AsRef<str>>(key: &str, examples: &[S]) -> String {
    let example = if examples.is_empty() {
        "\"VALUE\", \"OTHER\"".to_string()
    } else {
        examples
            .iter()
            .take(2)
            .map(|v| format!("\"{}\"", v.as_ref()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{key} takes a list, not a single value — e.g. \
         {{\"{key}\": [{example}]}}. Any number of valid entries combine; \
         one entry per value, never a comma-joined string."
    )
}

/// Valid values as enumerated by a serde message ("unknown variant
/// `judiciaire`, expected one of `TA`, `CAA`, …"), the source of examples of
/// [`list_hint`] for the typed enums of the DTO.
fn serde_variants(err: &str) -> Vec<&str> {
    err.split_once("expected one of ")
        .map(|(_, list)| list.split('`').skip(1).step_by(2).collect())
        .unwrap_or_default()
}

/// A scalar where the schema announces an array is wrapped into a singleton,
/// which separates two cases: good value, the singleton is taken and a WARN is
/// emitted; bad value, the error returned is the one after wrapping, which
/// enumerates the valid variants.
fn deser_list<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<Vec<T>>, RpcError> {
    let Some(value) = args.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let coerced = match value {
        Value::Array(_) => None,
        // A client that serialises its list before handing it over sends
        // `"[\"CE\",\"CAA\"]"`.
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_array)
            .or_else(|| Some(json!([value]))),
        _ => Some(json!([value])),
    };
    match serde_json::from_value(coerced.clone().unwrap_or_else(|| value.clone())) {
        Ok(parsed) => {
            if coerced.is_some() {
                tracing::warn!(
                    librejustice.mcp.coerced_arg = key,
                    librejustice.mcp.coerced_value = %value,
                    "mcp: scalar accepted as an array"
                );
            }
            Ok(Some(parsed))
        }
        Err(e) => {
            let msg = e.to_string();
            Err(
                RpcError::invalid_params(format!("invalid argument {key}: {msg}"))
                    .with_hint(list_hint(key, &serde_variants(&msg))),
            )
        }
    }
}

/// The message names the field: the bare serde message names none.
fn deser_field<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<T>, RpcError> {
    let Some(value) = args.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|e| RpcError::invalid_params(format!("invalid argument {key}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_exposes_all_tools() {
        let tools = lj_mcp_tools::tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "search_decisions",
                "get_decision",
                "get_norm",
                "search_norms",
                "search_notices",
                "get_notice",
            ]
        );
    }

    #[test]
    fn the_journal_officiel_answers_a_question_and_procurement_filters_read_procurement() {
        let funds = |fund: Option<Value>, question, procurement| {
            notice_funds(
                fund.as_ref().and_then(Value::as_array),
                question,
                procurement,
            )
        };
        assert_eq!(
            funds(None, false, false).unwrap(),
            ["marche", "bodacc", "joafe"]
        );
        assert_eq!(
            funds(None, true, false).unwrap(),
            ["marche", "bodacc", "joafe", "jo"]
        );
        assert_eq!(funds(None, true, true).unwrap(), ["marche"]);
        assert_eq!(funds(Some(json!(["JO"])), false, false).unwrap(), ["jo"]);
        assert!(funds(Some(json!(["raa"])), true, false).is_err());
    }

    #[test]
    fn a_notice_url_names_its_uid_slashes_included() {
        assert_eq!(
            notice_uid("https://librejustice.fr/notice/dj/jo-dj:DJ/ACCORD-0"),
            Some("jo-dj:DJ/ACCORD-0")
        );
        assert_eq!(
            notice_uid("/notice/fr/bodacc:A20260001?from=search#top"),
            Some("bodacc:A20260001")
        );
        assert_eq!(notice_uid("https://librejustice.fr/notice/fr/"), None);
        assert_eq!(
            notice_uid("https://librejustice.fr/norm/fr/code-civil"),
            None
        );
    }

    #[test]
    fn build_request_maps_referential_filters_and_ai_rerank() {
        let args = json!({
            "query": "bail commercial",
            "publication": ["PUBLIE_BULLETIN"],
            "jurisdiction_type": ["CC", "CA"],
            "solution": ["DISMISSED", "GRANTED"],
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
            Some(vec![
                lj_dtos::Solution::Dismissed,
                lj_dtos::Solution::Granted
            ])
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
    fn build_request_coerces_scalar_to_singleton_when_value_is_valid() {
        let req = build_search_request(&json!({
            "query": "bail",
            "legal_domain": "CIVIL_DROIT_LOCATIF",
            "jurisdiction_type": "CA",
        }))
        .unwrap();
        assert_eq!(
            req.legal_domain,
            Some(vec![lj_dtos::Domain::CivilDroitLocatif])
        );
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![lj_dtos::JurisdictionType::Ca])
        );
    }

    /// A list serialised into a string reads as the list.
    #[test]
    fn build_request_reads_a_list_serialised_into_a_string() {
        let req = build_search_request(&json!({
            "query": "impôt",
            "jurisdiction_type": "[\"CE\",\"CAA\",\"TA\"]",
        }))
        .unwrap();
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![
                lj_dtos::JurisdictionType::Ce,
                lj_dtos::JurisdictionType::Caa,
                lj_dtos::JurisdictionType::Ta,
            ])
        );
    }

    #[test]
    fn out_of_enum_value_reports_variants_and_list_shape() {
        for bad in [json!("judiciaire"), json!(["judiciaire"])] {
            let err = build_search_request(&json!({"query": "bail", "jurisdiction_type": bad}))
                .unwrap_err();
            assert!(err.message.contains("jurisdiction_type"));
            assert!(err.message.contains("unknown variant"));
            assert!(err.message.contains("CA"), "{}", err.message);
            let hint = err.hint.expect("hint on list field");
            assert!(hint.contains("takes a list, not a single value"), "{hint}");
            assert!(
                hint.contains("{\"jurisdiction_type\": [\"TA\", \"CAA\"]}"),
                "{hint}"
            );
            assert!(
                hint.contains("Any number of valid entries combine"),
                "{hint}"
            );
        }

        let err = build_search_request(&json!({
            "query": "bail",
            "solution": "ANNULATION,REJET",
        }))
        .unwrap_err();
        assert!(err.message.contains("unknown variant"));
        assert!(err.message.contains("REJET"), "{}", err.message);
        assert!(err.hint.is_some_and(|h| h.contains("comma-joined")));
    }

    #[test]
    fn build_request_rejects_empty_query() {
        assert!(build_search_request(&json!({"query": ""})).is_err());
        assert!(build_search_request(&json!({})).is_err());
    }

    #[test]
    fn out_of_range_limit_is_clamped() {
        for (asked, served) in [(100, 20), (50, 20), (30, 20), (0, 1), (-5, 1)] {
            let limit = clamp_search_limit(&json!({"limit": asked})).unwrap();
            assert_eq!(limit, served, "limit={asked}");
        }
        for asked in [1, 10, 20] {
            assert_eq!(clamp_search_limit(&json!({"limit": asked})).unwrap(), asked);
        }
        assert_eq!(clamp_search_limit(&json!({})).unwrap(), 10);
        assert!(clamp_search_limit(&json!({"limit": "20"})).is_err());
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
                place: None,
            }],
            Vec::new(),
        );
        let err = unknown_jurisdiction_code("tj75056", &refs);
        assert!(err.message.contains("tj_paris"));
        assert!(err.message.contains("Tribunal judiciaire de Paris"));
        let err = unknown_jurisdiction_code("tribunal_paris", &refs);
        assert!(err.message.contains("tj_paris"));
        let err = unknown_jurisdiction_code("tj_parsi", &refs);
        assert!(err.message.contains("tj_paris"));
    }

    #[test]
    fn relax_jurisdiction_codes_ne_reecrit_que_ce_qui_devient_connu() {
        let refs = crate::referential::Referential::new(
            vec![],
            vec![lj_store::repository::JurisdictionRow {
                code: "ca_aix_en_provence".into(),
                source_code: "ca13".into(),
                jurisdiction_type: "CA".into(),
                city: Some("Aix-en-Provence".into()),
                label: "Cour d'appel d'Aix-en-Provence".into(),
                place: None,
            }],
            Vec::new(),
        );
        let mut codes = [
            "ca_aix-en-provence".to_string(),
            "ca_aix_en_provence".to_string(),
            "ca-nulle-part".to_string(),
            "CA_AIX_EN_PROVENCE".to_string(),
            "CA.AIX_EN_PROVENCE".to_string(),
        ];
        relax_jurisdiction_codes(&mut codes, &refs);
        assert_eq!(codes[0], "ca_aix_en_provence");
        assert_eq!(codes[1], "ca_aix_en_provence");
        assert_eq!(codes[2], "ca-nulle-part");
        assert_eq!(codes[3], "ca_aix_en_provence");
        assert_eq!(codes[4], "ca_aix_en_provence");
    }

    /// Every address shape of an article resolves, the Légifrance id included.
    #[test]
    fn law_target_reads_every_address_shape() {
        let article = |juris: Option<&str>, code: &str, num: &str| LawTarget::Article {
            juris: juris.map(str::to_string),
            code: code.into(),
            num: num.into(),
        };
        assert_eq!(
            law_target(&json!({"url": "/norm/fr/code-civil/1240"})).unwrap(),
            article(Some("FR"), "code-civil", "1240")
        );
        // L'ancienne forme, sans segment, résout encore.
        assert_eq!(
            law_target(&json!({"url": "/texte/code-civil/1240"})).unwrap(),
            article(None, "code-civil", "1240")
        );
        assert_eq!(
            law_target(
                &json!({"code_name": "code-de-procedure-penale", "article_number": "706-88"})
            )
            .unwrap(),
            article(None, "code-de-procedure-penale", "706-88")
        );
        assert_eq!(
            law_target(&json!({"code": "code-civil", "article": "1240"})).unwrap(),
            article(None, "code-civil", "1240")
        );
        assert_eq!(
            law_target(&json!({"textId": "LEGIARTI000048784916"})).unwrap(),
            LawTarget::SourceUid("LEGIARTI000048784916".into())
        );
        assert_eq!(
            law_target(&json!({"url": "LEGIARTI000048784916"})).unwrap(),
            LawTarget::SourceUid("LEGIARTI000048784916".into())
        );
        // A Légifrance permalink resolves to its article id.
        assert_eq!(
            law_target(&json!({
                "url": "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000052014018"
            }))
            .unwrap(),
            LawTarget::SourceUid("LEGIARTI000052014018".into())
        );
        // The digit run bounds the id: neither a dated suffix nor a query enters it.
        assert_eq!(
            law_target(&json!({
                "url": "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419282/2024-01-01?x=1"
            }))
            .unwrap(),
            LawTarget::SourceUid("LEGIARTI000006419282".into())
        );
        assert_eq!(
            law_target(&json!({"textId": "LEGITEXT000006070721"})).unwrap(),
            LawTarget::TextUid("LEGITEXT000006070721".into())
        );
        assert_eq!(
            law_target(&json!({"textId": "KALIARTI000044253023"})).unwrap(),
            LawTarget::SourceUid("KALIARTI000044253023".into())
        );
        assert_eq!(
            law_target(&json!({"textId": "BOI-RSA-CHAMP-20-40-10-20"})).unwrap(),
            LawTarget::TextUid("BOI-RSA-CHAMP-20-40-10-20".into())
        );
        assert_eq!(
            law_target(&json!({"textId": "BOI-BIC-PROV-50", "article": "130"})).unwrap(),
            article(None, "BOI-BIC-PROV-50", "130")
        );
        assert_eq!(
            law_target(&json!({
                "url": "https://bofip.impots.gouv.fr/bofip/5123-PGP.html/identifiant=BOI-RSA-CHAMP-20-40-10-20-20250429"
            }))
            .unwrap(),
            LawTarget::TextUid("BOI-RSA-CHAMP-20-40-10-20".into())
        );
        // A permalink names the text first: the article it carries wins.
        assert_eq!(
            law_target(&json!({
                "url": "https://www.legifrance.gouv.fr/codes/id/LEGITEXT000033555730/LEGIARTI000033555747/"
            }))
            .unwrap(),
            LawTarget::SourceUid("LEGIARTI000033555747".into())
        );
        assert_eq!(
            law_target(&json!({
                "url": "https://www.legifrance.gouv.fr/codes/section_lc/LEGITEXT000006070721/LEGISCTA000006136059/"
            }))
            .unwrap(),
            LawTarget::SectionCid("LEGISCTA000006136059".into())
        );
        assert_eq!(
            law_target(&json!({"code": "circulaire-x", "query": "séjour"})).unwrap(),
            LawTarget::Root {
                juris: None,
                code: "circulaire-x".into()
            }
        );
        assert_eq!(
            law_target(&json!({"url": "https://librejustice.fr/norm/fr/code-du-travail"})).unwrap(),
            LawTarget::Root {
                juris: Some("FR".into()),
                code: "code-du-travail".into()
            }
        );
        assert_eq!(
            law_target(&json!({
                "url": "https://librejustice.fr/norm/fr/code-du-travail/section/LEGISCTA000006195791"
            }))
            .unwrap(),
            LawTarget::SectionOf {
                juris: Some("FR".into()),
                code: "code-du-travail".into(),
                cid: "LEGISCTA000006195791".into()
            }
        );
        assert_eq!(
            law_target(&json!({"url": "https://librejustice.fr/norm/fr/code-civil/livre-i/titre-v?date=2020-01-01"}))
                .unwrap(),
            LawTarget::Section {
                juris: Some("FR".into()),
                code: "code-civil".into(),
                path: "livre-i/titre-v".into()
            }
        );
        assert_eq!(
            law_target(&json!({"url": "/norm/fr/code-civil/1240/compare/2016-10-01/initiale"}))
                .unwrap(),
            article(Some("FR"), "code-civil", "1240")
        );
        assert_eq!(
            law_target(&json!({"url": "/norm/fr/code-civil/1240/2016-09-05"})).unwrap(),
            article(Some("FR"), "code-civil", "1240")
        );
        let err = law_target(&json!({"date": "2026-08-08"})).unwrap_err();
        assert!(err.message.contains("missing tool argument: url"));
        assert!(err.hint.is_some_and(|h| h.contains("{code, article}")));
        assert_eq!(lj_norm::text_identity::legifrance_id("LEGIARTI"), None);
        assert_eq!(
            lj_norm::text_identity::legifrance_id("LEGIARTI000006419282"),
            Some((
                lj_norm::text_identity::IdKind::Article,
                "LEGIARTI000006419282"
            ))
        );
    }

    #[test]
    fn build_request_rejects_unknown_fields() {
        let err = build_search_request(&json!({
            "query": "bail",
            "jurisdiction_name": ["ca_paris"],
        }))
        .unwrap_err();
        assert!(err.message.contains("jurisdiction_name"));

        let sans_remplacant = build_search_request(&json!({"query": "bail", "couleur": "x"}))
            .unwrap_err()
            .hint;
        assert!(sans_remplacant.is_none(), "{sans_remplacant:?}");

        for (field, replacement) in [
            ("chamber", "seat"),
            ("jurisdiction_name", "seat"),
            ("docketNumber", "query"),
            ("significance", "publication"),
            ("max_results", "limit"),
            ("dateEnd", "date_to"),
        ] {
            let err = build_search_request(&json!({"query": "bail", field: "x"})).unwrap_err();
            let hint = err.hint.unwrap_or_else(|| panic!("hint sur {field}"));
            assert!(hint.contains(replacement), "{field} : {hint}");
        }
    }

    /// A retired `chamber` folds into its seat; with no seat node or no court it stays.
    #[test]
    fn retired_chamber_folds_into_seat_or_does_not_fold() {
        use lj_store::repository::{FacetValueRow, JurisdictionRow};
        let refs = crate::referential::Referential::new(
            vec![FacetValueRow {
                uid: "jurisdiction_type:CC".to_string(),
                facet: "jurisdiction_type".to_string(),
                label: "Cour de cassation".to_string(),
                abbr: None,
                parent_uid: None,
                sort: 1,
                hint: None,
            }],
            vec![
                JurisdictionRow {
                    code: "cc".to_string(),
                    source_code: "CASS".to_string(),
                    jurisdiction_type: "CC".to_string(),
                    city: None,
                    label: "Cour de cassation".to_string(),
                    place: None,
                },
                JurisdictionRow {
                    code: "ca_douai".to_string(),
                    source_code: "DOUAI".to_string(),
                    jurisdiction_type: "CA".to_string(),
                    city: Some("Douai".to_string()),
                    label: "Cour d'appel de Douai".to_string(),
                    place: None,
                },
            ],
            vec![
                "FR.CC.SOCIALE".to_string(),
                "FR.CC.COMMERCIALE".to_string(),
                "FR.CA.DOUAI.C7".to_string(),
            ],
        );
        let folded = |mut args: Value| {
            fold_retired_chamber(&mut args, &refs);
            args
        };

        assert_eq!(
            folded(json!({"query": "q", "jurisdiction_type": ["CC"], "chamber": ["SOCIALE"]})),
            json!({"query": "q", "jurisdiction_type": ["CC"], "seat": ["FR.CC.SOCIALE"]})
        );
        assert_eq!(
            folded(json!({"query": "q", "jurisdiction_code": ["cc"], "chamber": ["COMMERCIALE"]})),
            json!({"query": "q", "jurisdiction_code": ["cc"], "seat": ["FR.CC.COMMERCIALE"]})
        );
        assert_eq!(
            folded(json!({"query": "q", "jurisdiction_code": ["ca_douai"], "chamber": ["7"]})),
            json!({"query": "q", "jurisdiction_code": ["ca_douai"], "seat": ["FR.CA.DOUAI.C7"]})
        );

        let kept = json!({"query": "q", "jurisdiction_type": ["CC"], "chamber": ["COMMERCIALE", "CIVILE"]});
        assert_eq!(folded(kept.clone()), kept);
        assert!(build_search_request(&kept)
            .unwrap_err()
            .message
            .contains("chamber"));

        let no_court = json!({"query": "q", "chamber": ["SOCIALE"]});
        assert_eq!(folded(no_court.clone()), no_court);

        let unknown_type =
            json!({"query": "q", "jurisdiction_type": ["XX"], "chamber": ["SOCIALE"]});
        assert_eq!(folded(unknown_type.clone()), unknown_type);
    }

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
    }

    /// A root combinator in an `inputSchema` makes Claude Code drop the tool, with
    /// no error and no trace.
    #[test]
    fn input_schemas_carry_no_root_combinator() {
        for tool in lj_mcp_tools::tool_definitions() {
            for combinateur in ["anyOf", "oneOf", "allOf", "not"] {
                assert!(
                    tool["inputSchema"].get(combinateur).is_none(),
                    "{}: {combinateur} à la racine de l'inputSchema",
                    tool["name"]
                );
            }
        }
    }

    /// The offending field is named, and a scalar field does not inherit the
    /// "takes a list" hint.
    #[test]
    fn scalar_field_error_names_the_field_without_the_array_hint() {
        let err = build_search_request(&json!({"query": "x", "mode": "lexicale"})).unwrap_err();
        assert!(err.message.contains("mode"), "{}", err.message);
        assert!(err.hint.is_none(), "{:?}", err.hint);

        assert!(build_search_request(&json!({"query": "x", "mode": "lexical"})).is_ok());
        assert!(build_search_request(&json!({
            "query": "x",
            "solution": ["GRANTED", "DISMISSED"],
        }))
        .is_ok());
    }

    /// Same for dates: the expected shape when the format failed, nothing more
    /// when the value is out of bounds, the message already carrying the bound.
    #[test]
    fn date_error_gives_the_expected_shape_only_when_the_format_failed() {
        let err = parse_mcp_date("date", "15/03/2024").unwrap_err();
        assert!(err.message.contains("invalid date"), "{}", err.message);
        let hint = err.hint.expect("hint on malformed date");
        assert!(hint.contains("YYYY-MM-DD"), "{hint}");

        assert!(parse_mcp_date("date", "2024-1-1")
            .unwrap_err()
            .hint
            .is_some());

        let too_early = parse_mcp_date("date_from", "1600-01-01").unwrap_err();
        assert!(
            too_early.message.contains("1678-01-01"),
            "{}",
            too_early.message
        );
        assert!(too_early.hint.is_none());

        assert!(parse_mcp_date("date", "2024-03-15").is_ok());
    }

    #[test]
    fn article_url_accepts_absolute_relative_legacy_and_bare_path() {
        use super::split_article_url as split;
        let civ = Some((None, "code-civil", Some("1240")));
        assert_eq!(split("https://librejustice.fr/texte/code-civil/1240"), civ);
        assert_eq!(split("/texte/code-civil/1240"), civ);
        assert_eq!(split("/loi/code-civil/1240"), civ);
        assert_eq!(split("code-civil/1240"), civ);
        assert_eq!(split("code-civil/1240/"), civ);

        // The jurisdiction segment, on the same doors.
        let fr = Some((Some("FR"), "code-civil", Some("1240")));
        assert_eq!(split("https://librejustice.fr/norm/fr/code-civil/1240"), fr);
        assert_eq!(split("/norm/fr/code-civil/1240"), fr);
        assert_eq!(split("/texte/fr/code-civil/1240"), fr);
        assert_eq!(
            split("/norm/be/nouveau-code-civil"),
            Some((Some("BE"), "nouveau-code-civil", None))
        );

        assert_eq!(
            split("/texte/code-du-travail"),
            Some((None, "code-du-travail", None))
        );
        assert_eq!(
            split("code-du-travail"),
            Some((None, "code-du-travail", None))
        );

        assert_eq!(split("https://librejustice.fr/decision/fr/abc"), None);
        assert_eq!(split("/norm/"), None);
        assert_eq!(split(""), None);
        // A jurisdiction segment on its own names no text.
        assert_eq!(split("/norm/fr"), None);
    }

    #[test]
    fn initialize_carries_protocol_and_instructions() {
        let res = initialize_result(&json!({"protocolVersion": "2025-06-18"}));
        assert_eq!(res["protocolVersion"], json!("2025-06-18"));
        let newest = initialize_result(&json!({"protocolVersion": "1.0.0"}));
        assert_eq!(
            newest["protocolVersion"],
            json!(lj_mcp_tools::PROTOCOL_VERSIONS[0])
        );
        assert_eq!(
            initialize_result(&json!(null))["protocolVersion"],
            newest["protocolVersion"]
        );
        assert_eq!(res["serverInfo"]["name"], json!(SERVER_NAME));
        assert!(res["instructions"]
            .as_str()
            .unwrap()
            .contains("European, Swiss and Belgian case law"));
    }
}
