//! Client Mistral : chat completions et OCR document, sur un pool de clés.
//!
//! Le débit n'est pas un réglage. Chaque appel passe par le registre
//! ([`crate::ratelimit`]), qui choisit une clé capable de payer et attend sinon.
//! Un appelant à échéance coupe l'attente par son `timeout_at`.
//!
//! Un client = un modèle = un seau, donc un registre par instance. Deux clients
//! sur deux **alias** du même modèle en tiendraient deux pour un seul seau ;
//! `GET /v1/models` déclare le graphe.

use anyhow::{Context, Result};
use base64::Engine;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::json;

use crate::ratelimit::{now_unix, Ledger, Quota};

// chat/completions serves mistral-small and mistral-medium no request on the free
// tier; conversations serves them at the announced rate.
const MISTRAL_URL: &str = "https://api.mistral.ai/v1/conversations";
const OCR_URL: &str = "https://api.mistral.ai/v1/ocr";
const MAX_RETRIES: u32 = 5;
/// Back-off des pannes de service (5xx, transport) : ~2 s doublées à chaque cran.
const RETRY_BASE_S: f64 = 2.0;
const RETRYABLE_STATUSES: &[u16] = &[429, 500, 502, 503, 504];

/// No retry inside: the caller decides (OCR: [`ocr_layout_with_retry`]).
pub struct MistralClient {
    http: ClientWithMiddleware,
    keys: Vec<String>,
    /// Même index que `keys`.
    fingerprints: Vec<String>,
    model: String,
    idx: std::sync::atomic::AtomicUsize,
    /// Indices des clés refusées (401, 402), sautées par le round-robin.
    dead: std::sync::Mutex<std::collections::HashSet<usize>>,
    /// Refus pas encore remontés, drainés par
    /// [`drain_spent_fingerprints`](Self::drain_spent_fingerprints).
    unreported: std::sync::Mutex<Vec<(usize, u16)>>,
    ledger: Ledger,
}

impl MistralClient {
    /// `keys` non vide.
    pub fn new(keys: Vec<String>, model: String) -> Result<Self> {
        if keys.is_empty() {
            anyhow::bail!("MistralClient: keys must be non-empty");
        }
        let http = ClientBuilder::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .context("mistral: client HTTP")?,
        )
        .with(TracingMiddleware::<SpanBackendWithUrl>::new())
        .build();
        // Deux instances ne démarrent pas sur la même clé.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        let ledger = Ledger::new(keys.len());
        Ok(Self {
            http,
            idx: std::sync::atomic::AtomicUsize::new(seed % keys.len()),
            fingerprints: keys.iter().map(|k| key_fingerprint(k)).collect(),
            keys,
            model,
            dead: std::sync::Mutex::new(std::collections::HashSet::new()),
            unreported: std::sync::Mutex::new(Vec::new()),
            ledger,
        })
    }

    /// Départ round-robin, refusées retirées ; le registre prend la première qui
    /// peut payer. Pool entièrement refusé ⇒ ordre complet, une clé peut
    /// redevenir valide.
    fn key_order(&self) -> Vec<usize> {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.keys.len();
        let start = self.idx.fetch_add(1, Relaxed) % n;
        let all = || (0..n).map(|k| (start + k) % n);
        let dead = self.dead.lock().expect("mutex dead non empoisonné");
        let live: Vec<usize> = all().filter(|i| !dead.contains(i)).collect();
        if live.is_empty() {
            all().collect()
        } else {
            live
        }
    }

    fn mark_dead(&self, key_idx: usize, status: u16) {
        if !self
            .dead
            .lock()
            .expect("mutex dead non empoisonné")
            .insert(key_idx)
        {
            return;
        }
        self.unreported
            .lock()
            .expect("mutex unreported non empoisonné")
            .push((key_idx, status));
        tracing::warn!(
            fingerprint = %self.fingerprints[key_idx],
            status,
            "mistral_key_dead"
        );
    }

    pub fn live_key_count(&self) -> usize {
        self.keys.len() - self.dead.lock().expect("mutex dead non empoisonné").len()
    }

    /// Refus constatés depuis le dernier appel. Draine : un client de longue vie
    /// ne doit pas faire réécrire la même ligne à chaque requête.
    pub fn drain_spent_fingerprints(&self) -> Vec<(String, u16)> {
        let mut unreported = self
            .unreported
            .lock()
            .expect("mutex unreported non empoisonné");
        unreported
            .drain(..)
            .map(|(i, status)| (self.fingerprints[i].clone(), status))
            .collect()
    }

    /// Calls a minute the pool admits as it stands, summed over its keys.
    pub fn rate_per_min(&self) -> f64 {
        self.ledger.rate_per_min()
    }

    /// Calls in flight the pool justifies holding, and the keys still mute.
    pub fn demand(&self) -> (f64, usize) {
        self.ledger.demand()
    }

    /// Borne de rotation : une passe complète du pool.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Contenu brut du message ; le parsing incombe à l'appelant.
    pub async fn chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> Result<String, MistralError> {
        self.complete(
            self.chat_payload(system, user, max_tokens),
            system.len() + user.len(),
            true,
        )
        .await
    }

    /// Same, without the wait: a caller holding one queue over several pools needs
    /// its job back rather than parked behind this bucket.
    pub async fn chat_now(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> Result<String, MistralError> {
        self.complete(
            self.chat_payload(system, user, max_tokens),
            system.len() + user.len(),
            false,
        )
        .await
    }

    fn chat_payload(&self, system: &str, user: &str, max_tokens: Option<u32>) -> serde_json::Value {
        let mut payload = json!({
            "model": self.model,
            "instructions": system,
            "inputs": user,
            "store": false,
            "completion_args": {"temperature": 0},
        });
        if let Some(max_tokens) = max_tokens {
            payload["completion_args"]["max_tokens"] = json!(max_tokens);
        }
        payload
    }

    async fn complete(
        &self,
        payload: serde_json::Value,
        prompt_chars: usize,
        wait: bool,
    ) -> Result<String, MistralError> {
        let body = self
            .post_raw(MISTRAL_URL, payload, prompt_chars, wait)
            .await?;
        Ok(message_text(&body))
    }

    /// The pages of the PDF as the OCR returns them, each with its blocks and their boxes.
    pub async fn ocr_layout(
        &self,
        pdf_bytes: &[u8],
    ) -> Result<Vec<serde_json::Value>, MistralError> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(pdf_bytes);
        let payload = json!({
            "model": self.model,
            "document": {
                "type": "document_url",
                "document_url": format!("data:application/pdf;base64,{b64}"),
            },
            "table_format": "markdown",
            "include_image_base64": false,
            "extract_header": true,
            "extract_footer": true,
        });
        // Coût nul : l'OCR se facture à la page, la taille du base64 ne prédit
        // rien. Seul le compteur de requêtes gate ; l'en-tête corrige ensuite.
        let body = self.post_raw(OCR_URL, payload, 0, true).await?;
        Ok(body["pages"].as_array().cloned().unwrap_or_default())
    }

    /// `prompt_chars` = taille du texte facturable, pour l'estimation de coût.
    async fn post_raw(
        &self,
        url: &str,
        payload: serde_json::Value,
        prompt_chars: usize,
        wait: bool,
    ) -> Result<serde_json::Value, MistralError> {
        // A caller on a deadline cuts this sleep with its `timeout_at`, and `Drop`
        // gives the reservation back.
        let reservation = loop {
            match self
                .ledger
                .reserve(&self.key_order(), prompt_chars, now_unix())
            {
                Ok(reservation) => break reservation,
                Err(delay) if !wait => return Err(MistralError::Busy(delay.as_secs_f64())),
                Err(delay) => tokio::time::sleep(delay).await,
            }
        };
        let key_idx = reservation.key_idx();
        let response = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.keys[key_idx]))
            .json(&payload)
            .send()
            .await
            .map_err(MistralError::Middleware)?;
        // Un refus n'annonce que le compteur épuisé : c'est justement celui qui
        // doit arrêter d'émettre.
        let quota = Quota::from_headers(response.headers());
        let status = response.status().as_u16();
        if !response.status().is_success() {
            if status == 429 {
                reservation.refused(quota, now_unix());
            } else {
                reservation.settle(quota, None, now_unix());
            }
            // 401 : clé refusée, sautée ensuite.
            // A 402 organisation never spends its budget, so the ledger would pick it
            // first once the live keys saturate.
            if status == 401 || status == 402 {
                self.mark_dead(key_idx, status);
            } else if status == 429 {
                // Quelle ressource a manqué ne se déduit pas du volume émis :
                // seule la réponse le dit.
                tracing::warn!(
                    key = %self.fingerprints[key_idx],
                    req_left = ?quota.req.map(|c| c.left),
                    tok_left = ?quota.tok.map(|c| c.left),
                    "mistral_rate_limited"
                );
            }
            return Err(MistralError::Status(status));
        }
        let body = response.json::<serde_json::Value>().await;
        reservation.settle(quota, billed_tokens(body.as_ref().ok()), now_unix());
        body.map_err(MistralError::Http)
    }
}

/// Tokens actually billed for a served call. `/v1/conversations` sends no
/// `x-ratelimit-tokens-query-cost`, and the counter drops by exactly this instead.
fn billed_tokens(body: Option<&serde_json::Value>) -> Option<u32> {
    let total = body?["usage"]["total_tokens"].as_u64()?;
    u32::try_from(total).ok()
}

fn message_text(body: &serde_json::Value) -> String {
    let mut text = String::new();
    for entry in body["outputs"].as_array().into_iter().flatten() {
        match &entry["content"] {
            serde_json::Value::String(s) => text.push_str(s),
            serde_json::Value::Array(chunks) => {
                for chunk in chunks {
                    if let Some(t) = chunk["text"].as_str() {
                        text.push_str(t);
                    }
                }
            }
            _ => {}
        }
    }
    text.trim().to_string()
}

/// Empreinte xxh3-64 hex d'une clé API.
pub fn key_fingerprint(key: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(key.as_bytes()))
}

#[derive(Debug)]
pub enum MistralError {
    /// Réponse HTTP non-2xx (code de statut).
    Status(u16),
    /// No key can pay yet, and one will in this many seconds. Nothing was sent.
    Busy(f64),
    /// Erreur de transport (réseau, timeout, parse JSON).
    Http(reqwest::Error),
    /// Erreur de la chaine middleware reqwest (envoi de la requete).
    Middleware(reqwest_middleware::Error),
}

impl std::fmt::Display for MistralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MistralError::Status(code) => write!(f, "mistral status {code}"),
            MistralError::Busy(wait) => write!(f, "mistral: pool busy for {wait:.1}s"),
            MistralError::Http(e) => write!(f, "mistral transport: {e}"),
            MistralError::Middleware(e) => write!(f, "mistral transport: {e}"),
        }
    }
}

impl std::error::Error for MistralError {}

pub fn is_retryable_status(code: u16) -> bool {
    RETRYABLE_STATUSES.contains(&code)
}

/// Back-off avec jitter ±25 %. `rand01` ∈ [0, 1) injecté (testable).
pub fn backoff_delay_s(attempt: u32, rand01: f64) -> f64 {
    let backoff = RETRY_BASE_S * 2f64.powi(attempt as i32);
    backoff * (0.75 + rand01 * 0.5)
}

/// Tirage non cryptographique dérivé de l'horloge.
pub fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

pub async fn ocr_layout_with_retry(
    client: &MistralClient,
    pdf_bytes: &[u8],
    label: &str,
) -> Result<Vec<serde_json::Value>, MistralError> {
    with_retry(client, label, || client.ocr_layout(pdf_bytes)).await
}

/// A 401 moves on to the next key at once, a 429 retries at once since admission
/// does the waiting, a 5xx or a transport error backs off. Any other 4xx returns.
async fn with_retry<T, F, Fut>(
    client: &MistralClient,
    label: &str,
    op: F,
) -> Result<T, MistralError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, MistralError>>,
{
    let mut dead_keys = 0usize;
    let mut throttled = 0u32;
    let mut transient = 0u32;
    loop {
        match op().await {
            Ok(out) => return Ok(out),
            Err(MistralError::Status(401)) => {
                dead_keys += 1;
                if dead_keys >= client.key_count() {
                    return Err(MistralError::Status(401)); // pool entièrement refusé
                }
                tracing::warn!(label, dead_keys, "mistral_ocr_key_dead_rotate");
            }
            Err(MistralError::Status(429)) => {
                if throttled >= MAX_RETRIES {
                    return Err(MistralError::Status(429));
                }
                throttled += 1;
                tracing::warn!(label, attempt = throttled, "mistral_ocr_throttled_retry");
            }
            Err(MistralError::Status(code)) if !is_retryable_status(code) => {
                return Err(MistralError::Status(code));
            }
            Err(err) => {
                if transient >= MAX_RETRIES {
                    return Err(err);
                }
                let delay = backoff_delay_s(transient, rand01());
                transient += 1;
                tracing::warn!(
                    label,
                    attempt = transient,
                    max = MAX_RETRIES,
                    backoff = delay,
                    error = %err,
                    "mistral_ocr_retry"
                );
                tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_jitters() {
        // base 2 s : 1ʳᵉ tentative ∈ [1,5 ; 2,5] s selon jitter.
        assert!(backoff_delay_s(0, 0.0) >= 1.5 && backoff_delay_s(0, 1.0) <= 2.5);
        assert!(backoff_delay_s(2, 0.5) > backoff_delay_s(0, 0.5));
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(404));
    }

    fn keys() -> Vec<String> {
        vec!["a".into(), "b".into(), "c".into()]
    }

    #[test]
    fn round_robin_advances_each_call() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        assert_eq!(c.key_order(), vec![0, 1, 2]);
        assert_eq!(c.key_order(), vec![1, 2, 0]);
        assert_eq!(c.key_order(), vec![2, 0, 1]);
        assert_eq!(c.key_order(), vec![0, 1, 2]);
    }

    #[test]
    fn round_robin_skips_dead_keys() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        c.mark_dead(1, 401); // « b » morte (401)
        assert_eq!(c.key_order(), vec![0, 2]);
        assert_eq!(c.key_order(), vec![2, 0]);
        assert_eq!(
            c.drain_spent_fingerprints(),
            vec![(key_fingerprint("b"), 401)]
        );
        c.mark_dead(1, 401);
        assert!(c.drain_spent_fingerprints().is_empty());
    }

    #[test]
    fn round_robin_all_dead_still_rotates() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        for i in 0..3 {
            c.mark_dead(i, 401);
        }
        // Tout le pool refusé : ordre complet, l'appelant borne ses retries.
        assert_eq!(c.key_order(), vec![0, 1, 2]);
        assert_eq!(c.key_order(), vec![1, 2, 0]);
    }

    #[test]
    fn fingerprint_is_stable_hex16() {
        assert_eq!(key_fingerprint("a").len(), 16);
        assert_eq!(key_fingerprint("a"), key_fingerprint("a"));
        assert_ne!(key_fingerprint("a"), key_fingerprint("b"));
    }

    #[test]
    fn chat_payload_optional_fields() {
        let c = MistralClient::new(vec!["k".into()], "mistral-small-2506".into()).unwrap();
        let p = c.chat_payload("sys", "usr", None);
        assert_eq!(p["model"], "mistral-small-2506");
        assert_eq!(p["instructions"], "sys");
        assert_eq!(p["inputs"], "usr");
        assert_eq!(p["store"], false);
        assert_eq!(p["completion_args"]["temperature"], 0);
        assert!(p["completion_args"].get("max_tokens").is_none());
        let p = c.chat_payload("sys", "usr", Some(400));
        assert_eq!(p["completion_args"]["max_tokens"], 400);
    }

    #[test]
    fn message_text_reads_string_and_chunked_content() {
        let plain = serde_json::json!({"outputs": [{"content": " 1: 90\n2: 10 "}]});
        assert_eq!(message_text(&plain), "1: 90\n2: 10");
        let chunked = serde_json::json!({"outputs": [
            {"content": [
                {"type": "thinking", "thinking": [{"type": "text", "text": "raisonnement"}]},
                {"type": "text", "text": "1: 90\n"},
                {"type": "text", "text": "2: 10"},
            ]},
        ]});
        assert_eq!(message_text(&chunked), "1: 90\n2: 10");
        assert_eq!(message_text(&serde_json::json!({})), "");
    }
}
