//! Endpoints OAuth 2.1 (Authorization Code + PKCE S256 + DCR) pour l'accès MCP.
//!
//! Port de `apps/api/.../oauth.py`. Spec MCP « Authorization » (2025-03-26),
//! requise par Claude.ai Custom Connectors et ChatGPT Connectors :
//!
//! - Authorization Code + PKCE S256 (RFC 6749 / 7636)
//! - Dynamic Client Registration (RFC 7591) — `POST /oauth/register`
//! - Authorization Server Metadata (RFC 8414) — `/.well-known/oauth-authorization-server`
//! - Protected Resource Metadata (RFC 9728) — `/.well-known/oauth-protected-resource`
//!
//! Clients publics uniquement : pas de `client_secret`, la sécurité repose sur
//! PKCE. Les helpers crypto (base64url, SHA-256, tokens aléatoires) sont
//! implémentés localement pour rester sur les deps déclarées du crate.

use crate::auth::RequiredUser;
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ── DTOs (parité avec les modèles Pydantic d'`oauth.py`) ────────────────────

/// RFC 7591 §2 — métadonnées client envoyées par Claude.ai / ChatGPT.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    // Acceptés mais non contraignants (le flow impose sa propre combinaison).
    #[serde(default)]
    pub grant_types: Option<Vec<String>>,
    #[serde(default)]
    pub response_types: Option<Vec<String>>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub client_id: String,
    pub client_id_issued_at: i64,
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
}

#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    // Paramètres requis côté FastAPI (`Query(...)` sans défaut) : un manquant →
    // 422 de validation, AVANT les contrôles métier 400. On les capture en
    // `Option` pour rendre ce 422 explicite plutôt que le rejet 400 de
    // l'extracteur `Query`.
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    #[serde(default = "default_s256")]
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: Option<String>,
}

fn default_s256() -> String {
    "S256".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub client_id: String,
    pub code_challenge: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenForm {
    // Champs `Form()` requis côté FastAPI : un manquant → 422 de validation
    // (`{type:"missing", loc:["body", <champ>]}`), pas le rejet 422 par défaut
    // de l'extracteur `Form`. On les capture en `Option` pour reproduire ce
    // corps Pydantic à la main, dans l'ordre de déclaration FastAPI.
    #[serde(default)]
    pub grant_type: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_verifier: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    // Grant `refresh_token` (RFC 6749 §6) : seul champ requis en plus de
    // `grant_type` / `client_id` pour ce flow.
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub refresh_token: String,
}

// ── Routeur OAuth (préfixe `/oauth`) ────────────────────────────────────────

/// Routes `/oauth/{register,authorize,approve,token}` (parité avec le
/// `APIRouter(prefix="/oauth")` Python).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/approve", post(approve))
        .route("/oauth/token", post(token))
}

/// Routes de discovery `/.well-known/*` (RFC 8414 + RFC 9728 + MCP 2025-03-26).
///
/// Servies à la racine (cf. `main.py`) car publiées dans la discovery /
/// consommées par les clients externes. Les deux chemins
/// `oauth-protected-resource` servent le même document (RFC 9728 + variante
/// avec composant de path exigée par Claude.ai).
pub fn well_known_router() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_as_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth_protected_resource_metadata),
        )
        .route(
            "/.well-known/openai-apps-challenge",
            get(openai_apps_challenge),
        )
        .route("/.well-known/glama.json", get(glama_manifest))
}

/// Sert le token de vérification de propriété du domaine pour le catalogue d'apps
/// ChatGPT en `text/plain`. Lu depuis les `Settings`
/// (`LIBREJUSTICE_API_OPENAI_APPS_CHALLENGE_TOKEN`) ; non configuré → 404.
async fn openai_apps_challenge(State(state): State<AppState>) -> Response {
    match state.settings.openai_apps_challenge_token.as_deref() {
        Some(token) => token.to_owned().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Manifeste de claim du connecteur pour l'annuaire Glama (glama.ai/mcp) :
/// `maintainers` doit matcher le compte Glama du mainteneur. Lu depuis les
/// `Settings` (`LIBREJUSTICE_API_GLAMA_MAINTAINER`) ; non configuré → 404.
async fn glama_manifest(State(state): State<AppState>) -> Response {
    match state.settings.glama_maintainer.as_deref() {
        Some(maintainer) => Json(serde_json::json!({
            "$schema": "https://glama.ai/mcp/schemas/server.json",
            "maintainers": [maintainer],
        }))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// RFC 7591 — Dynamic Client Registration (public client only, PKCE-protected).
async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Response> {
    // Contrainte Pydantic `redirect_uris: Field(min_length=1, max_length=10)` →
    // 422 (et non 400) côté FastAPI quand la liste est vide ou > 10.
    if body.redirect_uris.is_empty() {
        return Err(ApiError::Unprocessable(validation::too_short(
            &["body", "redirect_uris"],
            json!(body.redirect_uris),
            "List",
            1,
        )));
    }
    if body.redirect_uris.len() > 10 {
        return Err(ApiError::Unprocessable(validation::too_long(
            &["body", "redirect_uris"],
            json!(body.redirect_uris),
            "List",
            10,
        )));
    }
    for uri in &body.redirect_uris {
        if !(uri.starts_with("https://")
            || uri.starts_with("http://localhost")
            || uri.starts_with("http://127.0.0.1"))
        {
            return Err(ApiError::BadRequest("invalid_redirect_uri".into()));
        }
    }

    let client_id = format!("lj_{}", token_urlsafe(16));
    let conn = pool_conn(&state).await?;
    conn.execute(
        "INSERT INTO mcp_clients (client_id, name, redirect_uris) VALUES ($1, $2, $3)",
        &[&client_id, &body.client_name, &body.redirect_uris],
    )
    .await
    .map_err(store_err)?;

    let resp = RegisterResponse {
        client_id,
        client_id_issued_at: Utc::now().timestamp(),
        redirect_uris: body.redirect_uris,
        client_name: body.client_name,
        grant_types: vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ],
        response_types: vec!["code".to_string()],
        token_endpoint_auth_method: "none".to_string(),
    };
    Ok((StatusCode::CREATED, Json(resp)).into_response())
}

/// RFC 6749 — Redirige vers la page d'approbation frontend après validations.
async fn authorize(
    State(state): State<AppState>,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Response> {
    // Paramètres requis manquants → 422 (validation FastAPI), avant tout 400
    // métier. Ordre de déclaration FastAPI : response_type, client_id,
    // redirect_uri, code_challenge (un seul rapporté à la fois côté `Query`).
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["query", f]));
    let response_type = q.response_type.ok_or_else(|| missing("response_type"))?;
    let client_id = q.client_id.ok_or_else(|| missing("client_id"))?;
    let redirect_uri = q.redirect_uri.ok_or_else(|| missing("redirect_uri"))?;
    let code_challenge = q.code_challenge.ok_or_else(|| missing("code_challenge"))?;

    if response_type != "code" {
        return Err(ApiError::BadRequest("unsupported_response_type".into()));
    }
    if q.code_challenge_method != "S256" {
        return Err(ApiError::BadRequest(
            "unsupported_code_challenge_method".into(),
        ));
    }
    if !redirect_uri_allowed(&state, &client_id, &redirect_uri).await? {
        return Err(ApiError::BadRequest(
            "invalid_client_or_redirect_uri".into(),
        ));
    }

    let mut qs = form_urlencode(&[
        ("client_id", &client_id),
        ("redirect_uri", &redirect_uri),
        ("code_challenge", &code_challenge),
        ("code_challenge_method", &q.code_challenge_method),
    ]);
    if let Some(st) = q.state.as_deref() {
        qs.push('&');
        qs.push_str(&form_urlencode(&[("state", st)]));
    }
    let web_base = state.settings.web_base_url.trim_end_matches('/');
    Ok(Redirect::to(&format!("{web_base}/authorize-mcp?{qs}")).into_response())
}

/// Génère un code d'autorisation éphémère (60 s) après approbation utilisateur.
async fn approve(
    State(state): State<AppState>,
    RequiredUser(user_id): RequiredUser,
    Json(body): Json<ApproveRequest>,
) -> Result<Json<serde_json::Value>> {
    if !redirect_uri_allowed(&state, &body.client_id, &body.redirect_uri).await? {
        return Err(ApiError::BadRequest(
            "invalid_client_or_redirect_uri".into(),
        ));
    }

    let code = token_urlsafe(32);
    let expires_at = Utc::now() + Duration::seconds(60);
    let conn = pool_conn(&state).await?;
    conn.execute(
        "INSERT INTO mcp_auth_codes \
           (code, user_id, client_id, code_challenge, code_challenge_method, redirect_uri, expires_at) \
         VALUES ($1, $2, $3, $4, 'S256', $5, $6)",
        &[
            &code,
            &user_id,
            &body.client_id,
            &body.code_challenge,
            &body.redirect_uri,
            &expires_at,
        ],
    )
    .await
    .map_err(store_err)?;

    Ok(Json(
        json!({"code": code, "redirect_uri": body.redirect_uri}),
    ))
}

/// Point d'entrée `POST /oauth/token` : dispatch sur `grant_type`.
///
/// RFC 6749 §4.1.3 / §6 — corps en `application/x-www-form-urlencoded`.
/// Claude.ai et ChatGPT envoient strictement ce format. Deux grants supportés :
/// `authorization_code` (échange du code + PKCE) et `refresh_token` (rotation).
async fn token(State(state): State<AppState>, Form(body): Form<TokenForm>) -> Result<Response> {
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["body", f]));
    let grant_type = body
        .grant_type
        .clone()
        .ok_or_else(|| missing("grant_type"))?;
    match grant_type.as_str() {
        "authorization_code" => token_authorization_code(&state, body).await,
        "refresh_token" => token_refresh(&state, body).await,
        _ => Err(ApiError::BadRequest("unsupported_grant_type".into())),
    }
}

/// Grant `authorization_code` : échange un code (+ PKCE S256) contre une paire
/// access token (30 j) + refresh token (90 j).
async fn token_authorization_code(state: &AppState, body: TokenForm) -> Result<Response> {
    // Champs `Form()` requis, dans l'ordre de déclaration FastAPI : un manquant
    // → 422 `{type:"missing", loc:["body", <champ>]}` (un seul rapporté).
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["body", f]));
    let code = body.code.ok_or_else(|| missing("code"))?;
    let code_verifier = body.code_verifier.ok_or_else(|| missing("code_verifier"))?;
    let client_id = body.client_id.ok_or_else(|| missing("client_id"))?;
    let redirect_uri = body.redirect_uri.ok_or_else(|| missing("redirect_uri"))?;

    let conn = pool_conn(state).await?;
    let row = conn
        .query_opt(
            "SELECT code, user_id, client_id, code_challenge, redirect_uri \
             FROM mcp_auth_codes \
             WHERE code = $1 AND expires_at > now() AND client_id = $2",
            &[&code, &client_id],
        )
        .await
        .map_err(store_err)?;

    let Some(row) = row else {
        return Err(ApiError::BadRequest("invalid_grant".into()));
    };
    let db_user_id: String = row.get(1);
    let db_client_id: String = row.get(2);
    let code_challenge: String = row.get(3);
    let stored_redirect_uri: String = row.get(4);

    if redirect_uri != stored_redirect_uri {
        return Err(ApiError::BadRequest("redirect_uri_mismatch".into()));
    }

    // Validation PKCE S256 : base64url(sha256(verifier)) sans padding.
    let computed = base64_url_no_pad(&sha256(code_verifier.as_bytes()));
    if computed != code_challenge {
        return Err(ApiError::BadRequest("invalid_code_verifier".into()));
    }

    conn.execute("DELETE FROM mcp_auth_codes WHERE code = $1", &[&code])
        .await
        .map_err(store_err)?;

    let resp = issue_token_pair(&conn, &db_user_id, &db_client_id).await?;
    Ok(Json(resp).into_response())
}

/// Grant `refresh_token` (RFC 6749 §6) avec **rotation** : consomme l'ancien
/// refresh token et en émet un neuf, pour un client public (pas de secret, la
/// rotation est la contre-mesure OAuth 2.1 contre le rejeu d'un token volé).
async fn token_refresh(state: &AppState, body: TokenForm) -> Result<Response> {
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["body", f]));
    let refresh_token = body.refresh_token.ok_or_else(|| missing("refresh_token"))?;
    let client_id = body.client_id.ok_or_else(|| missing("client_id"))?;

    // Rotation atomique : le DELETE ... RETURNING consomme le token (rejeu
    // impossible) et ne rend une ligne que s'il était valide et non expiré.
    let conn = pool_conn(state).await?;
    let row = conn
        .query_opt(
            "DELETE FROM mcp_refresh_tokens \
             WHERE refresh_token = $1 AND client_id = $2 AND expires_at > now() \
             RETURNING user_id, client_id",
            &[&refresh_token, &client_id],
        )
        .await
        .map_err(store_err)?;

    let Some(row) = row else {
        return Err(ApiError::BadRequest("invalid_grant".into()));
    };
    let db_user_id: String = row.get(0);
    let db_client_id: String = row.get(1);

    let resp = issue_token_pair(&conn, &db_user_id, &db_client_id).await?;
    Ok(Json(resp).into_response())
}

/// Émet et persiste une paire access token (30 j) + refresh token (90 j) pour un
/// couple (utilisateur, client). Partagé par les deux grants.
async fn issue_token_pair(
    conn: &deadpool_postgres::Object,
    user_id: &str,
    client_id: &str,
) -> Result<TokenResponse> {
    let access_token = token_urlsafe(32);
    let refresh_token = token_urlsafe(32);
    let access_expires = Utc::now() + Duration::days(30);
    let refresh_expires = Utc::now() + Duration::days(90);

    conn.execute(
        "INSERT INTO mcp_tokens (access_token, user_id, client_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
        &[&access_token, &user_id, &client_id, &access_expires],
    )
    .await
    .map_err(store_err)?;
    conn.execute(
        "INSERT INTO mcp_refresh_tokens (refresh_token, user_id, client_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
        &[&refresh_token, &user_id, &client_id, &refresh_expires],
    )
    .await
    .map_err(store_err)?;

    Ok(TokenResponse {
        access_token,
        token_type: "bearer".to_string(),
        expires_in: 2_592_000, // 30 jours
        refresh_token,
    })
}

async fn oauth_as_metadata(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let base = request_base_url(&uri, &headers, &state.settings.public_base_url);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["mcp"],
    }))
}

async fn oauth_protected_resource_metadata(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let base = request_base_url(&uri, &headers, &state.settings.public_base_url);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"],
    }))
}

/// Base URL (`scheme://host`) dérivée de la requête entrante, pour les URLs
/// publiées dans la discovery OAuth (parité `_request_base_url` de `main.py`).
///
/// Derrière le reverse-proxy (Caddy, ADR 0042) elles doivent refléter l'hôte
/// vu par le client, pas la valeur configurée (port/hôte backend ≠ public) :
///
/// - schéma : `X-Forwarded-Proto` si présent, sinon le schéma de l'URI (origin-form
///   HTTP/1 → absent → `http`, comme `request.url.scheme` côté uvicorn) ;
/// - hôte : `X-Forwarded-Host` si présent, sinon l'en-tête `Host`.
///
/// Ces en-têtes ne sont fiables que derrière un proxy de confiance (le mandat
/// l'assume — Caddy les pose). Sans hôte, on retombe sur `fallback`
/// (`public_base_url`). Toujours sans `/` final.
pub(crate) fn request_base_url(uri: &Uri, headers: &HeaderMap, fallback: &str) -> String {
    let first_token = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let scheme = first_token("x-forwarded-proto")
        .or_else(|| uri.scheme_str())
        .unwrap_or("http");
    let host = first_token("x-forwarded-host").or_else(|| {
        headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    match host {
        Some(host) => format!("{scheme}://{host}")
            .trim_end_matches('/')
            .to_string(),
        None => fallback.trim_end_matches('/').to_string(),
    }
}

// ── Helpers DB ──────────────────────────────────────────────────────────────

async fn pool_conn(state: &AppState) -> Result<deadpool_postgres::Object> {
    state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))
}

fn store_err(e: deadpool_postgres::tokio_postgres::Error) -> ApiError {
    ApiError::Internal(format!("db: {e}"))
}

/// Le `redirect_uri` est-il enregistré pour ce `client_id` ? (parité avec
/// `_redirect_uri_allowed`).
async fn redirect_uri_allowed(
    state: &AppState,
    client_id: &str,
    redirect_uri: &str,
) -> Result<bool> {
    let conn = pool_conn(state).await?;
    let row = conn
        .query_opt(
            "SELECT redirect_uris FROM mcp_clients WHERE client_id = $1",
            &[&client_id],
        )
        .await
        .map_err(store_err)?;
    let Some(row) = row else { return Ok(false) };
    let uris: Vec<String> = row.get(0);
    Ok(uris.iter().any(|u| u == redirect_uri))
}

// ── Crypto / encodage (sans dépendance externe) ─────────────────────────────

/// Encode des octets en base64url **sans padding** (RFC 4648 §5).
///
/// Équivalent de `base64.urlsafe_b64encode(...).rstrip(b"=")` (PKCE) et de la
/// sérialisation de `secrets.token_urlsafe`.
fn base64_url_no_pad(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

/// Token aléatoire URL-safe (parité avec `secrets.token_urlsafe(n)` : `n`
/// octets de hasard cryptographique encodés en base64url sans padding).
fn token_urlsafe(n_bytes: usize) -> String {
    base64_url_no_pad(&random_bytes(n_bytes))
}

/// Octets aléatoires d'origine OS via `getrandom` (syscall `getrandom(2)` sous
/// Linux) — pas de file descriptor, pas de read bloquant sur le thread async.
/// Panique si la source d'entropie de l'OS échoue : un secret OAuth ne doit
/// jamais être dérivé d'un fallback non cryptographique.
fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("getrandom OS entropy");
    buf
}

/// SHA-256 (FIPS 180-4) en Rust pur — pour la vérification PKCE S256.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Encode `application/x-www-form-urlencoded` (RFC 3986, espace → `%20` comme
/// `urllib.parse.urlencode` avec `quote_via` par défaut `quote_plus`… mais
/// Python `urlencode` utilise `quote_plus` → espace devient `+`). On reproduit
/// `quote_plus`.
fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&quote_plus(k));
        out.push('=');
        out.push_str(&quote_plus(v));
    }
    out
}

/// `urllib.parse.quote_plus` : caractères non réservés inchangés, espace → `+`,
/// reste en `%XX` majuscule.
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        // Vecteurs FIPS 180-4 standards.
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn base64_url_no_pad_matches_rfc4648() {
        // "foobar" → Zm9vYmFy (pas de padding ici), "foo" → Zm9v, "fo" → Zm8.
        assert_eq!(base64_url_no_pad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64_url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64_url_no_pad(b"f"), "Zg");
        // URL-safe alphabet : '-' et '_' à la place de '+' et '/'.
        assert_eq!(base64_url_no_pad(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn pkce_s256_challenge_matches_rfc7636_example() {
        // RFC 7636 annexe B : verifier de référence → challenge attendu.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64_url_no_pad(&sha256(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn quote_plus_space_and_reserved() {
        assert_eq!(quote_plus("a b"), "a+b");
        assert_eq!(quote_plus("x&y=z"), "x%26y%3Dz");
        assert_eq!(quote_plus("café"), "caf%C3%A9");
        assert_eq!(quote_plus("keep-_.~"), "keep-_.~");
    }

    #[test]
    fn token_urlsafe_len_and_alphabet() {
        let t = token_urlsafe(32);
        // 32 octets → 43 caractères base64url sans padding (ceil(32/3)*4 - pad).
        assert_eq!(t.len(), 43);
        assert!(t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // Deux tirages diffèrent (entropie OS).
        assert_ne!(t, token_urlsafe(32));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn base_url_prefers_forwarded_then_host_then_fallback() {
        let uri: Uri = "/.well-known/oauth-authorization-server".parse().unwrap();
        // X-Forwarded-* gagnent (cas reverse-proxy Caddy).
        assert_eq!(
            request_base_url(
                &uri,
                &headers(&[
                    ("X-Forwarded-Proto", "https"),
                    ("X-Forwarded-Host", "librejustice.fr"),
                    ("Host", "127.0.0.1:8301"),
                ]),
                "https://fallback.example",
            ),
            "https://librejustice.fr"
        );
        // Sans forwarded : schéma `http` (URI origin-form) + en-tête Host.
        assert_eq!(
            request_base_url(
                &uri,
                &headers(&[("Host", "127.0.0.1:8301")]),
                "https://fallback.example",
            ),
            "http://127.0.0.1:8301"
        );
        // Plusieurs valeurs forwarded : on garde la première, trimée.
        assert_eq!(
            request_base_url(
                &uri,
                &headers(&[
                    ("X-Forwarded-Proto", "https, http"),
                    ("X-Forwarded-Host", "a.example , b.example"),
                ]),
                "https://fallback.example",
            ),
            "https://a.example"
        );
        // Aucun hôte disponible → fallback configuré (sans `/` final).
        assert_eq!(
            request_base_url(&uri, &HeaderMap::new(), "https://fallback.example/"),
            "https://fallback.example"
        );
    }
}
