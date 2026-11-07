//! Configuration runtime du serveur API.
//!
//! Settings centralisé, prefix `LIBREJUSTICE_API_` (+ secrets/DB partagés sous
//! `LIBREJUSTICE_`) : tout est lu ici dans [`Settings::from_env`] et injecté
//! dans l'état axum.

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
    /// The embedding chain in priority order, never empty.
    pub embed_chain: Vec<lj_llm::hosts::Entry>,
    pub embed_connect_timeout: f64,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_backend_token: Option<String>,
    // Divers
    pub leg_limit: u32,
    pub cors_origins: Vec<String>,
    pub mcp_require_auth: bool,
    pub mcp_allowed_hosts: Vec<String>,
    /// Token de vérification de propriété du domaine pour le catalogue d'apps
    /// ChatGPT (servi en `text/plain` sur `/.well-known/openai-apps-challenge`).
    /// Valeur publique. Absent → 404.
    pub openai_apps_challenge_token: Option<String>,
    pub internal_origin_token: Option<String>,
    pub usage_pseudonym_key: Option<String>,
    /// Mainteneur déclaré pour le claim du connecteur sur Glama (servi en JSON
    /// sur `/.well-known/glama.json` : `{"maintainers": [<valeur>]}` — email du
    /// compte Glama ou login GitHub). Valeur publique. Absent → 404.
    pub glama_maintainer: Option<String>,
    /// P-256 private key signing the OIDC `id_token`, PKCS#8 DER in one line of
    /// base64. Absent: no `id_token` is issued, `/.well-known/jwks.json` is a
    /// 404, and discovery advertises `none` as its signing algorithm.
    pub oidc_signing_key: Option<String>,
    /// Clé IndexNow, partagée avec le cron `lj-ingest indexnow` : le protocole
    /// exige qu'elle soit servie à `https://<host>/<clé>.txt`. Publique par
    /// protocole. Absent → pas de route.
    pub indexnow_key: Option<String>,
    pub gunicorn_workers: usize,
    pub version: String,
    pub grafana_otlp_endpoint: Option<String>,
    pub grafana_otlp_user: Option<String>,
    pub grafana_cloud_api_key: Option<String>,
    pub otel_service_name: String,
    pub deployment_environment: Option<String>,
    pub host_name: Option<String>,
    pub container_name: Option<String>,
    pub db_application_name: Option<String>,
    pub slow_request_ms: u64,
    pub web_base_url: String,
    pub public_base_url: String,
    /// Clé d'envoi Resend (envoi seul, restreinte au domaine) et expéditeur des
    /// mails d'auth. Vides = émission désactivée, l'inscription refuse.
    pub resend_api_key: String,
    pub mail_from: String,
    /// Relais SMTP par lequel part un signalement. Adresse ou identifiants
    /// vides = signalement désactivé.
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    /// Boîte qui reçoit les signalements.
    pub support_mailbox: String,
    /// Signalements émis dans la journée, toutes origines confondues. Le relais
    /// borne ce qu'il accepte et se ferme au-delà : un seau par appelant ne
    /// protège que du répéteur, celui-ci protège d'une foule.
    pub signalements_par_jour: u32,
    /// Client OAuth du tour de connexion Google. Vides = tour désactivé,
    /// `/api/auth/google/start` répond 503.
    pub google_client_id: String,
    pub google_client_secret: String,
    /// Client OAuth du tour de connexion Microsoft, multi-tenant (`/common`).
    /// Vides = tour désactivé, `/api/auth/microsoft/start` répond 503.
    pub microsoft_client_id: String,
    pub microsoft_client_secret: String,
    /// Dépenses de recherche par appelant et par minute (embedding + récupération
    /// hybride), comptées sur MISS du `search_cache` uniquement.
    pub search_budget_per_min: u32,
    /// Le budget de recherche refuse-t-il, ou se contente-t-il de compter ?
    pub search_budget_enforce: bool,
    /// Reranks Mistral par appelant et par minute, comptés sur MISS du
    /// `rerank_cache`. Le dépassement n'est jamais un refus : la recherche rend
    /// l'ordre RRF (`crate::search::reranked_order`).
    pub rerank_budget_per_min: u32,
    /// The INPI account the RNE API logs in with. Empty: no company documents.
    pub inpi_username: String,
    pub inpi_password: String,
    /// The bucket the INPI files are kept in. `None` when `LIBREJUSTICE_S3_ENDPOINT` is unset.
    pub bucket: Option<lj_bucket::Config>,
}

/// Lit `var`, renvoie `None` si absent ou chaîne vide (env vide = « non
/// fourni »).
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

/// Parse un entier avec borne `[lo, hi]`. Erreur franche si hors borne.
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

/// Parse un booléen (`true/1/yes/on` insensible à la casse).
fn parse_bool(var: &str, default: bool) -> bool {
    match opt(var) {
        None => default,
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
    }
}

fn environment_is_first_party(env: Option<&str>) -> bool {
    matches!(env, Some("beta" | "dev" | "staging"))
}

/// Parse une liste séparée par des virgules, items vides éliminés.
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

fn bucket_config() -> Result<Option<lj_bucket::Config>> {
    let Some(endpoint) = opt("LIBREJUSTICE_S3_ENDPOINT") else {
        return Ok(None);
    };
    let required = |key: &str| {
        opt(key)
            .ok_or_else(|| anyhow::anyhow!("{key} is unset while LIBREJUSTICE_S3_ENDPOINT is set"))
    };
    Ok(Some(lj_bucket::Config {
        endpoint,
        region: required("LIBREJUSTICE_S3_REGION")?,
        bucket: required("LIBREJUSTICE_S3_BUCKET")?,
        access_key_id: required("LIBREJUSTICE_S3_ACCESS_KEY_ID")?,
        secret_access_key: required("LIBREJUSTICE_S3_SECRET_ACCESS_KEY")?,
    }))
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
        // Le sémaphore de recherche (`AppState.search_permits`) borne les recherches
        // concurrentes à `pool_max / PEAK_CONNS_PER_SEARCH`, chacune tenant jusqu'à
        // 3 connexions en `try_join` ; `pool_max` ne règle que le plafond de
        // concurrence DB.
        let pool_max = parse_ranged::<usize>("LIBREJUSTICE_API_POOL_MAX", 32, 1, 128)?;
        let vchord_probes = parse_ranged::<u32>("LIBREJUSTICE_API_VCHORD_PROBES", 20, 1, 512)?;

        let embed_backend = or_default("LIBREJUSTICE_EMBED_BACKEND", "auto");
        let embed_chain = lj_llm::hosts::parse_chain(
            opt("LIBREJUSTICE_EMBED_URL")
                .as_deref()
                .unwrap_or(lj_llm::hosts::DEFAULT_URL),
            opt("LIBREJUSTICE_EMBED_API_KEY").as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
        let embed_connect_timeout =
            parse_ranged::<f64>("LIBREJUSTICE_EMBED_CONNECT_TIMEOUT", 0.2, 0.005, 5.0)?;
        let cloudflare_account_id = opt("LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID");
        let cloudflare_backend_token = opt("LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN");

        let leg_limit = parse_ranged::<u32>("LIBREJUSTICE_API_LEG_LIMIT", 200, 10, 1000)?;

        // CORS : liste explicite, sinon dérivée de web_base_url.
        let cors_origins = parse_csv("LIBREJUSTICE_API_CORS_ORIGINS");

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
        let internal_origin_token = opt("LIBREJUSTICE_API_INTERNAL_ORIGIN_TOKEN");
        let usage_pseudonym_key = opt("LIBREJUSTICE_API_USAGE_PSEUDONYM_KEY");
        let glama_maintainer = opt("LIBREJUSTICE_API_GLAMA_MAINTAINER");
        let oidc_signing_key = opt("LIBREJUSTICE_API_OIDC_SIGNING_KEY");
        let indexnow_key = opt("LIBREJUSTICE_INDEXNOW_KEY");

        let gunicorn_workers =
            parse_ranged::<usize>("LIBREJUSTICE_API_GUNICORN_WORKERS", 4, 1, 32)?;

        let grafana_otlp_endpoint = opt("LIBREJUSTICE_GRAFANA_OTLP_ENDPOINT");
        let grafana_otlp_user = opt("LIBREJUSTICE_GRAFANA_OTLP_USER");
        let grafana_cloud_api_key = opt("LIBREJUSTICE_GRAFANA_CLOUD_API_KEY");
        let otel_service_name =
            or_default("LIBREJUSTICE_API_OTEL_SERVICE_NAME", "librejustice-api");
        let deployment_environment = opt("LIBREJUSTICE_API_DEPLOYMENT_ENVIRONMENT");
        // Only the deploy knows the shipped commit, so prod refuses to start
        // rather than answer "dev" to whoever asks what is running.
        let version = match (
            opt("LIBREJUSTICE_API_VERSION"),
            deployment_environment.as_deref(),
        ) {
            (Some(v), _) => v,
            (None, Some("prod")) => {
                anyhow::bail!("LIBREJUSTICE_API_VERSION absente : le déploiement doit l'exporter")
            }
            (None, _) => "dev".to_string(),
        };
        // Même nom pour les deux binaires : c'est un fait d'environnement du
        // processus, pas un réglage de l'API. Absents sur l'hôte, où le hostname
        // du noyau suffit et où il n'y a pas de conteneur à nommer.
        let host_name = opt("LIBREJUSTICE_HOST_NAME");
        let container_name = opt("LIBREJUSTICE_CONTAINER_NAME");
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

        let resend_api_key = or_default("LIBREJUSTICE_API_RESEND_API_KEY", "");
        let mail_from = or_default(
            "LIBREJUSTICE_API_MAIL_FROM",
            "LibreJustice <noreply@librejustice.fr>",
        );

        let smtp_host = or_default("LIBREJUSTICE_SMTP_HOST", "");
        let smtp_port = parse_ranged("LIBREJUSTICE_SMTP_PORT", 587, 1, 65_535)? as u16;
        let smtp_user = or_default("LIBREJUSTICE_SMTP_USER", "");
        let smtp_password = or_default("LIBREJUSTICE_SMTP_PASSWORD", "");
        let support_mailbox = or_default(
            "LIBREJUSTICE_API_SUPPORT_MAILBOX",
            "contact@librejustice.fr",
        );
        let signalements_par_jour =
            parse_ranged("LIBREJUSTICE_API_SIGNALEMENTS_PAR_JOUR", 200, 1, 100_000)?;

        let google_client_id = or_default("LIBREJUSTICE_GOOGLE_OAUTH_CLIENT_ID", "");
        let google_client_secret = or_default("LIBREJUSTICE_GOOGLE_OAUTH_CLIENT_SECRET", "");
        let microsoft_client_id = or_default("LIBREJUSTICE_MICROSOFT_OAUTH_CLIENT_ID", "");
        let microsoft_client_secret = or_default("LIBREJUSTICE_MICROSOFT_OAUTH_CLIENT_SECRET", "");

        // 30/min: over seven days the busiest of 392 callers peaked at 23 searches
        // in a sliding minute, and none went above 25.
        let search_budget_per_min =
            parse_ranged("LIBREJUSTICE_API_SEARCH_BUDGET_PER_MIN", 30, 1, 10_000)?;
        let search_budget_enforce = parse_bool("LIBREJUSTICE_API_SEARCH_BUDGET_ENFORCE", true);
        let rerank_budget_per_min =
            parse_ranged("LIBREJUSTICE_API_RERANK_BUDGET_PER_MIN", 20, 1, 10_000)?;

        let inpi_username = or_default("LIBREJUSTICE_INPI_USERNAME", "");
        let inpi_password = or_default("LIBREJUSTICE_INPI_PASSWORD", "");
        let bucket = bucket_config()?;

        let mut settings = Settings {
            pg_password,
            db_url,
            bind_host,
            bind_port,
            pool_min,
            pool_max,
            vchord_probes,
            embed_backend,
            embed_chain,
            embed_connect_timeout,
            cloudflare_account_id,
            cloudflare_backend_token,
            leg_limit,
            cors_origins,
            mcp_require_auth,
            mcp_allowed_hosts,
            openai_apps_challenge_token,
            internal_origin_token,
            usage_pseudonym_key,
            glama_maintainer,
            oidc_signing_key,
            indexnow_key,
            gunicorn_workers,
            version,
            grafana_otlp_endpoint,
            grafana_otlp_user,
            grafana_cloud_api_key,
            otel_service_name,
            deployment_environment,
            host_name,
            container_name,
            db_application_name,
            slow_request_ms,
            web_base_url,
            public_base_url,
            resend_api_key,
            mail_from,
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_password,
            support_mailbox,
            signalements_par_jour,
            google_client_id,
            google_client_secret,
            microsoft_client_id,
            microsoft_client_secret,
            search_budget_per_min,
            search_budget_enforce,
            rerank_budget_per_min,
            inpi_username,
            inpi_password,
            bucket,
        };

        settings.inject_pg_password();
        settings.default_cors_origins();
        Ok(settings)
    }

    /// Injecte `pg_password` dans `db_url` si l'URL n'a pas déjà de mot de passe.
    fn inject_pg_password(&mut self) {
        if let Some(pw) = self.pg_password.as_deref() {
            if let Some(injected) = inject_password(&self.db_url, pw) {
                self.db_url = injected;
            }
        }
    }

    pub fn scrapes_shared_resources(&self) -> bool {
        self.grafana_otlp_endpoint.is_some()
            && self.grafana_otlp_user.is_some()
            && self.grafana_cloud_api_key.is_some()
            && self.deployment_environment.as_deref() == Some("prod")
    }

    pub fn traffic_is_first_party(&self) -> bool {
        environment_is_first_party(self.deployment_environment.as_deref())
    }

    /// `cors_origins` vide → dérivé de `web_base_url` + dev local.
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
/// section userinfo avant le `@`). Le mot de passe est inséré tel quel (pas
/// d'URL-encoding).
fn inject_password(url: &str, password: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let (scheme, rest) = url.split_at(scheme_end + 3);
    // `rest` = `[userinfo@]host[:port][/path...]`.
    let netloc_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (netloc, tail) = rest.split_at(netloc_end);

    let (userinfo, hostport) = match netloc.rfind('@') {
        Some(at) => (&netloc[..at], &netloc[at + 1..]),
        None => ("", netloc),
    };

    if userinfo.contains(':') {
        return None;
    }

    // username peut être vide → `:password@host`.
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
        let got = inject_password("postgresql://app@db.example:5432/app", "s3cr3t");
        assert_eq!(
            got.as_deref(),
            Some("postgresql://app:s3cr3t@db.example:5432/app")
        );
    }

    #[test]
    fn inject_pg_password_skips_when_present() {
        assert_eq!(
            inject_password("postgresql://app:already@db.example:5432/app", "s3cr3t",),
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
        assert!(parse_ranged::<u16>("LIBREJUSTICE_TEST_PORT", 8080, 1, 65535).is_err());
        std::env::remove_var("LIBREJUSTICE_TEST_PORT");
    }

    #[test]
    fn only_named_environments_are_first_party() {
        for env in ["beta", "dev", "staging"] {
            assert!(environment_is_first_party(Some(env)), "{env}");
        }
        for env in [None, Some("prod"), Some("preprod"), Some("")] {
            assert!(!environment_is_first_party(env), "{env:?}");
        }
    }
}
