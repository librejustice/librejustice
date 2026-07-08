//! Headers `Cache-Control` (port de `cache.py`).
//!
//! Côté Python, `public_cache(s_maxage, max_age)` est une dépendance FastAPI qui
//! pose le header sur la réponse. Côté axum, on expose la même valeur de header
//! sous forme de constantes/helper, à appliquer par les handlers (ou un layer)
//! qui possèdent les routes search/decision.

/// Type d'un TTL de cache : valeurs `s-maxage` (CDN) et `max-age` (navigateur).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    pub s_maxage: u32,
    pub max_age: u32,
}

impl CachePolicy {
    pub const fn new(s_maxage: u32, max_age: u32) -> Self {
        Self { s_maxage, max_age }
    }

    /// Rend la valeur du header `Cache-Control` (parité octet avec
    /// `public, s-maxage=…, max-age=…`).
    pub fn header_value(&self) -> String {
        format!(
            "public, s-maxage={}, max-age={}",
            self.s_maxage, self.max_age
        )
    }
}

/// Recherche : résultats stables 1 h côté CDN, 5 min côté navigateur.
pub const CACHE_SEARCH: CachePolicy = CachePolicy::new(3600, 300);
/// Décision : contenu immuable après ingestion, 24 h CDN / 1 h navigateur.
pub const CACHE_DECISION: CachePolicy = CachePolicy::new(86400, 3600);
/// Sitemaps : régénérés par le cron (hebdo), 1 h CDN / 1 h navigateur. Le CDN
/// Cloudflare absorbe les hits des crawlers → Postgres touché rarement.
pub const CACHE_SITEMAP: CachePolicy = CachePolicy::new(3600, 3600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_value_matches_python() {
        // Parité octet avec `public_cache` côté FastAPI.
        assert_eq!(
            CACHE_SEARCH.header_value(),
            "public, s-maxage=3600, max-age=300"
        );
        assert_eq!(
            CACHE_DECISION.header_value(),
            "public, s-maxage=86400, max-age=3600"
        );
    }
}
