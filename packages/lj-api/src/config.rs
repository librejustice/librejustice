//! Configuration runtime du serveur API (port de `apps/api/.../config.py`).
//!
//! Settings centralisé, prefix `LIBREJUSTICE_API_` (+ secrets/DB partagés sous
//! `LIBREJUSTICE_`). Pas de `std::env::var` dispersé : tout est lu ici dans
//! [`Settings::from_env`] et injecté dans l'état axum.

use anyhow::{Context, Result};
use std::env;

/// Paramètres runtime du serveur API mono-serveur.
#[derive(Debug, Clone)]
pub struct Settings {
    // DB partagée (LIBREJUSTICE_*)
    pub pg_password: Option<String>,
    pub db_url: String,
    // Réseau (LIBREJUSTICE_API_*)
    pub bind_host: String,
    pub bind_port: u16,
    pub pool_min: usize,
    pub pool_max: usize,
    pub vchord_probes: u32,
    // Embedding
    pub embed_backend: String,
    pub embed_url: Option<String>,
    pub embed_api_key: Option<String>,
    pub embed_connect_timeout: f64,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_backend_token: Option<String>,
    // Divers
    pub leg_limit: u32,
    pub cors_origins: Vec<String>,
    pub supabase_url: Option<String>,
    pub supabase_secret_key: Option<String>,
    pub mcp_require_auth: bool,
    pub mcp_allowed_hosts: Vec<String>,
    /// Token de vérification de propriété du domaine pour le catalogue d'apps
    /// ChatGPT (servi en `text/plain` sur `/.well-known/openai-apps-challenge`).
    /// Valeur publique (non secrète) mais propre au déploiement. Absent → 404.
    pub openai_apps_challenge_token: Option<String>,
    /// Clé IndexNow (ADR 0044), partagée avec le cron `lj-ingest indexnow` :
    /// le protocole exige qu'elle soit servie à `https://<host>/<clé>.txt`,
    /// sinon les soumissions sont rejetées. Publique par protocole. Absent →
    /// pas de route.
    pub indexnow_key: Option<String>,
    pub gunicorn_workers: usize,
    pub version: String,
    // Export OTel direct vers la gateway Grafana Cloud (plus de collector local).
    pub grafana_otlp_endpoint: Option<String>,
    pub grafana_otlp_user: Option<String>,
    pub grafana_cloud_api_key: Option<String>,
    pub otel_service_name: String,
    pub deployment_environment: Option<String>,
    pub db_application_name: Option<String>,
    pub slow_request_ms: u64,
    pub web_base_url: String,
    pub public_base_url: String,
    pub mistral_api_keys: Vec<String>,
    pub mistral_model: String,
    /// Chemin du certificat Cloudflare Origin CA pour le TLS rustls in-process
    /// (ADR 0061). `None` → HTTP en clair.
    pub tls_cert_path: Option<String>,
    /// Chemin de la clé privée associée au certificat Origin CA. `None` → HTTP
    /// en clair.
    pub tls_key_path: Option<String>,
}

/// Lit `var`, renvoie `None` si absent ou chaîne vide (parité avec Pydantic qui
/// traite l'env vide comme « non fourni » pour les champs `Option`).
fn opt(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Lit `var` avec valeur par défaut.
fn or_default(var: &str, default: &str) -> String {
    opt(var).unwrap_or_else(|| default.to_string())
}

/// Parse un entier avec borne `[lo, hi]`. Erreur franche si hors borne (parité
/// avec les contraintes Pydantic `ge`/`le`).
fn parse_ranged<T>(var: &str, default: T, lo: T, hi: T) -> Result<T>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    match opt(var) {
        None => Ok(default),
        Some(raw) => {
            let v: T = raw
                .parse()
                .map_err(|e| anyhow::anyhow!("{var}: valeur invalide {raw:?}: {e}"))?;
            if v < lo || v > hi {
                anyhow::bail!("{var}={v} hors borne [{lo}, {hi}]");
            }
            Ok(v)
        }
    }
}

/// Parse un booléen façon Pydantic (`true/1/yes/on` insensible à la casse).
fn parse_bool(var: &str, default: bool) -> bool {
    match opt(var) {
        None => default,
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
    }
}

/// Parse une liste séparée par des virgules, items vides éliminés (parité avec
/// `_split_mistral_keys`).
fn parse_csv(var: &str) -> Vec<String> {
    match opt(var) {
        None => Vec::new(),
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

impl Settings {
    /// Lit l'environnement (prefix `LIBREJUSTICE_API_` / `LIBREJUSTICE_`) et
    /// valide (injecte `pg_password` dans `db_url`, etc.).
    pub fn from_env() -> Result<Self> {
        let pg_password = opt("LIBREJUSTICE_PG_PASSWORD");
        let db_url = or_default(
            "LIBREJUSTICE_DB_URL",
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
        );

        let bind_host = or_default("LIBREJUSTICE_API_BIND_HOST", "127.0.0.1");
        let bind_port = parse_ranged::<u16>("LIBREJUSTICE_API_BIND_PORT", 8300, 1, 65535)?;
        let pool_min = parse_ranged::<usize>("LIBREJUSTICE_API_POOL_MIN", 2, 1, 64)?;
        // Défaut 32 : le sémaphore de recherche (cf. `AppState.search_permits`)
        // borne les recherches concurrentes à `pool_max / PEAK_CONNS_PER_SEARCH`,
        // soit ~10 ici (chacune tenant jusqu'à 3 connexions en `try_join`). C'est le
        // sémaphore — pas la taille du pool — qui rend le pool deadlock-safe (toute
        // recherche admise peut acquérir son pic de conns) ; `pool_max` ne règle
        // donc que le plafond de concurrence DB.
        let pool_max = parse_ranged::<usize>("LIBREJUSTICE_API_POOL_MAX", 32, 1, 128)?;
        let vchord_probes = parse_ranged::<u32>("LIBREJUSTICE_API_VCHORD_PROBES", 20, 1, 512)?;

        let embed_backend = or_default("LIBREJUSTICE_EMBED_BACKEND", "auto");
        let embed_url = opt("LIBREJUSTICE_EMBED_URL");
        let embed_api_key = opt("LIBREJUSTICE_EMBED_API_KEY");
        let embed_connect_timeout =
            parse_ranged::<f64>("LIBREJUSTICE_EMBED_CONNECT_TIMEOUT", 0.2, 0.005, 5.0)?;
        let cloudflare_account_id = opt("LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID");
        let cloudflare_backend_token = opt("LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN");

        let leg_limit = parse_ranged::<u32>("LIBREJUSTICE_API_LEG_LIMIT", 200, 10, 1000)?;

        // CORS : liste explicite, sinon dérivée de web_base_url (cf. validator Python).
        let cors_origins = parse_csv("LIBREJUSTICE_API_CORS_ORIGINS");

        let supabase_url = opt("LIBREJUSTICE_VITE_SUPABASE_URL");
        let supabase_secret_key = opt("LIBREJUSTICE_API_SUPABASE_SECRET_KEY");

        let mcp_require_auth = parse_bool("LIBREJUSTICE_API_MCP_REQUIRE_AUTH", false);
        let mcp_allowed_hosts = match opt("LIBREJUSTICE_API_MCP_ALLOWED_HOSTS") {
            Some(_) => parse_csv("LIBREJUSTICE_API_MCP_ALLOWED_HOSTS"),
            None => vec![
                "localhost".to_string(),
                "localhost:*".to_string(),
                "127.0.0.1".to_string(),
                "127.0.0.1:*".to_string(),
                "test".to_string(),
                "librejustice.fr".to_string(),
            ],
        };

        let openai_apps_challenge_token = opt("LIBREJUSTICE_API_OPENAI_APPS_CHALLENGE_TOKEN");
        let indexnow_key = opt("LIBREJUSTICE_INDEXNOW_KEY");

        let gunicorn_workers =
            parse_ranged::<usize>("LIBREJUSTICE_API_GUNICORN_WORKERS", 4, 1, 32)?;
        let version = or_default("LIBREJUSTICE_API_VERSION", "dev");

        // Credentials gateway OTLP Grafana Cloud (regle #5 : via Settings,
        // prefixe LIBREJUSTICE_). L'export OTel s'active si les trois sont la.
        let grafana_otlp_endpoint = opt("LIBREJUSTICE_GRAFANA_OTLP_ENDPOINT");
        let grafana_otlp_user = opt("LIBREJUSTICE_GRAFANA_OTLP_USER");
        let grafana_cloud_api_key = opt("LIBREJUSTICE_GRAFANA_CLOUD_API_KEY");
        let otel_service_name =
            or_default("LIBREJUSTICE_API_OTEL_SERVICE_NAME", "librejustice-api");
        let deployment_environment = opt("LIBREJUSTICE_API_DEPLOYMENT_ENVIRONMENT");
        let db_application_name = opt("LIBREJUSTICE_API_DB_APPLICATION_NAME");

        let slow_request_ms = match opt("LIBREJUSTICE_API_SLOW_REQUEST_MS") {
            None => 200,
            Some(raw) => {
                let v: u64 = raw.parse().with_context(|| {
                    format!("LIBREJUSTICE_API_SLOW_REQUEST_MS invalide: {raw:?}")
                })?;
                v
            }
        };

        let web_base_url = or_default("LIBREJUSTICE_API_WEB_BASE_URL", "https://librejustice.fr");
        let public_base_url = or_default(
            "LIBREJUSTICE_API_PUBLIC_BASE_URL",
            "https://librejustice.fr",
        );

        let mistral_api_keys = parse_csv("LIBREJUSTICE_MISTRAL_API_KEYS");
        let mistral_model = or_default("LIBREJUSTICE_MISTRAL_MODEL", "mistral-small-2506");

        let tls_cert_path = opt("LIBREJUSTICE_API_TLS_CERT_PATH");
        let tls_key_path = opt("LIBREJUSTICE_API_TLS_KEY_PATH");

        let mut settings = Settings {
            pg_password,
            db_url,
            bind_host,
            bind_port,
            pool_min,
            pool_max,
            vchord_probes,
            embed_backend,
            embed_url,
            embed_api_key,
            embed_connect_timeout,
            cloudflare_account_id,
            cloudflare_backend_token,
            leg_limit,
            cors_origins,
            supabase_url,
            supabase_secret_key,
            mcp_require_auth,
            mcp_allowed_hosts,
            openai_apps_challenge_token,
            indexnow_key,
            gunicorn_workers,
            version,
            grafana_otlp_endpoint,
            grafana_otlp_user,
            grafana_cloud_api_key,
            otel_service_name,
            deployment_environment,
            db_application_name,
            slow_request_ms,
            web_base_url,
            public_base_url,
            mistral_api_keys,
            mistral_model,
            tls_cert_path,
            tls_key_path,
        };

        settings.inject_pg_password();
        settings.default_cors_origins();
        Ok(settings)
    }

    /// Injecte `pg_password` dans `db_url` si l'URL n'a pas déjà de mot de passe
    /// (parité avec `_inject_pg_password`).
    fn inject_pg_password(&mut self) {
        if let Some(pw) = self.pg_password.as_deref() {
            if let Some(injected) = inject_password(&self.db_url, pw) {
                self.db_url = injected;
            }
        }
    }

    /// `cors_origins` vide → dérivé de `web_base_url` + dev local (parité avec
    /// `_default_cors_origins`).
    fn default_cors_origins(&mut self) {
        if self.cors_origins.is_empty() {
            self.cors_origins = vec![
                self.web_base_url.trim_end_matches('/').to_string(),
                "http://localhost:5174".to_string(),
                "http://127.0.0.1:5174".to_string(),
            ];
        }
    }
}

/// Injecte un mot de passe dans le composant `netloc` d'une URL `scheme://user@host[:port]/...`.
///
/// Retourne `None` si l'URL a déjà un mot de passe (présence d'un `:` dans la
/// section userinfo avant le `@`) — parité avec `if not parsed.password`. Le mot
/// de passe est inséré tel quel (pas d'URL-encoding ; identique à `urlunparse`
/// côté Python qui ne ré-encode pas un password déjà fourni en clair).
fn inject_password(url: &str, password: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let (scheme, rest) = url.split_at(scheme_end + 3);
    // `rest` = `[userinfo@]host[:port][/path...]`. On isole netloc = avant le
    // premier `/`, `?` ou `#`.
    let netloc_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (netloc, tail) = rest.split_at(netloc_end);

    let (userinfo, hostport) = match netloc.rfind('@') {
        Some(at) => (&netloc[..at], &netloc[at + 1..]),
        None => ("", netloc),
    };

    // Déjà un password ? userinfo contient un `:`.
    if userinfo.contains(':') {
        return None;
    }

    // username peut être vide (cas redis sans user) → `:password@host`.
    let new_netloc = if userinfo.is_empty() {
        format!(":{password}@{hostport}")
    } else {
        format!("{userinfo}:{password}@{hostport}")
    };
    Some(format!("{scheme}{new_netloc}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_pg_password_no_existing() {
        let got = inject_password(
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
            "s3cr3t",
        );
        assert_eq!(
            got.as_deref(),
            Some("postgresql://librejustice:s3cr3t@127.0.0.1:5432/librejustice")
        );
    }

    #[test]
    fn inject_pg_password_skips_when_present() {
        // L'URL a déjà un password → on ne touche pas.
        assert_eq!(
            inject_password(
                "postgresql://librejustice:already@127.0.0.1:5432/librejustice",
                "s3cr3t",
            ),
            None
        );
    }

    #[test]
    fn parse_csv_trims_and_drops_empty() {
        std::env::set_var("LIBREJUSTICE_TEST_CSV", " a , ,b,  c  ");
        assert_eq!(
            parse_csv("LIBREJUSTICE_TEST_CSV"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        std::env::remove_var("LIBREJUSTICE_TEST_CSV");
    }

    #[test]
    fn parse_ranged_rejects_out_of_bounds() {
        std::env::set_var("LIBREJUSTICE_TEST_PORT", "70000");
        assert!(parse_ranged::<u16>("LIBREJUSTICE_TEST_PORT", 8300, 1, 65535).is_err());
        std::env::remove_var("LIBREJUSTICE_TEST_PORT");
    }
}
