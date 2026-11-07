//! Device authorisation against an OpenAI account, and the refresh that follows.
//!
//! OpenAI does not advertise this grant in its OIDC discovery document, which
//! lists `authorization_code` and `refresh_token` only. The contract below is the
//! one `codex` 0.154.0 speaks, read off its source
//! (`login/src/device_code_auth.rs`, `login/src/oauth/client.rs`): the device poll
//! hands back an authorization code carrying its own PKCE pair, which a second
//! call trades for tokens, and it answers 403 or 404 until a browser approves.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ISSUER: &str = "https://auth.openai.com";

/// Public PKCE client of the Codex CLI. Not a secret, and the only client this
/// grant accepts unless the account registers its own.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Where the human types the `user_code`, read off `codex login --device-auth`
/// 0.154.0: the same path on `chatgpt.com` answers 404.
pub const VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";

/// An open device authorisation, waiting on a browser.
#[derive(Debug)]
pub struct DeviceAuth {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval: Duration,
    pub expires_at: String,
}

/// What a completed authorisation hands back.
#[derive(Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub account_id: String,
    pub user_id: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub expires_in_secs: Option<i64>,
}

/// What a refresh hands back. Every field is optional on the wire, so the holder
/// keeps what the answer leaves out.
#[derive(Debug)]
pub struct Refreshed {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in_secs: Option<i64>,
}

impl Tokens {
    fn parse(body: &Value) -> Result<Self> {
        let field = |name: &str| -> Result<String> {
            body.get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow!("réponse de jeton sans {name}"))
        };
        let id_token = field("id_token")?;
        let access_token = field("access_token")?;
        Ok(Self {
            expires_in_secs: expires_in(body, Some(&access_token)),
            access_token,
            refresh_token: field("refresh_token")?,
            account_id: account_id(&id_token)?,
            user_id: user_id(&id_token)?,
            email: email(&id_token),
            plan: plan_type(&id_token),
            id_token,
        })
    }
}

/// Workspace the token speaks for, read from the id token. It names the account in
/// the `ChatGPT-Account-Id` header, and several seats share it.
pub fn account_id(id_token: &str) -> Result<String> {
    auth_claims(id_token)?["chatgpt_account_id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("id_token sans chatgpt_account_id"))
}

/// Seat the token speaks for, which with the workspace identifies a credential: each
/// pair carries its own window.
pub fn user_id(id_token: &str) -> Result<String> {
    auth_claims(id_token)?["chatgpt_user_id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("id_token sans chatgpt_user_id"))
}

/// Address of the approving seat, which labels the row without a hand.
pub fn email(id_token: &str) -> Option<String> {
    jwt_claims(id_token).ok()?["email"]
        .as_str()
        .map(str::to_string)
}

/// Plan of the workspace the approving browser sat in, `team` for ChatGPT Business.
pub fn plan_type(id_token: &str) -> Option<String> {
    auth_claims(id_token).ok()?["chatgpt_plan_type"]
        .as_str()
        .map(str::to_string)
}

/// Opens the window and returns the code to type in a browser.
pub async fn start_device_auth(http: &reqwest::Client, client_id: &str) -> Result<DeviceAuth> {
    let body: Value = http
        .post(format!("{ISSUER}/api/accounts/deviceauth/usercode"))
        .json(&json!({ "client_id": client_id }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let text = |name: &str| -> Result<String> {
        body.get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("ouverture du device sans {name}"))
    };
    let interval = body
        .get("interval")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| v.as_u64())
        })
        .unwrap_or(5);
    Ok(DeviceAuth {
        device_auth_id: text("device_auth_id")?,
        user_code: text("user_code")?,
        interval: Duration::from_secs(interval),
        expires_at: text("expires_at").unwrap_or_default(),
    })
}

/// `Ok(None)` while the browser has not approved yet. The approval hands back an
/// authorization code and the verifier the server minted for it; the tokens come
/// from trading that code.
pub async fn poll_device_token(
    http: &reqwest::Client,
    client_id: &str,
    auth: &DeviceAuth,
) -> Result<Option<Tokens>> {
    let response = http
        .post(format!("{ISSUER}/api/accounts/deviceauth/token"))
        .json(&json!({
            "device_auth_id": auth.device_auth_id,
            "user_code": auth.user_code,
        }))
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("device refusé ({status}): {body}");
    }
    let field = |name: &str| -> Result<&str> {
        body.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("approbation sans {name}"))
    };
    exchange_code(
        http,
        client_id,
        field("authorization_code")?,
        field("code_verifier")?,
    )
    .await
    .map(Some)
}

async fn exchange_code(
    http: &reqwest::Client,
    client_id: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens> {
    let redirect = format!("{ISSUER}/deviceauth/callback");
    let body: Value = http
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", &redirect),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Tokens::parse(&body)
}

/// Trades the refresh token for a fresh set. The old one dies here, so what this
/// returns must be stored before another process asks.
pub async fn refresh(
    http: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<Refreshed> {
    let body: Value = http
        .post(format!("{ISSUER}/oauth/token"))
        .json(&json!({
            "grant_type": "refresh_token",
            "client_id": client_id,
            "refresh_token": refresh_token,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
    let access_token = text("access_token");
    Ok(Refreshed {
        expires_in_secs: expires_in(&body, access_token.as_deref()),
        access_token,
        refresh_token: text("refresh_token"),
        id_token: text("id_token"),
    })
}

/// The `expires_in` of the answer, or, the field being absent, the `exp` of the
/// access token, which is a JWT.
fn expires_in(body: &Value, access_token: Option<&str>) -> Option<i64> {
    if let Some(secs) = body.get("expires_in").and_then(Value::as_i64) {
        return Some(secs);
    }
    let exp = jwt_claims(access_token?).ok()?["exp"].as_i64()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(exp - now)
}

fn auth_claims(id_token: &str) -> Result<Value> {
    Ok(jwt_claims(id_token)?["https://api.openai.com/auth"].clone())
}

fn jwt_claims(token: &str) -> Result<Value> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("jeton sans charge utile"))?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .context("charge utile du jeton illisible")?;
    Ok(serde_json::from_slice(&raw)?)
}
