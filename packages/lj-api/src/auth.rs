//! Validation JWT Supabase + JIT provisioning du profil user local.
//!
//! Port de `apps/api/.../auth.py`. À chaque requête authentifiée, le row
//! `users(sub, …)` est upserté (création + bump `last_seen_at`). Le `sub`
//! Supabase sert de PK opaque (ADR 0036).
//!
//! Vérification : ES256/RSA via JWKS Supabase (`/auth/v1/.well-known/jwks.json`),
//! audience `authenticated`. Pilotée par l'extracteur [`OptionalUser`].

use crate::error::{ApiError, Result};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Forme minimale des claims qu'on lit après vérification de signature.
#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    user_metadata: Option<serde_json::Value>,
}

/// Extrait un `display_name` depuis les claims JWT Supabase.
///
/// Préfère `user_metadata.full_name` (rempli par les providers OAuth), fallback
/// sur `user_metadata.name`. Sinon `None` — l'utilisateur le remplira lui-même
/// via `PATCH /me`. Tronqué à 80 caractères (parité avec `_display_name_hint`).
fn display_name_hint(user_metadata: Option<&serde_json::Value>) -> Option<String> {
    let meta = user_metadata?.as_object()?;
    for key in ["full_name", "name"] {
        if let Some(v) = meta.get(key).and_then(|v| v.as_str()) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(80).collect());
            }
        }
    }
    None
}

// ── Cache JWKS par URL Supabase (parité avec `_jwks_clients`) ───────────────

struct JwksEntry {
    set: JwkSet,
    fetched_at: Instant,
}

/// Durée de vie du cache JWKS (parité avec `lifespan=3600` côté PyJWKClient).
const JWKS_LIFESPAN: Duration = Duration::from_secs(3600);

fn jwks_cache() -> &'static Mutex<HashMap<String, JwksEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, JwksEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Récupère le JWKS (cache 1 h) puis sélectionne la clé par `kid`.
///
/// Renvoie `None` (→ token ignoré, comme `PyJWKClientError` côté Python) si le
/// fetch échoue ou si aucune clé ne correspond.
async fn signing_key_for(supabase_url: &str, kid: &str) -> Option<DecodingKey> {
    // 1. Cache chaud ?
    {
        let cache = jwks_cache().lock().ok()?;
        if let Some(entry) = cache.get(supabase_url) {
            if entry.fetched_at.elapsed() < JWKS_LIFESPAN {
                if let Some(key) = key_from_set(&entry.set, kid) {
                    return Some(key);
                }
            }
        }
    }

    // 2. Fetch + mise en cache.
    let jwks_url = format!(
        "{}/auth/v1/.well-known/jwks.json",
        supabase_url.trim_end_matches('/')
    );
    let set: JwkSet = reqwest::get(&jwks_url).await.ok()?.json().await.ok()?;
    let key = key_from_set(&set, kid);
    if let Ok(mut cache) = jwks_cache().lock() {
        cache.insert(
            supabase_url.to_string(),
            JwksEntry {
                set,
                fetched_at: Instant::now(),
            },
        );
    }
    key
}

fn key_from_set(set: &JwkSet, kid: &str) -> Option<DecodingKey> {
    let jwk = set.find(kid)?;
    match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) => {
            DecodingKey::from_ec_components(&ec.x, &ec.y).ok()
        }
        AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e).ok(),
        _ => None,
    }
}

/// JIT upsert du profil user local (parité avec `_jit_upsert_user`).
///
/// Idempotent : ne réécrit jamais un `email` / `display_name` déjà renseigné
/// (`COALESCE`), bump `last_seen_at`.
async fn jit_upsert_user(
    state: &AppState,
    sub: &str,
    email: Option<&str>,
    display_name: Option<&str>,
) -> Result<()> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "INSERT INTO users (sub, email, display_name) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (sub) DO UPDATE SET \
           email        = COALESCE(users.email, EXCLUDED.email), \
           display_name = COALESCE(users.display_name, EXCLUDED.display_name), \
           last_seen_at = now()",
        &[&sub, &email, &display_name],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("jit_upsert_user: {e}")))?;
    Ok(())
}

/// Décode + vérifie un token Supabase (ES256/RSA via JWKS, audience
/// `authenticated`) et fait le JIT upsert. Renvoie `Some(sub)` ou `None` (token
/// absent/invalide/JWKS HS), jamais d'erreur — parité stricte avec `auth.py`.
async fn resolve_supabase_user(state: &AppState, token: &str) -> Option<String> {
    let supabase_url = state.settings.supabase_url.as_deref()?;

    let header = decode_header(token).ok()?;
    let kid = header.kid.as_deref()?;
    let signing_key = signing_key_for(supabase_url, kid).await?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&["authenticated"]);
    let data = decode::<SupabaseClaims>(token, &signing_key, &validation).ok()?;
    let claims = data.claims;

    let sub = claims.sub.clone();
    let email = claims.email.clone();
    let display = display_name_hint(claims.user_metadata.as_ref());
    // Best-effort : un échec d'upsert ne doit pas casser la requête lue (parité
    // avec Python où l'upsert ne lève pas vers le handler en cas de souci DB —
    // ici on log et on rend quand même le sub).
    if let Err(e) = jit_upsert_user(state, &sub, email.as_deref(), display.as_deref()).await {
        tracing::warn!("jit_upsert_user failed: {e}");
    }
    Some(sub)
}

/// Extrait le bearer token de l'en-tête `Authorization`.
fn bearer_token(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    let text = value.to_str().ok()?;
    text.strip_prefix("Bearer ")
        .or_else(|| text.strip_prefix("bearer "))
        .map(str::to_string)
}

/// Extracteur axum : `user_id` (sub Supabase) ou `None`.
///
/// Parité avec `optional_user` : token absent/invalide → `None` (jamais 401).
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<String>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Some(token) = bearer_token(parts) else {
            return Ok(OptionalUser(None));
        };
        Ok(OptionalUser(resolve_supabase_user(state, &token).await))
    }
}

/// Extracteur axum : `user_id` requis, sinon 401 `auth_required`.
///
/// Parité avec `required_user`.
#[derive(Debug, Clone)]
pub struct RequiredUser(pub String);

impl FromRequestParts<AppState> for RequiredUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Ok(OptionalUser(user)) = OptionalUser::from_request_parts(parts, state).await;
        user.map(RequiredUser).ok_or(ApiError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_name_prefers_full_name() {
        let meta = json!({"full_name": "  Jean Dupont  ", "name": "jdupont"});
        assert_eq!(
            display_name_hint(Some(&meta)).as_deref(),
            Some("Jean Dupont")
        );
    }

    #[test]
    fn display_name_falls_back_to_name() {
        let meta = json!({"name": "jdupont"});
        assert_eq!(display_name_hint(Some(&meta)).as_deref(), Some("jdupont"));
    }

    #[test]
    fn display_name_none_when_blank_or_missing() {
        assert_eq!(display_name_hint(None), None);
        let blank = json!({"full_name": "   ", "name": ""});
        assert_eq!(display_name_hint(Some(&blank)), None);
        let not_obj = json!("nope");
        assert_eq!(display_name_hint(Some(&not_obj)), None);
    }

    #[test]
    fn display_name_truncated_to_80() {
        let long = "a".repeat(200);
        let meta = json!({ "full_name": long });
        assert_eq!(display_name_hint(Some(&meta)).unwrap().chars().count(), 80);
    }
}
