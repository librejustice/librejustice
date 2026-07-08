//! Client API Judilibre (port de `sources/judilibre/client.py`).
//!
//! Client HTTP fin : pas de logique métier, pas de cache. Trois endpoints
//! utiles à l'ingest (`/scan`, `/transactionalhistory`, `/decision`) avec auth
//! par jeton Bearer OAuth2 PISTE (mêmes identifiants que Légifrance, cf.
//! [`crate::piste`]). L'orchestration (pagination, watermark, dispatch fichier)
//! vit dans [`crate::downloader`].

use crate::error::{Result, SourceError};
use crate::piste::PisteOAuth;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde_json::Value;

/// URL de base par défaut de l'API Judilibre (PISTE).
pub const BASE_URL: &str = "https://api.piste.gouv.fr/cassation/judilibre/v1.0";
/// User-Agent envoyé sur chaque requête (port de `client.USER_AGENT`).
pub const USER_AGENT: &str = "librejustice-judilibre/0.1 (+https://github.com/)";

/// Client HTTP Judilibre (reqwest). Détient `base_url`, l'OAuth PISTE et le client.
pub struct JudilibreClient {
    pub base_url: String,
    client: ClientWithMiddleware,
    oauth: PisteOAuth,
}

impl JudilibreClient {
    /// Construit un client. `base_url` est normalisée (trailing `/` retiré),
    /// l'authentification se fait par jeton Bearer OAuth2 PISTE (client_credentials).
    ///
    /// Note : contrairement au Python (`base_url` par défaut), le contrat Rust
    /// passe explicitement `base_url` en premier argument. Utiliser
    /// [`BASE_URL`] pour la valeur de prod.
    pub fn new(
        base_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // Wrappé reqwest-middleware + TracingMiddleware : chaque requête émet un
        // span HTTP client (http.client.request) exporté vers Tempo.
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            base_url,
            client,
            oauth: PisteOAuth::new(client_id, client_secret),
        }
    }

    /// `GET /scan` — pagination cursor via `searchAfter`.
    ///
    /// Successeur officiel de `/export` (deprecated). Les `params` sont passés
    /// tels quels en query-string ; l'orchestrateur fournit `jurisdiction`,
    /// `date_type`, `date_start`/`date_end`, `searchAfter`, `batch_size`.
    /// Réponse : `{results, total, next_batch, ...}` ; `next_batch` est une
    /// querystring contenant `searchAfter` à extraire pour la page suivante.
    pub async fn scan(&self, params: &[(&str, String)]) -> Result<Value> {
        self.get("/scan", params).await
    }

    /// `GET /transactionalhistory` — opérations depuis `date`.
    ///
    /// Réponse : `{transactions, total, next_page, page_size, query_date}`.
    /// `next_page` contient `from_id` à extraire pour la page suivante.
    pub async fn transactional_history(&self, date: &str, from_id: Option<&str>) -> Result<Value> {
        let mut params: Vec<(&str, String)> = vec![("date", date.to_string())];
        if let Some(fid) = from_id {
            params.push(("from_id", fid.to_string()));
        }
        self.get("/transactionalhistory", &params).await
    }

    /// `GET /decision?id=...` — décision unique.
    pub async fn decision(&self, decision_id: &str) -> Result<Value> {
        self.get("/decision", &[("id", decision_id.to_string())])
            .await
    }

    /// Cœur partagé : GET signé Bearer, JSON ou `JudilibreApi` sur statut ≠ 200.
    ///
    /// Sur **401**, le jeton caché est purgé et la requête rejouée **une fois**
    /// avec un Bearer frais : le TTL réel PISTE peut être plus court que
    /// l'`expires_in` annoncé, d'où un 401 en plein run long (cf. `piste.rs`).
    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        match self.get_once(path, params).await {
            Err(SourceError::JudilibreApi { status: 401, .. }) => {
                self.oauth.invalidate().await;
                self.get_once(path, params).await
            }
            other => other,
        }
    }

    async fn get_once(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let token = self.oauth.bearer().await?;
        let response = self
            .client
            .get(&url)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json")
            .query(params)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête judilibre {url}: {e}")))?;
        let status = response.status();
        if status.as_u16() != 200 {
            let body = response.text().await.unwrap_or_default();
            return Err(SourceError::JudilibreApi {
                status: status.as_u16(),
                url,
                body,
            });
        }
        let text = response.text().await?;
        Ok(sonic_rs::from_str(&text)?)
    }
}

/// Extrait un paramètre d'une querystring (`?key=value` ou `key=value`).
///
/// Port fidèle de `extract_query_param` (Python `urllib.parse.parse_qs`) :
/// renvoie la **première** valeur du paramètre, `None` si la querystring est
/// vide/absente ou la clé absente. Décode le percent-encoding et les `+`.
pub fn extract_query_param(querystring: Option<&str>, key: &str) -> Option<String> {
    let qs = querystring?;
    if qs.is_empty() {
        return None;
    }
    // Tolère un éventuel `?` de tête (parse_qs ne le voit jamais, mais les
    // valeurs Judilibre `next_batch`/`next_page` sont des querystrings nues).
    let qs = qs.strip_prefix('?').unwrap_or(qs);
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        if decode_qs_component(k) == key {
            return Some(decode_qs_component(v));
        }
    }
    None
}

/// Décodage querystring minimal : `+` → espace, `%XX` → octet. Suffisant pour
/// les curseurs opaques Judilibre (qui peuvent contenir des `%2C`, etc.).
fn decode_qs_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_query_param_basic() {
        let qs = Some("searchAfter=abc123&batch_size=1000");
        assert_eq!(
            extract_query_param(qs, "searchAfter").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_query_param(qs, "batch_size").as_deref(),
            Some("1000")
        );
    }

    #[test]
    fn extract_query_param_missing_and_empty() {
        assert_eq!(extract_query_param(Some("a=1"), "b"), None);
        assert_eq!(extract_query_param(Some(""), "a"), None);
        assert_eq!(extract_query_param(None, "a"), None);
    }

    #[test]
    fn extract_query_param_first_value_wins() {
        // parse_qs renvoie la première valeur (parsed.get(key, [None])[0]).
        assert_eq!(
            extract_query_param(Some("x=1&x=2"), "x").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn extract_query_param_percent_and_plus_decoding() {
        assert_eq!(
            extract_query_param(Some("from_id=a%2Cb+c"), "from_id").as_deref(),
            Some("a,b c")
        );
    }

    #[test]
    fn base_url_trailing_slash_trimmed() {
        let c = JudilibreClient::new("https://x/v1/", "id", "secret");
        assert_eq!(c.base_url, "https://x/v1");
    }
}
