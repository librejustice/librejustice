//! Client API Judilibre.
//!
//! Client HTTP fin. Trois endpoints utiles à l'ingest (`/scan`,
//! `/transactionalhistory`, `/decision`) avec auth par jeton Bearer OAuth2 PISTE
//! (mêmes identifiants que Légifrance, [`crate::piste`]). L'orchestration vit
//! dans [`crate::downloader`].

use crate::error::{Result, SourceError};
use crate::piste::PisteOAuth;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde_json::Value;
use std::time::Duration;

/// URL de base par défaut de l'API Judilibre (PISTE).
pub const BASE_URL: &str = "https://api.piste.gouv.fr/cassation/judilibre/v1.0";
/// User-Agent envoyé sur chaque requête.
pub const USER_AGENT: &str = "librejustice-judilibre/0.1 (+https://github.com/)";

pub struct JudilibreClient {
    pub base_url: String,
    client: ClientWithMiddleware,
    oauth: PisteOAuth,
}

impl JudilibreClient {
    /// Construit un client. `base_url` est normalisée (trailing `/` retiré),
    /// l'authentification se fait par jeton Bearer OAuth2 PISTE
    /// (client_credentials). [`BASE_URL`] pour la valeur de prod.
    pub fn new(
        base_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
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
    /// Les `params` sont passés tels quels en query-string ; l'orchestrateur
    /// fournit `jurisdiction`, `date_type`, `date_start`/`date_end`, `searchAfter`,
    /// `batch_size`. Réponse : `{results, total, next_batch, ...}` ; `next_batch`
    /// est une querystring contenant `searchAfter` à extraire pour la page suivante.
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

    /// Jurisdiction codes the API serves, read from `/taxonomy?id=jurisdiction`.
    pub async fn jurisdictions(&self) -> Result<Vec<String>> {
        let page = self
            .get("/taxonomy", &[("id", "jurisdiction".to_string())])
            .await?;
        let codes = page
            .get("result")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                SourceError::Invalid(format!("judilibre taxonomy jurisdiction: {page}"))
            })?;
        Ok(codes.keys().cloned().collect())
    }

    /// Cœur partagé : GET signé Bearer, JSON ou `JudilibreApi` sur statut ≠ 200.
    ///
    /// Sur **401**, le jeton caché est purgé et la requête rejouée **une fois**
    /// avec un Bearer frais. A 429 or a 5xx is asked again three times, 1, 2
    /// then 4 s apart: the source answers 500 while its Elasticsearch memory
    /// breaker is tripped.
    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut delay = Duration::from_secs(1);
        let mut retries = 0;
        loop {
            let result = match self.get_once(path, params).await {
                Err(SourceError::JudilibreApi { status: 401, .. }) => {
                    self.oauth.invalidate().await;
                    self.get_once(path, params).await
                }
                other => other,
            };
            match result {
                Err(SourceError::JudilibreApi { status, .. })
                    if (status == 429 || status >= 500) && retries < 3 =>
                {
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                    retries += 1;
                }
                other => return other,
            }
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
        parse_response(&text)
    }
}

/// Out of the async body, which compiles in its caller, at the caller's opt-level.
/// A decision text can carry a lone UTF-16 surrogate, read as U+FFFD.
#[inline(never)]
fn parse_response(text: &str) -> Result<Value> {
    let mut de = sonic_rs::Deserializer::from_str(text).utf8_lossy();
    let value = de.deserialize()?;
    de.end()?;
    Ok(value)
}

/// Extrait un paramètre d'une querystring (`?key=value` ou `key=value`).
///
/// Renvoie la **première** valeur du paramètre, `None` si la querystring est
/// vide/absente ou la clé absente. Décode le percent-encoding et les `+`.
pub fn extract_query_param(querystring: Option<&str>, key: &str) -> Option<String> {
    let qs = querystring?;
    if qs.is_empty() {
        return None;
    }
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

/// Décodage querystring minimal : `+` → espace, `%XX` → octet.
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

    #[test]
    fn lone_surrogate_reads_as_replacement_character() {
        let v = parse_response(r#"{"text":"Localit\ud83d\u00e9"}"#).unwrap();
        assert_eq!(v["text"], "Localit\u{FFFD}\u{e9}");
    }
}
