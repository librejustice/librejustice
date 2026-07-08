//! Fournisseur de jeton OAuth2 PISTE (`client_credentials`), partagé par les
//! clients Judilibre et Légifrance — deux API DILA derrière la passerelle PISTE,
//! authentifiées par le même couple `client_id`/`client_secret`
//! (`LIBREJUSTICE_PISTE_CLIENT_ID`/`_SECRET`). Le jeton Bearer est mis en cache et
//! rafraîchi à expiration.

use std::time::{Duration, Instant};

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

/// Endpoint OAuth2 PISTE (production).
pub const OAUTH_TOKEN_URL: &str = "https://oauth.piste.gouv.fr/api/oauth/token";
/// User-Agent des requêtes de jeton.
const USER_AGENT: &str = "librejustice-piste/0.1 (+https://github.com/)";

/// Détient les identifiants `client_credentials` et un jeton Bearer caché.
pub struct PisteOAuth {
    oauth_url: String,
    client_id: String,
    client_secret: String,
    client: ClientWithMiddleware,
    /// Jeton courant + instant d'expiration (avec marge). `None` au démarrage.
    token: Mutex<Option<(String, Instant)>>,
}

impl PisteOAuth {
    /// Construit un fournisseur de production (endpoint [`OAUTH_TOKEN_URL`]).
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            oauth_url: OAUTH_TOKEN_URL.to_string(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            client,
            token: Mutex::new(None),
        }
    }

    /// Renvoie un jeton Bearer valide, en le rafraîchissant si absent/expiré.
    ///
    /// Le lock est tenu pendant tout le rafraîchissement : sous forte concurrence,
    /// un seul appelant fait le `POST /token`, les autres attendent puis lisent le
    /// jeton frais — pas de stampede sur l'endpoint OAuth.
    pub async fn bearer(&self) -> Result<String> {
        let mut guard = self.token.lock().await;
        if let Some((tok, expiry)) = guard.as_ref() {
            if Instant::now() < *expiry {
                return Ok(tok.clone());
            }
        }
        // client_credentials, scope openid (cf. FAQ API PISTE). On encode le corps
        // `x-www-form-urlencoded` à la main : `reqwest_middleware` n'expose pas
        // `.form()`, et le secret peut contenir des caractères réservés.
        let form = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=openid",
            urlencode(&self.client_id),
            urlencode(&self.client_secret),
        );
        let resp = self
            .client
            .post(&self.oauth_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("oauth piste {}: {e}", self.oauth_url)))?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if status != 200 {
            return Err(SourceError::PisteOAuth {
                status,
                url: self.oauth_url.clone(),
                body,
            });
        }
        let v: Value = sonic_rs::from_str(&body)?;
        let tok = v["access_token"]
            .as_str()
            .ok_or_else(|| SourceError::Invalid("oauth piste: access_token absent".into()))?
            .to_string();
        // expires_in en secondes (souvent 3600) ; marge de 60 s avant péremption.
        let ttl = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
        let expiry = Instant::now() + Duration::from_secs(ttl);
        *guard = Some((tok.clone(), expiry));
        Ok(tok)
    }

    /// Purge le jeton caché : le prochain [`bearer`](Self::bearer) en re-POSTera
    /// un frais. Appelé quand l'API renvoie 401 alors qu'on croyait le jeton
    /// valide — le TTL réel côté PISTE peut être plus court que `expires_in`
    /// annoncé (ou skew d'horloge), d'où une expiration anticipée. Auto-réparation.
    pub async fn invalidate(&self) {
        *self.token.lock().await = None;
    }
}

/// Percent-encode pour un corps `application/x-www-form-urlencoded` (RFC 3986
/// *unreserved* préservé, tout le reste en `%XX`). Suffisant pour les valeurs
/// OAuth (id/secret), qui n'ont pas d'espaces.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
