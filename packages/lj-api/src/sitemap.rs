//! Service HTTP des sitemaps depuis Postgres (ADR 0064).
//!
//! Remplace le Worker Cloudflare + bucket R2 : le cron (`lj-ingest sitemap`)
//! upsert la table `sitemaps`, ces routes la servent. Le CDN Cloudflare cache
//! les réponses (`Cache-Control` 1 h) → Postgres touché rarement.
//!
//! - `GET /sitemap.xml` → ligne `sitemap-index.xml` (entrée robots/Google) ;
//! - `GET /sitemaps/{file}` → ligne `{file}` (ex. `sitemap-1.xml.gz`). Le nom
//!   complet est UN segment de chemin : matchit/axum 0.8 n'autorise pas de
//!   capture préfixe+suffixe dans un segment (`/sitemap-{n}.xml.gz` est refusé),
//!   d'où le préfixe `/sitemaps/` qui isole la capture sans shadower les routes
//!   Leptos.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::cache::CACHE_SITEMAP;
use crate::error::{ApiError, Result};
use crate::state::AppState;
use lj_store::repository::DecisionRepository;

/// Routes sitemap, montées à la racine (hors `/api`) par `assemble_routes`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sitemap.xml", get(sitemap_index))
        .route("/sitemaps/{file}", get(sitemap_sub))
}

/// `/sitemap.xml` → ligne `sitemap-index.xml`.
async fn sitemap_index(State(state): State<AppState>) -> Result<Response> {
    serve(&state, "sitemap-index.xml").await
}

/// `/sitemaps/{file}` → ligne `{file}` (nom de fichier complet, ex. `sitemap-1.xml.gz`).
async fn sitemap_sub(State(state): State<AppState>, Path(file): Path<String>) -> Result<Response> {
    serve(&state, &file).await
}

/// Lit la ligne `filename` et la sert avec son `Content-Type` + `Cache-Control` ;
/// 404 si absente.
async fn serve(state: &AppState, filename: &str) -> Result<Response> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let Some((body, content_type)) = DecisionRepository::new(&conn)
        .fetch_sitemap(filename)
        .await?
    else {
        return Err(ApiError::NotFound);
    };

    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&CACHE_SITEMAP.header_value()) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    Ok(resp)
}
