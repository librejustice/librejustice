//! Client Mistral générique (chat completions) — I/O source partagée.
//!
//! Complétion chat brute (système + utilisateur, température 0) avec rotation de
//! clés round-robin. Consommé par l'ingest (`lj-ingest` : résumés + OCR CNDA) et
//! le rerank listwise de l'API (`lj-api`). Le parsing / la validation de la
//! réponse incombent à l'appelant, qui gère aussi son propre back-off (helpers
//! [`is_retryable_status`] / [`backoff_delay_s`] ; OCR : [`ocr_with_retry`]).
//!
//! Deux stratégies de clé : **round-robin** (chat, [`MistralClient::new`]) qui
//! tourne à chaque appel pour répartir le débit, et **collante** (doc/OCR,
//! [`MistralClient::new_sticky`]) qui tient une seule clé et n'avance qu'**en cas
//! d'échec**. L'OCR depuis une IP datacenter se fait flaguer ; étaler chaque
//! document sur tout le pool (round-robin) salirait toutes les clés d'un coup —
//! la stratégie collante en brûle une à la fois, gardant les autres en réserve.

use anyhow::{Context, Result};
use base64::Engine;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::json;

const MISTRAL_URL: &str = "https://api.mistral.ai/v1/chat/completions";
const OCR_URL: &str = "https://api.mistral.ai/v1/ocr";
/// Modèle OCR (document API). Le `model` du client reste le modèle chat ; l'OCR
/// fixe le sien.
const OCR_MODEL: &str = "mistral-ocr-latest";
const MAX_RETRIES: u32 = 5;
/// Base du back-off : 1ʳᵉ retry ~2 s (×jitter → 1,5–2,5 s) pour laisser le quota
/// se régénérer sur 429 sans re-storming immédiatement ; double à chaque tentative.
const RETRY_BASE_S: f64 = 2.0;
const RETRYABLE_STATUSES: &[u16] = &[429, 500, 502, 503, 504];

/// Client Mistral. Pas de retry interne — l'appelant gère son back-off (helpers
/// [`is_retryable_status`] / [`backoff_delay_s`] ; OCR : [`ocr_with_retry`]).
/// Sélection de clé : round-robin (chat, [`new`](Self::new)) ou collante (doc/OCR,
/// [`new_sticky`](Self::new_sticky)).
pub struct MistralClient {
    http: ClientWithMiddleware,
    keys: Vec<String>,
    /// Empreintes xxh3-64 des clés (même index que `keys`).
    fingerprints: Vec<String>,
    model: String,
    idx: std::sync::atomic::AtomicUsize,
    /// `true` → la clé tourne à **chaque** appel (chat) ; `false` → clé collante
    /// (doc/OCR), n'avance que sur [`advance_key_from`](Self::advance_key_from) (échec).
    round_robin: bool,
    /// Indices des clés vues mortes (401), sautées par le round-robin.
    dead: std::sync::Mutex<std::collections::HashSet<usize>>,
    /// Intervalle minimal entre deux requêtes HTTP (throttle client). `None` (par
    /// défaut) → aucun gate, débit non bridé (prod : le débit est réparti par le
    /// pool multi-clés). `Some(d)` → chaque appel réserve un créneau espacé de `d`
    /// (voir [`with_min_interval`](Self::with_min_interval)) : tient une seule clé
    /// sous la limite RPS du tier gratuit (bancs offline).
    min_interval: Option<std::time::Duration>,
    /// Prochain créneau d'envoi réservable (échéancier du throttle). Ignoré si
    /// `min_interval` est `None`.
    next_slot: tokio::sync::Mutex<tokio::time::Instant>,
}

impl MistralClient {
    /// Client **round-robin** (chat) : clé tournante à chaque appel. `keys` non vide.
    pub fn new(keys: Vec<String>, model: String) -> Result<Self> {
        Self::build(keys, model, true)
    }

    /// Client **collant** (doc/OCR) : tient une clé en régime permanent, n'avance
    /// que sur échec ([`advance_key_from`](Self::advance_key_from)). Chaque clé doc ne vit
    /// que ~30 min après sa 1ʳᵉ utilisation depuis l'IP datacenter : la consommer
    /// seule, puis basculer sur la suivante (encore fraîche) à l'épuisement,
    /// **séquence** les fenêtres au lieu de toutes les démarrer en parallèle
    /// (round-robin). `keys` non vide.
    pub fn new_sticky(keys: Vec<String>, model: String) -> Result<Self> {
        Self::build(keys, model, false)
    }

    fn build(keys: Vec<String>, model: String, round_robin: bool) -> Result<Self> {
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
        // Offset aléatoire par process : on part d'un index pseudo-arbitraire
        // dérivé de l'horloge (équivalent `random.randrange`).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        Ok(Self {
            http,
            idx: std::sync::atomic::AtomicUsize::new(seed % keys.len()),
            fingerprints: keys.iter().map(|k| key_fingerprint(k)).collect(),
            keys,
            model,
            round_robin,
            dead: std::sync::Mutex::new(std::collections::HashSet::new()),
            min_interval: None,
            next_slot: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// Bride le débit à **au plus une requête toutes les `interval`** (throttle
    /// client-side). Sans ça, un fan-out concurrent (les 15 appels du reranker
    /// listwise) part d'un coup et sature la limite RPS d'une clé unique → 429.
    /// Les appels se sérialisent alors en réservant des créneaux espacés de
    /// `interval`. Réservé aux bancs offline sur le compte à clé unique ; la prod
    /// ne l'active pas (le débit est réparti par le pool multi-clés).
    pub fn with_min_interval(mut self, interval: std::time::Duration) -> Self {
        self.min_interval = Some(interval);
        self
    }

    /// Clé courante (index + secret). Round-robin → avance à chaque appel en
    /// sautant les clés mortes ; collante → peek (stable tant que
    /// [`advance_key_from`](Self::advance_key_from) n'est pas appelé).
    fn next_key(&self) -> (usize, &str) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.round_robin {
            let dead = self.dead.lock().expect("mutex dead non empoisonné");
            for _ in 0..self.keys.len() {
                let i = self.idx.fetch_add(1, Relaxed) % self.keys.len();
                if !dead.contains(&i) {
                    return (i, &self.keys[i]);
                }
            }
            let i = self.idx.fetch_add(1, Relaxed) % self.keys.len();
            return (i, &self.keys[i]);
        }
        let i = self.idx.load(Relaxed) % self.keys.len();
        (i, &self.keys[i])
    }

    fn mark_dead(&self, key_idx: usize) {
        self.dead
            .lock()
            .expect("mutex dead non empoisonné")
            .insert(key_idx);
        tracing::warn!(
            fingerprint = %self.fingerprints[key_idx],
            "mistral_key_dead_401"
        );
    }

    /// Empreintes des clés vues mortes (401) pendant la vie du client.
    pub fn spent_fingerprints(&self) -> Vec<String> {
        let dead = self.dead.lock().expect("mutex dead non empoisonné");
        dead.iter().map(|&i| self.fingerprints[i].clone()).collect()
    }

    /// Époque de la clé courante (client collant) — à capturer avant l'appel
    /// pour dédupliquer les avances concurrentes
    /// ([`advance_key_from`](Self::advance_key_from)).
    fn key_epoch(&self) -> usize {
        self.idx.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Avance d'une clé **si l'époque n'a pas déjà bougé** (CAS). Sous OCR
    /// concurrent, N appels en vol prennent un 401 sur la même clé morte :
    /// seul le premier avance, les autres retentent sur la clé fraîche — un
    /// `fetch_add` par appel sauterait N clés d'un bloc.
    fn advance_key_from(&self, epoch: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        let _ = self
            .idx
            .compare_exchange(epoch, epoch + 1, Relaxed, Relaxed);
    }

    /// Nombre de clés du pool (borne de rotation : une passe complète).
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Complétion chat générique (système + utilisateur, température 0). Renvoie le
    /// contenu brut du message ; le parsing/validation incombe à l'appelant.
    ///
    /// `max_tokens` : plafond de génération (`None` = défaut Mistral, pas de plafond).
    /// `prompt_cache_key` : clé de cache prompt côté Mistral (`None` = pas de cache) —
    /// amortit le coût du prompt système sur des appels répétés (ex. rerank listwise).
    pub async fn chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        prompt_cache_key: Option<&str>,
    ) -> Result<String, MistralError> {
        self.complete(self.chat_payload(system, user, max_tokens, prompt_cache_key))
            .await
    }

    /// Payload chat-completions : `max_tokens` / `prompt_cache_key` ne sont inclus
    /// que si `Some` (Mistral applique ses défauts sinon).
    fn chat_payload(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        prompt_cache_key: Option<&str>,
    ) -> serde_json::Value {
        let mut payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0,
            "stream": false,
        });
        if let Some(max_tokens) = max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        if let Some(key) = prompt_cache_key {
            payload["prompt_cache_key"] = json!(key);
        }
        payload
    }

    /// POST `/chat/completions` : extrait `choices[0].message.content` (trimé).
    async fn complete(&self, payload: serde_json::Value) -> Result<String, MistralError> {
        let body = self.post_raw(MISTRAL_URL, payload).await?;
        Ok(body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string())
    }

    /// OCR document (`/v1/ocr`) : PDF → markdown par page concaténé. Le PDF entre
    /// en data-URI base64 inline ; `extract_header`/`extract_footer` sortent les
    /// en-têtes/pieds de page récurrents (n° de page, en-tête `n°`) du corps. Sortie
    /// = `pages[].markdown` joints par `\n` (les pages se concatènent en un texte
    /// continu). Non-déterministe : à cacher en amont (clé = checksum du PDF).
    pub async fn ocr(&self, pdf_bytes: &[u8]) -> Result<String, MistralError> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(pdf_bytes);
        let payload = json!({
            "model": OCR_MODEL,
            "document": {
                "type": "document_url",
                "document_url": format!("data:application/pdf;base64,{b64}"),
            },
            "table_format": "markdown",
            "include_image_base64": false,
            "extract_header": true,
            "extract_footer": true,
        });
        let body = self.post_raw(OCR_URL, payload).await?;
        let markdown = body["pages"]
            .as_array()
            .map(|pages| {
                pages
                    .iter()
                    .map(|p| p["markdown"].as_str().unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(markdown)
    }

    /// POST JSON partagé : clé tournante, statut non-2xx → `MistralError::Status`,
    /// corps désérialisé en `Value`.
    async fn post_raw(
        &self,
        url: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, MistralError> {
        // Throttle client-side (banc) : réserve le prochain créneau espacé de
        // `min_interval` puis attend son échéance. Sous fan-out concurrent (les 15
        // appels du reranker), les tâches se sérialisent en réservant des créneaux
        // croissants — chaque clé reste sous son budget/minute. `None` en prod : pas
        // de gate.
        if let Some(interval) = self.min_interval {
            let wait = {
                let mut slot = self.next_slot.lock().await;
                let now = tokio::time::Instant::now();
                let scheduled = (*slot).max(now);
                *slot = scheduled + interval;
                scheduled.saturating_duration_since(now)
            };
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let (key_idx, key) = self.next_key();
        let response = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {key}"))
            .json(&payload)
            .send()
            .await
            .map_err(MistralError::Middleware)?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            // 401 en round-robin : clé morte, sautée ensuite (le client collant
            // garde sa sémantique 401 propre).
            if status == 401 && self.round_robin {
                self.mark_dead(key_idx);
            }
            return Err(MistralError::Status(status));
        }
        response.json().await.map_err(MistralError::Http)
    }
}

/// Empreinte xxh3-64 hex (16 chars) d'une clé API.
pub fn key_fingerprint(key: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(key.as_bytes()))
}

/// Erreur d'un appel Mistral (distingue statut HTTP du transport réseau, pour la
/// décision de retry).
#[derive(Debug)]
pub enum MistralError {
    /// Réponse HTTP non-2xx (code de statut).
    Status(u16),
    /// Erreur de transport (réseau, timeout, parse JSON).
    Http(reqwest::Error),
    /// Erreur de la chaine middleware reqwest (envoi de la requete).
    Middleware(reqwest_middleware::Error),
}

impl std::fmt::Display for MistralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MistralError::Status(code) => write!(f, "mistral status {code}"),
            MistralError::Http(e) => write!(f, "mistral transport: {e}"),
            MistralError::Middleware(e) => write!(f, "mistral transport: {e}"),
        }
    }
}

impl std::error::Error for MistralError {}

/// Vrai si un statut HTTP justifie un retry (429/5xx).
pub fn is_retryable_status(code: u16) -> bool {
    RETRYABLE_STATUSES.contains(&code)
}

/// Délai de back-off (avec jitter ±25 %) pour une tentative :
/// `base * 2^attempt * (0.75 + rand*0.5)`. `rand01` ∈ [0, 1) injecté (testable).
pub fn backoff_delay_s(attempt: u32, rand01: f64) -> f64 {
    let backoff = RETRY_BASE_S * 2f64.powi(attempt as i32);
    backoff * (0.75 + rand01 * 0.5)
}

/// Tirage `rand01` ∈ [0, 1) non cryptographique (jitter). Dérivé de l'horloge.
pub fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

/// OCR avec un client **collant** ([`MistralClient::new_sticky`]). Deux régimes
/// d'échec :
///
/// - **Clé épuisée (401)** : une clé doc ne vit que ~30 min après sa 1ʳᵉ
///   utilisation depuis l'IP datacenter ; passé ce délai elle renvoie 401. Ce
///   n'est pas un échec franc mais le signal « clé brûlée » → on bascule sur la
///   suivante (encore fraîche, [`advance_key_from`](MistralClient::advance_key_from)) et on
///   retente, sans back-off. Au plus une passe du pool : tout en 401 ⇒ pool épuisé.
/// - **Transitoire (429/5xx, transport)** : back-off exponentiel + jitter sur la
///   **même** clé (jusqu'à `MAX_RETRIES`).
///
/// Tout autre 4xx (400 = doc illisible) remonte en échec franc.
pub async fn ocr_with_retry(
    client: &MistralClient,
    pdf_bytes: &[u8],
    label: &str,
) -> Result<String, MistralError> {
    let mut spent_keys = 0usize;
    let mut transient = 0u32;
    loop {
        let epoch = client.key_epoch();
        match client.ocr(pdf_bytes).await {
            Ok(out) => return Ok(out),
            // Clé épuisée : bascule sur la suivante (fraîche), sans back-off.
            Err(MistralError::Status(401)) => {
                spent_keys += 1;
                if spent_keys >= client.key_count() {
                    return Err(MistralError::Status(401)); // tout le pool brûlé
                }
                client.advance_key_from(epoch);
                tracing::warn!(label, spent_keys, "mistral_ocr_key_spent_rotate");
            }
            // Autre 4xx non-retryable (400 doc illisible) → échec franc.
            Err(MistralError::Status(code)) if !is_retryable_status(code) => {
                return Err(MistralError::Status(code));
            }
            // Transitoire (429/5xx, transport) → back-off, même clé.
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
        c.idx.store(0, Relaxed); // point de départ déterministe
        let seq: Vec<&str> = (0..4).map(|_| c.next_key().1).collect();
        assert_eq!(seq, vec!["a", "b", "c", "a"]); // tourne à chaque appel
    }

    #[test]
    fn round_robin_skips_dead_keys() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        c.mark_dead(1); // « b » morte (401)
        let seq: Vec<&str> = (0..4).map(|_| c.next_key().1).collect();
        assert_eq!(seq, vec!["a", "c", "a", "c"]); // « b » sautée
        assert_eq!(c.spent_fingerprints(), vec![key_fingerprint("b")]);
    }

    #[test]
    fn round_robin_all_dead_still_rotates() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        for i in 0..3 {
            c.mark_dead(i);
        }
        // Tout le pool mort : rotation brute, l'appelant borne ses retries.
        let seq: Vec<&str> = (0..2).map(|_| c.next_key().1).collect();
        assert_eq!(seq.len(), 2);
        assert_ne!(seq[0], seq[1]); // continue de tourner
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
        // Sans options : ni max_tokens ni prompt_cache_key ; température 0, stream off.
        let p = c.chat_payload("sys", "usr", None, None);
        assert_eq!(p["model"], "mistral-small-2506");
        assert_eq!(p["temperature"], 0);
        assert_eq!(p["stream"], false);
        assert!(p.get("max_tokens").is_none());
        assert!(p.get("prompt_cache_key").is_none());
        assert_eq!(p["messages"][0]["role"], "system");
        assert_eq!(p["messages"][0]["content"], "sys");
        assert_eq!(p["messages"][1]["content"], "usr");
        // Avec options (ex. ingest résumé + rerank listwise).
        let p = c.chat_payload("sys", "usr", Some(400), Some("rr-lws-v1"));
        assert_eq!(p["max_tokens"], 400);
        assert_eq!(p["prompt_cache_key"], "rr-lws-v1");
    }

    #[test]
    fn sticky_holds_until_advance() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new_sticky(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        // Même clé tant qu'on n'avance pas (peek).
        assert_eq!(c.next_key().1, "a");
        assert_eq!(c.next_key().1, "a");
        // Avance depuis l'époque courante (clé épuisée) → suivante.
        c.advance_key_from(0);
        assert_eq!(c.next_key().1, "b");
        assert_eq!(c.next_key().1, "b");
        // Époque périmée (un autre appel en vol a déjà basculé) → no-op :
        // la clé fraîche n'est pas sautée.
        c.advance_key_from(0);
        assert_eq!(c.next_key().1, "b");
    }
}
