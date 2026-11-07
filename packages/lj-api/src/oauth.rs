//! Endpoints OAuth 2.1 (Authorization Code + PKCE S256 + DCR) pour l'accès MCP.
//!
//! Spec MCP « Authorization » (2025-03-26), requise par Claude.ai Custom
//! Connectors et ChatGPT Connectors :
//!
//! - Authorization Code + PKCE S256 (RFC 6749 / 7636)
//! - Dynamic Client Registration (RFC 7591) — `POST /oauth/register`
//! - Token Revocation (RFC 7009) — `POST /oauth/revoke`
//! - Authorization Server Metadata (RFC 8414) — `/.well-known/oauth-authorization-server`
//! - Protected Resource Metadata (RFC 9728) — `/.well-known/oauth-protected-resource`
//! - OpenID Connect Discovery : `/.well-known/openid-configuration`
//! - UserInfo (OIDC Core §5.3) : `GET|POST /oauth/userinfo`
//!
//! Clients publics uniquement : pas de `client_secret`, la sécurité repose sur
//! PKCE.
//!
//! The ES256 `id_token` and its JWKS only exist when a signing key is
//! configured; without one the server issues no token and discovery advertises
//! `none`, rather than promising a signature it cannot produce.

use crate::auth::OptionalUser;
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;
use axum::extract::{Form, Query, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ── DTOs ────────────────────────────────────────────────────────────────────

/// An access token carries the same authority whatever the client asked for:
/// the MCP tools, and the identity of the account that issued it. So the token
/// endpoint returns these three and UserInfo serves any valid token, which is
/// what spares a scope column on `mcp_auth_codes` and `mcp_tokens`. Every
/// discovery document announces the same list, the resource one included.
const SCOPES: [&str; 3] = ["mcp", "openid", "email"];

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
    // Paramètres requis : un manquant → 422 de validation, avant les contrôles
    // métier 400. Capturés en `Option` pour rendre ce 422 explicite.
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

/// Formulaire de consentement posté par `/authorize-mcp` (HTML pur, sans JS).
/// Champs requis en `Option` pour le 422 de validation, comme [`TokenForm`].
#[derive(Debug, Deserialize)]
pub struct ApproveForm {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub code_challenge: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenForm {
    // Champs de formulaire requis : un manquant → 422 de validation
    // (`{type:"missing", loc:["body", <champ>]}`). Capturés en `Option` pour
    // construire ce corps à la main.
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
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

/// RFC 7009 §2.1 — corps de `POST /oauth/revoke`. Seul `token` est lu ;
/// `token_type_hint` est un champ non déclaré, donc écarté par serde (cf.
/// [`revoke`]).
#[derive(Debug, Deserialize)]
pub struct RevokeForm {
    #[serde(default)]
    pub token: Option<String>,
}

// ── Routeur OAuth (préfixe `/oauth`) ────────────────────────────────────────

/// Routes `/oauth/{register,authorize,approve,token,revoke,userinfo}`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/approve", post(approve))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .route("/oauth/userinfo", get(userinfo).post(userinfo))
        .layer(axum::middleware::from_fn(step_span))
}

async fn step_span(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let step = req
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let agent: String = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    let response = next.run(req).await;
    let status = response.status().as_u16();
    let _span = tracing::info_span!(
        "oauth_step",
        librejustice.oauth.step = %step,
        librejustice.oauth.status = status,
        librejustice.oauth.agent = %agent,
    )
    .entered();
    tracing::info!(agent = %agent, "oauth step: {step} -> {status}");
    response
}

/// Routes de discovery `/.well-known/*` (RFC 8414 + RFC 9728 + MCP 2025-03-26).
///
/// Each of the three documents answers all **three** path forms: the bare one,
/// the path-component one (`/.well-known/…/mcp`) Claude.ai requires, and the
/// resource-prefixed one (`/mcp/.well-known/…`) other clients try first. A
/// scanner starting with the wrong form must not read a 404 as an absence.
pub fn well_known_router() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_as_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server/mcp",
            get(oauth_as_metadata),
        )
        .route(
            "/mcp/.well-known/oauth-authorization-server",
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
            "/mcp/.well-known/oauth-protected-resource",
            get(oauth_protected_resource_metadata),
        )
        .route(
            "/.well-known/openid-configuration",
            get(openid_configuration),
        )
        .route(
            "/.well-known/openid-configuration/mcp",
            get(openid_configuration),
        )
        .route(
            "/mcp/.well-known/openid-configuration",
            get(openid_configuration),
        )
        .route("/.well-known/jwks.json", get(jwks))
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
/// schéma `connector.json`, `maintainers` = objets `{"email": …}` dont l'email
/// doit matcher le compte Glama (vérification automatique par leur crawler).
/// Lu depuis les `Settings` (`LIBREJUSTICE_API_GLAMA_MAINTAINER`) ; non
/// configuré → 404.
async fn glama_manifest(State(state): State<AppState>) -> Response {
    match state.settings.glama_maintainer.as_deref() {
        Some(email) => Json(serde_json::json!({
            "$schema": "https://glama.ai/mcp/schemas/connector.json",
            "maintainers": [{ "email": email }],
        }))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `redirect_uri` acceptable à l'enregistrement : HTTPS partout, HTTP seulement
/// sur la boucle locale.
///
/// Compare un host parsé, jamais un préfixe de chaîne :
/// `http://localhost.attaquant.tld/cb` n'est pas la boucle locale. Tout ce qui
/// est loopback est admis, pas seulement `127.0.0.1` — un client natif qui
/// écoute sur `127.0.0.2` est aussi légitimement hors réseau.
fn is_allowed_redirect_uri(uri: &str) -> bool {
    let Ok(parsed) = url::Url::parse(uri) else {
        return false;
    };
    let Some(host) = parsed.host() else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(addr) => addr.is_loopback(),
            url::Host::Ipv6(addr) => addr.is_loopback(),
        },
        _ => false,
    }
}

/// RFC 7591 — Dynamic Client Registration (public client only, PKCE-protected).
async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Response> {
    // `redirect_uris` : longueur 1 à 10, sinon 422 (et non 400).
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
        if !is_allowed_redirect_uri(uri) {
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
    // Paramètres requis manquants → 422, avant tout 400 métier. Ordre :
    // response_type, client_id, redirect_uri, code_challenge (un seul rapporté
    // à la fois).
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

/// Consent: HTML form → 303 to the MCP client, code issued (5 min) or refusal.
/// The same challenge gets the same code while it lives: a mobile browser
/// replays the submission when the return does not land.
///
/// `deny` needs no session: refusing issues nothing, and the return is checked
/// against the registered client. `approve` without a session (expired between
/// the render and the click) sends back to login, the full consent in `next`. A
/// cross-site POST does not carry the cookie (`SameSite=Lax`): it falls on that
/// path, never on a code issue.
async fn approve(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    entetes: HeaderMap,
    Form(body): Form<ApproveForm>,
) -> Result<Response> {
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["body", f]));
    let client_id = body.client_id.ok_or_else(|| missing("client_id"))?;
    let code_challenge = body
        .code_challenge
        .ok_or_else(|| missing("code_challenge"))?;
    let redirect_uri = body.redirect_uri.ok_or_else(|| missing("redirect_uri"))?;
    let action = body.action.ok_or_else(|| missing("action"))?;
    let etat = body.state.as_deref().filter(|s| !s.is_empty());

    if !redirect_uri_allowed(&state, &client_id, &redirect_uri).await? {
        return Err(ApiError::BadRequest(
            "invalid_client_or_redirect_uri".into(),
        ));
    }
    match action.as_str() {
        "deny" => return retour_client(&redirect_uri, ("error", "access_denied"), etat),
        "approve" => {}
        _ => return Err(ApiError::BadRequest("invalid_action".into())),
    }

    let Some(user_id) = user else {
        let mut qs = form_urlencode(&[
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("code_challenge", &code_challenge),
        ]);
        if let Some(st) = etat {
            qs.push('&');
            qs.push_str(&form_urlencode(&[("state", st)]));
        }
        let next = quote_plus(&format!("/authorize-mcp?{qs}"));
        return Ok(Redirect::to(&format!("/login?next={next}")).into_response());
    };

    crate::locale::retenir(
        &state,
        &user_id,
        &crate::locale::du_navigateur(&entetes),
        None,
    )
    .await?;
    let conn = pool_conn(&state).await?;
    let rejeu = conn
        .query_opt(
            "SELECT code, created_at FROM mcp_auth_codes \
              WHERE user_id = $1 AND client_id = $2 AND code_challenge = $3 \
                AND redirect_uri = $4 AND expires_at > now() \
              ORDER BY created_at DESC LIMIT 1",
            &[&user_id, &client_id, &code_challenge, &redirect_uri],
        )
        .await
        .map_err(store_err)?;
    if let Some(ligne) = rejeu {
        let code: String = ligne.get(0);
        let emis: chrono::DateTime<Utc> = ligne.get(1);
        tracing::warn!(
            librejustice.oauth.rejeu = true,
            age_ms = (Utc::now() - emis).num_milliseconds(),
            "consent replayed: same challenge, the already issued code goes out again"
        );
        return retour_client(&redirect_uri, ("code", &code), etat);
    }

    let code = token_urlsafe(32);
    let expires_at = Utc::now() + Duration::minutes(5);
    conn.execute(
        "INSERT INTO mcp_auth_codes \
           (code, user_id, client_id, code_challenge, code_challenge_method, redirect_uri, expires_at) \
         VALUES ($1, $2, $3, $4, 'S256', $5, $6)",
        &[
            &code,
            &user_id,
            &client_id,
            &code_challenge,
            &redirect_uri,
            &expires_at,
        ],
    )
    .await
    .map_err(store_err)?;

    retour_client(&redirect_uri, ("code", &code), etat)
}

/// 303 vers le `redirect_uri` (déjà validé) du client, paramètre et `state`
/// ajoutés à la query existante.
fn retour_client(
    redirect_uri: &str,
    (cle, valeur): (&str, &str),
    etat: Option<&str>,
) -> Result<Response> {
    let mut url = url::Url::parse(redirect_uri)
        .map_err(|_| ApiError::BadRequest("invalid_client_or_redirect_uri".into()))?;
    url.query_pairs_mut().append_pair(cle, valeur);
    if let Some(st) = etat {
        url.query_pairs_mut().append_pair("state", st);
    }
    Ok(Redirect::to(url.as_str()).into_response())
}

/// Point d'entrée `POST /oauth/token` : dispatch sur `grant_type`.
///
/// RFC 6749 §4.1.3 / §6 — corps en `application/x-www-form-urlencoded`.
/// Claude.ai et ChatGPT envoient strictement ce format. Deux grants supportés :
/// `authorization_code` (échange du code + PKCE) et `refresh_token` (rotation).
///
/// Le `client_id` est aussi lu dans un `Authorization: Basic` (RFC 6749 §2.3.1) :
/// `golang.org/x/oauth2` sonde le endpoint sous cette forme avant le corps.
async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(mut body): Form<TokenForm>,
) -> Result<Response> {
    if body.client_id.is_none() {
        body.client_id = client_id_basic(&headers);
    }
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

/// `client_id` porté par un en-tête `Authorization: Basic` : partie avant `:`
/// du couple base64. Nos `client_id` sont url-safe, donc jamais percent-encodés
/// par le client (RFC 6749 §2.3.1).
fn client_id_basic(headers: &HeaderMap) -> Option<String> {
    let (scheme, credentials) = headers.get(AUTHORIZATION)?.to_str().ok()?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(credentials).ok()?).ok()?;
    let (client_id, _) = decoded.split_once(':')?;
    (!client_id.is_empty()).then(|| client_id.to_string())
}

/// Grant `authorization_code` : échange un code (+ PKCE S256) contre une paire
/// access token (30 j) + refresh token (90 j).
async fn token_authorization_code(state: &AppState, body: TokenForm) -> Result<Response> {
    // Champs de formulaire requis, dans l'ordre : un manquant → 422
    // `{type:"missing", loc:["body", <champ>]}` (un seul rapporté).
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

    let computed = base64_url_no_pad(&sha256(code_verifier.as_bytes()));
    if computed != code_challenge {
        return Err(ApiError::BadRequest("invalid_code_verifier".into()));
    }

    conn.execute("DELETE FROM mcp_auth_codes WHERE code = $1", &[&code])
        .await
        .map_err(store_err)?;

    let resp = issue_token_pair(state, &conn, &db_user_id, &db_client_id).await?;
    Ok(Json(resp).into_response())
}

/// Grant `refresh_token` (RFC 6749 §6) avec **rotation** : consomme l'ancien
/// refresh token et en émet un neuf. Pour un client public, la rotation est la
/// contre-mesure OAuth 2.1 contre le rejeu d'un token volé.
async fn token_refresh(state: &AppState, body: TokenForm) -> Result<Response> {
    let missing = |f: &'static str| ApiError::Unprocessable(validation::missing(&["body", f]));
    let refresh_token = body.refresh_token.ok_or_else(|| missing("refresh_token"))?;
    let client_id = body.client_id.ok_or_else(|| missing("client_id"))?;

    // Rotation atomique : le DELETE ... RETURNING consomme le token (rejeu
    // impossible) et ne rend une ligne que s'il était valide et non expiré.
    let digest = sha256(refresh_token.as_bytes());
    let digest: &[u8] = &digest;
    let conn = pool_conn(state).await?;
    let row = conn
        .query_opt(
            "DELETE FROM mcp_refresh_tokens \
             WHERE refresh_token_sha256 = $1 AND client_id = $2 AND expires_at > now() \
             RETURNING user_id, client_id",
            &[&digest, &client_id],
        )
        .await
        .map_err(store_err)?;

    let Some(row) = row else {
        return Err(ApiError::BadRequest("invalid_grant".into()));
    };
    let db_user_id: String = row.get(0);
    let db_client_id: String = row.get(1);

    let resp = issue_token_pair(state, &conn, &db_user_id, &db_client_id).await?;
    Ok(Json(resp).into_response())
}

/// Émet et persiste une paire access token (30 j) + refresh token (90 j) pour un
/// couple (utilisateur, client). Partagé par les deux grants.
async fn issue_token_pair(
    state: &AppState,
    conn: &deadpool_postgres::Object,
    user_id: &str,
    client_id: &str,
) -> Result<TokenResponse> {
    let access_token = token_urlsafe(32);
    let refresh_token = token_urlsafe(32);
    let access_expires = Utc::now() + Duration::days(30);
    let refresh_expires = Utc::now() + Duration::days(90);

    // Seule l'empreinte est persistée : le clair ne quitte cette fonction que dans
    // la réponse au client.
    let access_digest = sha256(access_token.as_bytes());
    let access_digest: &[u8] = &access_digest;
    let refresh_digest = sha256(refresh_token.as_bytes());
    let refresh_digest: &[u8] = &refresh_digest;

    conn.execute(
        "INSERT INTO mcp_tokens (access_token_sha256, user_id, client_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
        &[&access_digest, &user_id, &client_id, &access_expires],
    )
    .await
    .map_err(store_err)?;
    conn.execute(
        "INSERT INTO mcp_refresh_tokens (refresh_token_sha256, user_id, client_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
        &[&refresh_digest, &user_id, &client_id, &refresh_expires],
    )
    .await
    .map_err(store_err)?;
    // Both token tables expire and are purged, so nothing else in the schema
    // still knows that an account once connected.
    conn.execute(
        "UPDATE users SET mcp_connected_at = now() \
         WHERE sub = $1 AND mcp_connected_at IS NULL",
        &[&user_id],
    )
    .await
    .map_err(store_err)?;

    let id_token = match state.oidc_signer.as_deref() {
        Some(signer) => {
            let row = conn
                .query_opt(
                    "SELECT email, email_confirmed_at IS NOT NULL FROM users WHERE sub = $1",
                    &[&user_id],
                )
                .await
                .map_err(store_err)?;
            let email: Option<String> = row.as_ref().and_then(|r| r.get(0));
            let verified: bool = row.as_ref().is_some_and(|r| r.get(1));
            signer.id_token(
                &public_base(state),
                user_id,
                client_id,
                email.as_deref(),
                verified,
            )
        }
        None => None,
    };

    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: 2_592_000, // 30 jours
        refresh_token,
        scope: SCOPES.join(" "),
        id_token,
    })
}

/// RFC 7009 — `POST /oauth/revoke` : le porteur d'un jeton le rend.
///
/// Corps `application/x-www-form-urlencoded`. `token_type_hint` est optionnel
/// et sans effet : la suppression vise les deux tables. Un `token` absent est
/// une requête invalide → 400 `invalid_request` (RFC 6749 §5.2).
///
/// **`token` présent ⇒ 200 à corps vide**, y compris inconnu, expiré ou
/// informe : RFC 7009 §2.2 interdit de renseigner l'appelant sur l'existence du
/// jeton. Aucune authentification de client : nos clients sont publics, la
/// connaissance du jeton est le seul justificatif qu'ils puissent produire.
///
/// La comparaison porte sur l'empreinte SHA-256. Rien ne lie en base un access
/// token à son refresh token : on rend le jeton présenté, pas son jumeau.
async fn revoke(State(state): State<AppState>, Form(body): Form<RevokeForm>) -> Result<Response> {
    let Some(token) = body.token else {
        return Err(ApiError::BadRequest("invalid_request".into()));
    };

    let digest = sha256(token.as_bytes());
    let digest: &[u8] = &digest;
    let conn = pool_conn(&state).await?;
    conn.execute(
        "DELETE FROM mcp_tokens WHERE access_token_sha256 = $1",
        &[&digest],
    )
    .await
    .map_err(store_err)?;
    conn.execute(
        "DELETE FROM mcp_refresh_tokens WHERE refresh_token_sha256 = $1",
        &[&digest],
    )
    .await
    .map_err(store_err)?;

    Ok(StatusCode::OK.into_response())
}

/// RFC 8414 and OpenID Discovery serve the **same** document under two names.
/// The OpenAI scanner reads `oauth-authorization-server` alone, so a provider that
/// only carries its OIDC fields under the second name is, to that reader, not a
/// provider at all. The union is legal on both paths.
async fn oauth_as_metadata(State(state): State<AppState>) -> Json<serde_json::Value> {
    openid_configuration(State(state)).await
}

async fn openid_configuration(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(openid_configuration_doc(
        &public_base(&state),
        state.oidc_signer.is_some(),
    ))
}

/// JWKS of the `id_token` signature. No key configured, no document: a `keys`
/// array served empty would let a client believe a signature is verifiable.
async fn jwks(State(state): State<AppState>) -> Response {
    match state.oidc_signer.as_deref() {
        Some(signer) => Json(signer.jwks()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The single discovery document, RFC 8414 and OpenID Discovery 1.0 at once:
/// where to send the user, how to exchange codes, and who the user is.
fn openid_configuration_doc(base: &str, signed: bool) -> serde_json::Value {
    let mut doc = json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "userinfo_endpoint": format!("{base}/oauth/userinfo"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        // Announced: absent the field, the RFC 8414 default is
        // `client_secret_basic`, which our public clients cannot produce.
        "revocation_endpoint_auth_methods_supported": ["none"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["none"],
        "scopes_supported": SCOPES,
        "claims_supported": ["sub", "email", "email_verified"],
    });
    // The algorithm follows the key actually loaded, never the intention: a
    // discovery promising ES256 with no JWKS behind it fails every validator.
    if signed {
        doc["id_token_signing_alg_values_supported"] = json!(["ES256"]);
        doc["jwks_uri"] = json!(format!("{base}/.well-known/jwks.json"));
    }
    doc
}

/// OIDC Core §5.3. The bearer of an MCP access token reads the identity of the
/// account that issued it. ChatGPT Enterprise workspaces need it to compare the
/// email domain with the ones they have verified.
async fn userinfo(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token else {
        return userinfo_challenge("invalid_request");
    };

    let digest = sha256(token.as_bytes());
    let digest: &[u8] = &digest;
    let conn = match pool_conn(&state).await {
        Ok(conn) => conn,
        Err(e) => return e.into_response(),
    };
    let row = conn
        .query_opt(
            "SELECT u.sub, u.email, u.email_confirmed_at IS NOT NULL \
             FROM mcp_tokens t JOIN users u ON u.sub = t.user_id \
             WHERE t.access_token_sha256 = $1 AND t.expires_at > now()",
            &[&digest],
        )
        .await;
    let row = match row {
        Ok(row) => row,
        Err(e) => return store_err(e).into_response(),
    };
    let Some(row) = row else {
        return userinfo_challenge("invalid_token");
    };

    let sub: String = row.get(0);
    let email: Option<String> = row.get(1);
    let verified: bool = row.get(2);
    Json(userinfo_claims(&sub, email.as_deref(), verified)).into_response()
}

/// RFC 6750 §3: a refusal names its cause in the challenge, and the body stays
/// empty.
fn userinfo_challenge(error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::WWW_AUTHENTICATE,
            format!("Bearer realm=\"mcp\", error=\"{error}\""),
        )],
    )
        .into_response()
}

/// `sub` always, the address and its verified state only when the account
/// carries one: OIDC requires `sub` alone, and a missing address is not an
/// empty string.
fn userinfo_claims(sub: &str, email: Option<&str>, verified: bool) -> serde_json::Value {
    match email {
        Some(email) => json!({ "sub": sub, "email": email, "email_verified": verified }),
        None => json!({ "sub": sub }),
    }
}

async fn oauth_protected_resource_metadata(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let base = public_base(&state);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "scopes_supported": SCOPES,
        "bearer_methods_supported": ["header"],
    }))
}

/// Base URL publiée dans la discovery OAuth et le challenge MCP, sans `/` final.
///
/// Vaut `public_base_url` et rien d'autre : la dériver de la requête entrante
/// (`X-Forwarded-Proto` / `X-Forwarded-Host`, sinon `Host`) laisserait le
/// client dicter les URLs que nos propres documents de discovery annoncent.
///
/// En dev local, l'hôte se pose donc en config —
/// `LIBREJUSTICE_API_PUBLIC_BASE_URL=http://localhost:3000`.
fn public_base(state: &AppState) -> String {
    state
        .settings
        .public_base_url
        .trim_end_matches('/')
        .to_string()
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
    ApiError::Internal(format!("db: {}", lj_pg::error::pg_detail(&e)))
}

/// Nom déclaré par le client à son enregistrement (RFC 7591 §2), pour l'écran
/// de consentement — la query d'autorisation ne porte que le `client_id`, qui
/// n'apprend rien à personne. `None` si le client n'en a pas déclaré.
pub async fn client_name(state: &AppState, client_id: &str) -> Result<Option<String>> {
    let conn = pool_conn(state).await?;
    let row = conn
        .query_opt(
            "SELECT name FROM mcp_clients WHERE client_id = $1",
            &[&client_id],
        )
        .await
        .map_err(store_err)?;
    Ok(row
        .and_then(|row| row.get::<_, Option<String>>(0))
        .filter(|nom| !nom.trim().is_empty()))
}

/// Le `redirect_uri` est-il enregistré pour ce `client_id` ?
pub(crate) async fn redirect_uri_allowed(
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
pub(crate) fn base64_url_no_pad(data: &[u8]) -> String {
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

/// Token aléatoire URL-safe : `n` octets de hasard cryptographique encodés en
/// base64url sans padding.
pub(crate) fn token_urlsafe(n_bytes: usize) -> String {
    base64_url_no_pad(&random_bytes(n_bytes))
}

/// Octets aléatoires d'origine OS via `getrandom` (syscall `getrandom(2)` sous
/// Linux) — pas de file descriptor, pas de read bloquant sur le thread async.
/// Panique si la source d'entropie de l'OS échoue.
pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("getrandom OS entropy");
    buf
}

/// SHA-256 (FIPS 180-4) en Rust pur — vérification PKCE S256 et empreinte des
/// jetons.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
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

    for block in msg.as_chunks::<64>().0 {
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

/// Encode `application/x-www-form-urlencoded` : espace → `+` (`quote_plus`).
pub(crate) fn form_urlencode(pairs: &[(&str, &str)]) -> String {
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
pub(crate) fn quote_plus(s: &str) -> String {
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
    fn redirect_uri_http_hors_boucle_locale_est_refuse() {
        for uri in [
            "http://localhost.attaquant.tld/cb",
            "http://127.0.0.1.attaquant.tld/cb",
            "http://localhost@attaquant.tld/cb",
            "http://attaquant.tld/cb",
            "http://[::1].attaquant.tld/cb",
        ] {
            assert!(!is_allowed_redirect_uri(uri), "doit refuser {uri}");
        }
    }

    #[test]
    fn redirect_uri_boucle_locale_en_clair_est_admise() {
        for uri in [
            "http://localhost/cb",
            "http://localhost:1234/cb",
            "http://LocalHost:1234/cb",
            "http://127.0.0.1/cb",
            "http://127.0.0.1:8080/cb",
            "http://127.0.0.2:8080/cb",
            "http://[::1]:8080/cb",
        ] {
            assert!(is_allowed_redirect_uri(uri), "doit admettre {uri}");
        }
    }

    #[test]
    fn redirect_uri_https_partout_autres_schemas_refuses() {
        assert!(is_allowed_redirect_uri("https://client.example/cb"));
        assert!(!is_allowed_redirect_uri("https://"));
        // `url` normalise l'autorité vide en prenant le 1er segment pour host :
        // `http:///cb` a donc pour host `cb`, qui n'est pas la boucle locale.
        assert!(!is_allowed_redirect_uri("http:///cb"));
        for uri in [
            "app://callback",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://client.example/cb",
            "pas-une-url",
        ] {
            assert!(!is_allowed_redirect_uri(uri), "doit refuser {uri}");
        }
    }

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
        assert_eq!(base64_url_no_pad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64_url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64_url_no_pad(b"f"), "Zg");
        assert_eq!(base64_url_no_pad(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn client_id_lu_dans_authorization_basic() {
        let basic = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(AUTHORIZATION, v.parse().unwrap());
            client_id_basic(&h)
        };
        // `golang.org/x/oauth2` : client public, donc secret vide après le `:`.
        assert_eq!(
            basic("Basic bGpfYUJjRGVGOg=="),
            Some("lj_aBcDeF".to_string())
        );
        assert_eq!(
            basic("basic bGpfYUJjRGVGOnNlY3JldA=="),
            Some("lj_aBcDeF".into())
        );
        assert_eq!(basic("Bearer bGpfYUJjRGVGOg=="), None);
        assert_eq!(basic("Basic Og=="), None);
        assert_eq!(basic("Basic pas-du-base64!"), None);
        assert_eq!(client_id_basic(&HeaderMap::new()), None);
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
        assert_ne!(t, token_urlsafe(32));
    }

    #[test]
    fn openid_configuration_porte_ce_qu_exige_la_restriction_de_domaine() {
        let doc = openid_configuration_doc("https://librejustice.fr", true);
        assert_eq!(doc["issuer"], "https://librejustice.fr");
        assert_eq!(
            doc["userinfo_endpoint"],
            "https://librejustice.fr/oauth/userinfo"
        );
        for scope in ["mcp", "openid", "email"] {
            assert!(
                doc["scopes_supported"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s == scope),
                "scope {scope} absent de la découverte"
            );
        }
        for claim in ["sub", "email", "email_verified"] {
            assert!(doc["claims_supported"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == claim));
        }
        assert_eq!(doc["code_challenge_methods_supported"][0], "S256");
        assert_eq!(doc["subject_types_supported"][0], "public");
        assert_eq!(doc["id_token_signing_alg_values_supported"][0], "ES256");
        assert_eq!(
            doc["jwks_uri"],
            "https://librejustice.fr/.well-known/jwks.json"
        );

        // The same document is served under the RFC 8414 name, so nothing that
        // one carried may have been lost in the merge.
        for field in [
            "registration_endpoint",
            "revocation_endpoint",
            "revocation_endpoint_auth_methods_supported",
            "token_endpoint_auth_methods_supported",
            "response_types_supported",
            "grant_types_supported",
        ] {
            assert!(!doc[field].is_null(), "{field} absent du document unique");
        }

        let sans_cle = openid_configuration_doc("https://librejustice.fr", false);
        assert_eq!(sans_cle["id_token_signing_alg_values_supported"][0], "none");
        assert!(sans_cle["jwks_uri"].is_null());
    }

    #[test]
    fn userinfo_rend_sub_seul_quand_le_compte_n_a_pas_d_adresse() {
        assert_eq!(
            userinfo_claims("u-1", Some("a@b.fr"), true),
            json!({"sub": "u-1", "email": "a@b.fr", "email_verified": true})
        );
        assert_eq!(
            userinfo_claims("u-1", Some("a@b.fr"), false),
            json!({"sub": "u-1", "email": "a@b.fr", "email_verified": false})
        );
        assert_eq!(userinfo_claims("u-1", None, false), json!({"sub": "u-1"}));
    }

    #[test]
    fn le_jeton_annonce_les_trois_scopes() {
        assert_eq!(SCOPES.join(" "), "mcp openid email");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
