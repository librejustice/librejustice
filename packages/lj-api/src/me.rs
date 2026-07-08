//! Endpoints `/me/*` — lecture, mise à jour et suppression du profil utilisateur
//! (port de `me.py`). Cf. ADR 0036 (création/JIT), 0039 (suppression + RGPD),
//! 0056 (toggle d'enregistrement d'activité).
//!
//! Les handlers axum vivent dans `routes.rs` ; ce module porte la logique
//! d'accès aux données + Supabase, exposée comme fonctions de service prenant le
//! pool / l'`AppState`.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use lj_dtos::UserProfile;
use reqwest_middleware::ClientBuilder;
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};

use crate::config::Settings;
use crate::error::ApiError;
use crate::state::AppState;

/// Formate un timestamp Postgres `TIMESTAMPTZ` en RFC 3339 (parité avec la
/// sérialisation JSON Pydantic d'un `datetime.datetime`).
pub(crate) fn ts_to_rfc3339(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Lit le profil utilisateur (parité `_fetch_profile`).
///
/// Renvoie [`ApiError::NotFound`] si la ligne est absente (théoriquement
/// impossible : `required_user` vient de provisionner — `detail=profile_missing`).
pub async fn fetch_profile(pool: &Pool, sub: &str) -> Result<UserProfile, ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let row = conn
        .query_opt(
            "SELECT sub, email, display_name, created_at, last_seen_at, track_activity \
             FROM users WHERE sub = $1",
            &[&sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("fetch_profile: {e}")))?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(UserProfile {
        sub: row.get(0),
        email: row.get(1),
        display_name: row.get(2),
        created_at: ts_to_rfc3339(row.get(3)),
        last_seen_at: ts_to_rfc3339(row.get(4)),
        track_activity: row.get(5),
    })
}

/// `GET /me` (parité `get_me`).
pub async fn get_me(state: &AppState, user_sub: &str) -> Result<UserProfile, ApiError> {
    fetch_profile(&state.pool, user_sub).await
}

/// `PATCH /me` — met à jour le `display_name` puis relit le profil (parité
/// `patch_me`).
pub async fn patch_me(
    state: &AppState,
    user_sub: &str,
    display_name: Option<&str>,
) -> Result<UserProfile, ApiError> {
    {
        let conn = state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
        conn.execute(
            "UPDATE users SET display_name = $1 WHERE sub = $2",
            &[&display_name, &user_sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("patch_me: {e}")))?;
    }
    fetch_profile(&state.pool, user_sub).await
}

/// `PUT /me/activity-tracking` — active/désactive l'enregistrement d'activité
/// (ADR 0056). Désactiver = mode ZDR : on coupe le tracking **et** on purge tout
/// l'existant (recherches, lectures, signets) dans une seule transaction (parité
/// `set_activity_tracking`).
pub async fn set_activity_tracking(
    state: &AppState,
    user_sub: &str,
    enabled: bool,
) -> Result<UserProfile, ApiError> {
    {
        let mut conn = state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| ApiError::Internal(format!("tx begin: {e}")))?;
        tx.execute(
            "UPDATE users SET track_activity = $1 WHERE sub = $2",
            &[&enabled, &user_sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("set_activity_tracking: {e}")))?;
        if !enabled {
            tx.execute(
                "DELETE FROM user_search_history WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge history: {e}")))?;
            tx.execute(
                "DELETE FROM user_decision_views WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge views: {e}")))?;
            tx.execute(
                "DELETE FROM user_bookmarks WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge bookmarks: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| ApiError::Internal(format!("tx commit: {e}")))?;
    }
    fetch_profile(&state.pool, user_sub).await
}

/// Supprime l'utilisateur dans Supabase Auth via l'Admin API (parité
/// `_delete_supabase_user`).
///
/// Requiert `supabase_url` + `supabase_secret_key`. Strictement côté serveur.
/// 200/204 attendus ; 404 toléré (déjà supprimé → idempotence) ; tout autre code
/// = 502.
async fn delete_supabase_user(cfg: &Settings, sub: &str) -> Result<(), ApiError> {
    let (url_base, key) = match (
        cfg.supabase_url.as_deref(),
        cfg.supabase_secret_key.as_deref(),
    ) {
        (Some(u), Some(k)) => (u, k),
        _ => {
            // 503 account_deletion_unavailable côté Python.
            return Err(ApiError::Internal("account_deletion_unavailable".into()));
        }
    };
    let url = format!(
        "{}/auth/v1/admin/users/{}",
        url_base.trim_end_matches('/'),
        sub
    );
    // TracingMiddleware -> span HTTP client pour l'appel Supabase admin en Tempo.
    let inner = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Internal(format!("supabase http client: {e}")))?;
    let client = ClientBuilder::new(inner)
        .with(TracingMiddleware::<SpanBackendWithUrl>::new())
        .build();
    let resp = client
        .delete(&url)
        .bearer_auth(key)
        .header("apikey", key)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("supabase request: {e}")))?;
    let status = resp.status().as_u16();
    if !matches!(status, 200 | 204 | 404) {
        let body = resp.text().await.unwrap_or_default();
        let body_head: String = body.chars().take(200).collect();
        tracing::error!(status, body = %body_head, "supabase admin deleteUser failed");
        return Err(ApiError::Internal("supabase_delete_failed".into()));
    }
    Ok(())
}

/// `DELETE /me` — suppression définitive du compte (parité `delete_me`).
///
/// Ordre : Supabase Auth d'abord (échec → DB locale intacte, l'utilisateur peut
/// retenter). Puis `DELETE FROM users` qui cascade sur bookmarks / history /
/// decision_views / mcp_tokens / mcp_auth_codes (ON DELETE CASCADE).
pub async fn delete_me(state: &AppState, user_sub: &str) -> Result<(), ApiError> {
    delete_supabase_user(&state.settings, user_sub).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute("DELETE FROM users WHERE sub = $1", &[&user_sub])
        .await
        .map_err(|e| ApiError::Internal(format!("delete_me: {e}")))?;
    Ok(())
}
