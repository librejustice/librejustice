//! Configuration ingest (port de `apps/ingest/librejustice/config.py`).
//!
//! Settings centralisé, prefix `LIBREJUSTICE_`. Pas de `std::env::var` dispersé
//! ailleurs dans le crate : tout passe par [`Settings::from_env`].

use std::path::PathBuf;

use anyhow::Result;

/// Lecture canonique de l'env, factorisée pour ne lire une variable qu'ici.
///
/// Le port Python s'appuie sur `pydantic-settings` (`env_prefix="LIBREJUSTICE_"`,
/// `extra="ignore"`). On reproduit la même sémantique à la main : variable
/// absente ou vide → `None`.
fn env_opt(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Variable string avec valeur par défaut (équivalent `Field(default=...)`).
fn env_or(key: &str, default: &str) -> String {
    env_opt(key).unwrap_or_else(|| default.to_string())
}

/// Découpe une liste séparée par des virgules en éléments non vides, trimés.
///
/// Port de `_split_gemini_keys` / `_split_mistral_keys` : `None`/vide → `[]`.
fn split_csv(value: Option<String>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|raw| {
            raw.split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Parse un budget neurons (`LIBREJUSTICE_INGEST_NEURON_BUDGET`).
///
/// Accepte un entier brut ou un suffixe d'échelle `k`/`m` (`"9k"` → 9000,
/// `"1.5m"` → 1_500_000), insensible à la casse. Vide/absent → `None`. Valeur
/// non parsable = erreur franche (une seule frontière de validation, règle #12).
fn parse_neuron_budget(raw: Option<String>) -> Result<Option<usize>> {
    let Some(raw) = raw else { return Ok(None) };
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Ok(None);
    }
    let (num, scale) = match raw.strip_suffix('k') {
        Some(n) => (n, 1_000.0),
        None => match raw.strip_suffix('m') {
            Some(n) => (n, 1_000_000.0),
            None => (raw.as_str(), 1.0),
        },
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("LIBREJUSTICE_INGEST_NEURON_BUDGET invalide : {raw:?}"))?;
    Ok(Some((value * scale) as usize))
}

/// Délai du watchdog en secondes (`LIBREJUSTICE_INGEST_WATCHDOG_SECS`).
///
/// Absent/vide → défaut 4 h (14400 s) : couvre `db vacuum-full-chunks` (lock
/// ~30-60 min) avec marge, tout en cassant un hang du même ordre que celui du
/// 2026-06-11 (chaîne pendue ~7 h jusqu'au reboot). `0` désactive le watchdog.
/// Valeur non parsable = erreur franche (une seule frontière de validation, #12).
fn parse_watchdog_secs(raw: Option<String>) -> Result<u64> {
    let Some(raw) = raw else { return Ok(14_400) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(14_400);
    }
    raw.parse()
        .map_err(|_| anyhow::anyhow!("LIBREJUSTICE_INGEST_WATCHDOG_SECS invalide : {raw:?}"))
}

/// Paramètres runtime du pipeline d'ingestion.
///
/// Source : variables d'environnement préfixées `LIBREJUSTICE_`, injectées par
/// le shell local (`mise`/`direnv`) ou par la CI. Un seul nom par secret, pas
/// d'alias de compatibilité — renommer à la source si besoin.
#[derive(Debug, Clone)]
pub struct Settings {
    pub state_dir: PathBuf,
    pub db_url: String,
    pub piste_client_id: Option<String>,
    pub piste_client_secret: Option<String>,
    pub embed_backend: String,
    pub embed_url: String,
    pub embed_api_key: Option<String>,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_backend_token: Option<String>,
    /// Budget neurons Workers AI par run (cap dur côté embedder Cloudflare).
    /// Accepte les suffixes `k`/`m` (ex. `"9k"` → 9000). `None` = pas de cap.
    pub ingest_neuron_budget: Option<usize>,
    pub mistral_api_keys: Vec<String>,
    /// Clés Mistral dédiées à l'API document (OCR `/v1/ocr`), séparées des clés
    /// chat (`mistral_api_keys`) : un usage OCR en rafale depuis l'IP datacenter
    /// fait flaguer les comptes, on isole donc le pool pour ne pas tuer les clés
    /// chat (résumés). Vide → l'OCR n'est pas disponible (erreur franche à l'appel).
    pub mistral_docapi_keys: Vec<String>,
    pub mistral_model: String,
    pub indexnow_key: Option<String>,
    pub grafana_otlp_endpoint: Option<String>,
    pub grafana_otlp_user: Option<String>,
    pub grafana_cloud_api_key: Option<String>,
    pub otel_service_name: String,
    pub deployment_environment: Option<String>,
    /// Plafond dur de durée d'une invocation `lj-ingest` (watchdog). Au-delà, le
    /// process logge une erreur et abandonne avec un code non nul, plutôt que de
    /// rester pendu indéfiniment (le cron `&&` casse alors proprement). Couvre la
    /// plus longue commande légitime (`db vacuum-full-chunks`, lock ~30-60 min)
    /// avec marge. `0` = watchdog désactivé.
    pub watchdog_secs: u64,
}

impl Settings {
    /// Lit l'environnement (prefix `LIBREJUSTICE_`) et valide.
    pub fn from_env() -> Result<Self> {
        let state_dir = env_opt("LIBREJUSTICE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_state_dir);

        let pg_password = env_opt("LIBREJUSTICE_PG_PASSWORD");
        let db_url_raw = env_or(
            "LIBREJUSTICE_DB_URL",
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
        );
        let db_url = inject_pg_password(&db_url_raw, pg_password.as_deref())?;

        Ok(Self {
            state_dir,
            db_url,
            piste_client_id: env_opt("LIBREJUSTICE_PISTE_CLIENT_ID"),
            piste_client_secret: env_opt("LIBREJUSTICE_PISTE_CLIENT_SECRET"),
            embed_backend: env_or("LIBREJUSTICE_EMBED_BACKEND", "auto"),
            embed_url: env_or(
                "LIBREJUSTICE_EMBED_URL",
                "http://127.0.0.1:8400/v1/embeddings",
            ),
            embed_api_key: env_opt("LIBREJUSTICE_EMBED_API_KEY"),
            cloudflare_account_id: env_opt("LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID"),
            cloudflare_backend_token: env_opt("LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN"),
            ingest_neuron_budget: parse_neuron_budget(env_opt(
                "LIBREJUSTICE_INGEST_NEURON_BUDGET",
            ))?,
            mistral_api_keys: split_csv(env_opt("LIBREJUSTICE_MISTRAL_API_KEYS")),
            mistral_docapi_keys: split_csv(env_opt("LIBREJUSTICE_MISTRAL_DOCAPI_KEYS")),
            mistral_model: env_or("LIBREJUSTICE_MISTRAL_MODEL", "mistral-small-2506"),
            indexnow_key: env_opt("LIBREJUSTICE_INDEXNOW_KEY"),
            grafana_otlp_endpoint: env_opt("LIBREJUSTICE_GRAFANA_OTLP_ENDPOINT"),
            grafana_otlp_user: env_opt("LIBREJUSTICE_GRAFANA_OTLP_USER"),
            grafana_cloud_api_key: env_opt("LIBREJUSTICE_GRAFANA_CLOUD_API_KEY"),
            otel_service_name: env_or("LIBREJUSTICE_OTEL_SERVICE_NAME", "librejustice-ingest"),
            deployment_environment: env_opt("LIBREJUSTICE_DEPLOYMENT_ENVIRONMENT"),
            watchdog_secs: parse_watchdog_secs(env_opt("LIBREJUSTICE_INGEST_WATCHDOG_SECS"))?,
        })
    }

    /// `state_dir / "ingest/cache"` — port du `@computed_field cache_dir`.
    pub fn cache_dir(&self) -> PathBuf {
        self.state_dir.join("ingest/cache")
    }

    /// `state_dir / "sources/legal-corpus"` — datasets curés génériques (métadonnées
    /// `legal_text` + chemin du markdown OCR), chargés par `load-legal-corpus`
    /// (ADR 0108). Donnée d'ingest produite par les scripts Python jettables ; vit
    /// dans `state_dir`, jamais en git ni en chemin relatif.
    pub fn legal_corpus_dir(&self) -> PathBuf {
        self.state_dir.join("sources/legal-corpus")
    }
}

/// `Path.home() / ".local/share/librejustice"`.
fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/share/librejustice")
}

/// Injecte `pg_password` dans `db_url` si l'URL n'en porte pas déjà un.
///
/// Port fidèle du `@model_validator _inject_pg_password` : on ne touche que si
/// `pg_password` est posé et que l'URL n'a pas déjà de mot de passe. La forme
/// reconstruite est `scheme://user:pwd@host[:port]/...` — identique à
/// `urlunparse(parsed._replace(netloc=...))` côté Python.
fn inject_pg_password(db_url: &str, pg_password: Option<&str>) -> Result<String> {
    let Some(password) = pg_password.filter(|p| !p.is_empty()) else {
        return Ok(db_url.to_string());
    };

    // Découpe scheme://netloc/reste — on n'a besoin que du netloc (autorité).
    let Some((scheme, after_scheme)) = db_url.split_once("://") else {
        // Pas une URL avec autorité : on laisse tel quel (comme Python qui
        // ne réécrit que si un hostname est présent).
        return Ok(db_url.to_string());
    };
    let (netloc, rest) = match after_scheme.find(['/', '?', '#']) {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, ""),
    };

    // userinfo@host:port — sépare l'éventuel userinfo de host:port.
    let (userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((info, hp)) => (Some(info), hp),
        None => (None, netloc),
    };

    // Si un mot de passe est déjà présent (`user:pwd`), ne rien faire.
    if userinfo.is_some_and(|info| info.contains(':')) {
        return Ok(db_url.to_string());
    }

    let username = userinfo.unwrap_or("");
    let new_netloc = format!("{username}:{password}@{hostport}");
    Ok(format!("{scheme}://{new_netloc}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : `_inject_pg_password` n'ajoute le mot de passe que si l'URL n'en
    // porte pas déjà, en préservant user/host/port/path (ADR 0014).
    #[test]
    fn inject_password_when_absent() {
        let url = inject_pg_password(
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
            Some("s3cret"),
        )
        .unwrap();
        assert_eq!(
            url,
            "postgresql://librejustice:s3cret@127.0.0.1:5432/librejustice"
        );
    }

    #[test]
    fn keep_existing_password() {
        let url = inject_pg_password(
            "postgresql://librejustice:already@127.0.0.1:5432/librejustice",
            Some("s3cret"),
        )
        .unwrap();
        assert_eq!(
            url,
            "postgresql://librejustice:already@127.0.0.1:5432/librejustice"
        );
    }

    #[test]
    fn no_password_no_change() {
        let url =
            inject_pg_password("postgresql://librejustice@127.0.0.1/librejustice", None).unwrap();
        assert_eq!(url, "postgresql://librejustice@127.0.0.1/librejustice");
    }

    #[test]
    fn inject_without_port() {
        let url =
            inject_pg_password("postgresql://lj@db.internal/librejustice", Some("pw")).unwrap();
        assert_eq!(url, "postgresql://lj:pw@db.internal/librejustice");
    }

    // Spec : les clés Gemini/Mistral sont une liste CSV trimée, vides éliminés.
    #[test]
    fn split_csv_trims_and_filters() {
        assert_eq!(
            split_csv(Some(" a , b ,, c ".to_string())),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(split_csv(None).is_empty());
        assert!(split_csv(Some("   ".to_string())).is_empty());
    }

    // Spec : budget neurons — entier brut ou suffixe k/m, sinon erreur.
    #[test]
    fn parse_neuron_budget_scales_and_validates() {
        assert_eq!(parse_neuron_budget(None).unwrap(), None);
        assert_eq!(parse_neuron_budget(Some("  ".into())).unwrap(), None);
        assert_eq!(parse_neuron_budget(Some("9k".into())).unwrap(), Some(9_000));
        assert_eq!(
            parse_neuron_budget(Some("9000".into())).unwrap(),
            Some(9_000)
        );
        assert_eq!(
            parse_neuron_budget(Some("1.5M".into())).unwrap(),
            Some(1_500_000)
        );
        assert!(parse_neuron_budget(Some("plein".into())).is_err());
    }

    // Spec : computed field cache_dir.
    #[test]
    fn computed_dirs() {
        let settings = Settings {
            state_dir: PathBuf::from("/data/lj"),
            db_url: "postgresql://x".into(),
            piste_client_id: None,
            piste_client_secret: None,
            embed_backend: "openai-http".into(),
            embed_url: "http://x".into(),
            embed_api_key: None,
            cloudflare_account_id: None,
            cloudflare_backend_token: None,
            ingest_neuron_budget: None,
            mistral_api_keys: vec![],
            mistral_docapi_keys: vec![],
            mistral_model: "m".into(),
            indexnow_key: None,
            grafana_otlp_endpoint: None,
            grafana_otlp_user: None,
            grafana_cloud_api_key: None,
            otel_service_name: "librejustice-ingest".into(),
            deployment_environment: None,
            watchdog_secs: 14_400,
        };
        assert_eq!(settings.cache_dir(), PathBuf::from("/data/lj/ingest/cache"));
    }
}
