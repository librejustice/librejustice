//! Redirection trailing-slash (parité Starlette `redirect_slashes=True`).
//!
//! FastAPI/Starlette ne 404 pas sur une variante de slash : si le chemin demandé
//! ne matche aucune route mais que le chemin avec le slash final basculé
//! (ajouté/retiré) matche, Starlette renvoie un **307** vers le chemin canonique
//! (méthode + corps préservés). axum exige un match exact et 404. On reproduit le
//! 307 via le fallback du routeur racine.
//!
//! Le fallback ne se déclenche QUE sur un miss de routage (aucune route ne matche
//! le chemin) — exactement la condition de Starlette. Un 404 *retourné par un
//! handler* (ex. décision absente) ne passe pas par ici.
//!
//! Détection sans introspection du routeur (non exposée par axum) : table des
//! gabarits servis, tenue en miroir de `create_app` / `oauth` / `mcp`. Match au
//! niveau du chemin uniquement ; le cas-limite méthode (Starlette n'exige le 307
//! que sur un match FULL chemin+méthode) n'est pas reproduit — il ne survient que
//! sur une combinaison mauvaise-méthode + mauvais-slash, hors de tout usage réel.

use axum::extract::OriginalUri;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

/// Gabarits de tous les chemins servis (segments littéraux + `{param}`
/// mono-segment). Miroir des routes montées dans [`crate::routes::create_app`],
/// [`crate::oauth`] et [`crate::mcp`] — toute nouvelle route doit y être ajoutée
/// pour bénéficier de la redirection de slash.
const SERVED_TEMPLATES: &[&str] = &[
    // /api (nest)
    "/api/health",
    "/api/search",
    "/api/search-textes",
    "/api/decision/{p}",
    "/api/decision/{p}/similar",
    "/api/decision/{p}/download.docx",
    "/api/decision/{p}/download.pdf",
    "/api/me",
    "/api/me/activity-tracking",
    "/api/me/bookmarks",
    "/api/me/bookmarks/{p}",
    "/api/me/search-history",
    "/api/me/search-history/{p}",
    "/api/me/decision-views",
    "/api/me/decision-views/{p}",
    // oauth
    "/oauth/register",
    "/oauth/authorize",
    "/oauth/approve",
    "/oauth/token",
    // .well-known
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
    // mcp (servi avec et sans slash, cf. `mcp::mcp_router`)
    "/mcp/",
    "/mcp",
];

/// Un gabarit (`/a/{x}/b`) matche-t-il un chemin concret ? Même nombre de
/// segments ; `{...}` matche n'importe quel segment non vide, sinon égalité.
fn template_matches(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    if t.len() != p.len() {
        return false;
    }
    t.iter().zip(&p).all(|(ts, ps)| {
        if ts.starts_with('{') && ts.ends_with('}') {
            !ps.is_empty()
        } else {
            ts == ps
        }
    })
}

fn route_exists(path: &str) -> bool {
    SERVED_TEMPLATES.iter().any(|t| template_matches(t, path))
}

/// Bascule le slash final : `/x/` → `/x`, `/x` → `/x/`. `None` pour la racine.
fn toggle_trailing_slash(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    match path.strip_suffix('/') {
        Some(stripped) => Some(stripped.to_string()),
        None => Some(format!("{path}/")),
    }
}

/// Fallback racine : 307 vers la variante de slash si elle matche une route,
/// sinon 404 (parité Starlette `redirect_slashes`). La query string est
/// préservée dans la `Location`.
pub async fn slash_redirect_fallback(OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    if let Some(alt) = toggle_trailing_slash(path) {
        if route_exists(&alt) {
            let target = match uri.query() {
                Some(q) => format!("{alt}?{q}"),
                None => alt,
            };
            return Redirect::temporary(&target).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_param_matches_any_single_segment() {
        assert!(template_matches(
            "/api/decision/{p}",
            "/api/decision/abc123"
        ));
        assert!(!template_matches(
            "/api/decision/{p}",
            "/api/decision/abc/extra"
        ));
        assert!(!template_matches("/api/decision/{p}", "/api/decision/"));
    }

    #[test]
    fn route_exists_covers_health_and_mcp() {
        assert!(route_exists("/api/health"));
        assert!(route_exists("/mcp/"));
        assert!(route_exists("/mcp"));
        assert!(route_exists("/api/me/bookmarks/xyz"));
        assert!(!route_exists("/api/health/"));
    }

    #[test]
    fn toggle_adds_and_strips_one_slash() {
        assert_eq!(
            toggle_trailing_slash("/api/health/").as_deref(),
            Some("/api/health")
        );
        assert_eq!(toggle_trailing_slash("/mcp").as_deref(), Some("/mcp/"));
        assert_eq!(toggle_trailing_slash("/"), None);
    }
}
