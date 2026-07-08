//! Trait commun des embedders (port de `embedding/base.py`).

use crate::error::Result;
use ndarray::Array2;
use std::sync::Mutex;
use std::time::Instant;

/// Dimension cible des embeddings.
pub const EMBEDDING_DIM: usize = 1024;

/// Instruction Qwen3 asymétrique pour les requêtes (port fidèle de
/// `LEGAL_QUERY_INSTRUCTION`, Python). Doit rester identique octet-pour-octet :
/// elle entre dans le texte effectivement embedé et donc dans la parité.
pub const LEGAL_QUERY_INSTRUCTION: &str =
    "Given a French-language legal question, retrieve passages from \
French court decisions that best answer it.";

/// Préfixe une requête de l'instruction (format Qwen3-Embedding asymétrique).
///
/// Port de `format_query` : `f"Instruct: {instruction}\nQuery: {text}"`.
pub fn format_query(text: &str, instruction: &str) -> String {
    format!("Instruct: {instruction}\nQuery: {text}")
}

/// Backend d'embedding (passages vs requêtes, asymétrique). `async` car I/O HTTP.
#[allow(async_fn_in_trait)]
pub trait Embedder {
    /// Embed des passages (documents). Renvoie `(n_texts, EMBEDDING_DIM)`.
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>>;
    /// Embed des requêtes utilisateur (avec instruction).
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>>;
}

/// Poids de l'historique dans l'EMA par observation (port de `_FAILURE_ALPHA`).
const FAILURE_ALPHA: f64 = 0.70;
/// Demi-vie temporelle (1 h) — score /2 sans activité (port de `_FAILURE_HALF_LIFE`).
const FAILURE_HALF_LIFE: f64 = 3600.0;
/// Seuil au-dessus duquel vLLM est sauté (port de `_FAILURE_SKIP`).
const FAILURE_SKIP: f64 = 0.80;

/// Failure score ∈ [0, 1] pour un backend (port de `_BackendHealth`, Python).
///
/// Deux mécanismes indépendants :
/// - EMA par observation (alpha fixe) : chaque appel compte autant, peu importe
///   la cadence. 5 échecs consécutifs → score > 0.8.
/// - Décroissance temporelle (demi-vie 1 h) : le score fond naturellement quand
///   le backend n'est plus sollicité ou réussit à nouveau.
///
/// Horloge monotone via `std::time::Instant` (équivalent de `time.monotonic`),
/// décroissance via `f64::exp` — pur Rust, ARM-safe. État mutable derrière un
/// `Mutex` pour rester `&self` (le trait `Embedder` n'accorde qu'un emprunt
/// partagé).
struct BackendHealthState {
    failure_score: f64,
    last_update: Instant,
}

pub struct BackendHealth {
    inner: Mutex<BackendHealthState>,
}

impl Default for BackendHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendHealth {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BackendHealthState {
                failure_score: 0.0,
                last_update: Instant::now(),
            }),
        }
    }

    /// Score courant, décru temporellement depuis le dernier événement (port de
    /// `current_score` : `exp(-dt/half_life) * failure_score`, sans mutation).
    pub fn current_score(&self) -> f64 {
        let st = self.inner.lock().expect("backend health mutex");
        let dt = st.last_update.elapsed().as_secs_f64();
        (-dt / FAILURE_HALF_LIFE).exp() * st.failure_score
    }

    /// Enregistre une observation succès/échec (port de `record`) : décroissance
    /// temporelle depuis le dernier événement, puis EMA fixe alpha-pondérée.
    pub fn record(&self, failed: bool) {
        let mut st = self.inner.lock().expect("backend health mutex");
        let now = Instant::now();
        let dt = now.duration_since(st.last_update).as_secs_f64();
        let time_decayed = (-dt / FAILURE_HALF_LIFE).exp() * st.failure_score;
        st.failure_score =
            FAILURE_ALPHA * time_decayed + (1.0 - FAILURE_ALPHA) * if failed { 1.0 } else { 0.0 };
        st.last_update = now;
    }

    /// Lecture brute du `failure_score` (pour le log de parité, sans décroissance).
    fn raw_score(&self) -> f64 {
        self.inner
            .lock()
            .expect("backend health mutex")
            .failure_score
    }
}

/// Dispatcher statique des backends (évite `dyn Embedder`, non object-safe à
/// cause des `async fn`). Construit par `auto`/`cloudflare`/`openai-http`.
pub enum AnyEmbedder {
    Dummy(DummyEmbedder),
    Cloudflare(crate::cloudflare::CloudflareWorkersAIEmbedder),
    OpenAiHttp(crate::openai_http::OpenAIHttpEmbedder),
    /// vLLM (OpenAI-HTTP) en priorité, fallback Cloudflare Workers AI piloté par
    /// un `BackendHealth` (port de `AutoEmbedder`, Python).
    Auto {
        vllm: crate::openai_http::OpenAIHttpEmbedder,
        cloudflare: crate::cloudflare::CloudflareWorkersAIEmbedder,
        health: BackendHealth,
    },
}

impl Embedder for AnyEmbedder {
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        match self {
            AnyEmbedder::Dummy(e) => e.embed_passages(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_passages(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_passages(texts).await,
            AnyEmbedder::Auto {
                vllm,
                cloudflare,
                health,
            } => {
                let score = health.current_score();
                if score < FAILURE_SKIP {
                    match vllm.embed_passages(texts).await {
                        Ok(result) => {
                            health.record(false);
                            return Ok(result);
                        }
                        Err(exc) => {
                            health.record(true);
                            tracing::warn!(
                                failure_score = health.raw_score(),
                                error = %exc,
                                "vllm embed échoué → cloudflare"
                            );
                        }
                    }
                } else {
                    tracing::debug!(failure_score = score, "vllm sauté");
                }
                cloudflare.embed_passages(texts).await
            }
        }
    }
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        match self {
            AnyEmbedder::Dummy(e) => e.embed_query(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_query(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_query(texts).await,
            AnyEmbedder::Auto {
                vllm,
                cloudflare,
                health,
            } => {
                // Port de `AutoEmbedder.embed_query` : sous 0.8 on tente vLLM,
                // tout échec → record(failed) + log + fallthrough Cloudflare ;
                // au-dessus de 0.8 on saute vLLM direct vers Cloudflare.
                let score = health.current_score();
                if score < FAILURE_SKIP {
                    match vllm.embed_query(texts).await {
                        Ok(result) => {
                            health.record(false);
                            return Ok(result);
                        }
                        Err(exc) => {
                            health.record(true);
                            tracing::warn!(
                                failure_score = health.raw_score(),
                                error = %exc,
                                "vllm embed échoué → cloudflare"
                            );
                        }
                    }
                } else {
                    tracing::debug!(failure_score = score, "vllm sauté");
                }
                cloudflare.embed_query(texts).await
            }
        }
    }
}

/// Embedder déterministe par hachage (tests / mode hors-ligne).
///
/// Port de `DummyEmbedder` : produit un vecteur gaussien déterministe par texte,
/// L2-normalisé. ATTENTION parité : le Python s'appuie sur
/// `numpy.random.default_rng(seed).standard_normal(dim)` (PCG64 + Ziggurat),
/// non reproductible sans répliquer l'implémentation interne de numpy. Cette
/// version utilise un PRNG splitmix64 + Box-Muller, déterministe et
/// auto-cohérent côté Rust, mais **non identique octet-pour-octet** aux
/// vecteurs Python. Le dummy n'est utilisé qu'en tests/CI ; aucune parité
/// numérique n'est attendue pour ce backend (cf. notes).
#[derive(Debug, Clone, Copy)]
pub struct DummyEmbedder {
    pub dim: usize,
}

impl Default for DummyEmbedder {
    fn default() -> Self {
        Self { dim: EMBEDDING_DIM }
    }
}

impl DummyEmbedder {
    fn hash_embed(&self, texts: &[String]) -> Array2<f32> {
        let dim = self.dim;
        let mut out = Array2::<f32>::zeros((texts.len(), dim));
        for (i, text) in texts.iter().enumerate() {
            let seed = seed_from_text(text);
            let mut rng = SplitMix64::new(seed);
            // Tirage gaussien déterministe (Box-Muller).
            let mut v: Vec<f32> = (0..dim).map(|_| rng.next_gaussian() as f32).collect();
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            for (j, x) in v.into_iter().enumerate() {
                out[[i, j]] = x;
            }
        }
        out
    }
}

impl Embedder for DummyEmbedder {
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        Ok(self.hash_embed(texts))
    }
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        Ok(self.hash_embed(texts))
    }
}

/// Graine 64 bits dérivée du texte (FNV-1a 64). Substitut dependency-free du
/// `sha256(text)[:8]` Python — déterministe, pas équivalent cryptographiquement.
fn seed_from_text(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in text.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// PRNG splitmix64 — déterministe, sans dépendance externe.
struct SplitMix64 {
    state: u64,
    spare: Option<f64>,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Flottant uniforme dans `(0, 1)`.
    fn next_f64(&mut self) -> f64 {
        // 53 bits de mantisse → (0,1), borne basse écartée de zéro.
        let bits = self.next_u64() >> 11;
        (bits as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Tirage gaussien centré-réduit (Box-Muller, avec spare).
    fn next_gaussian(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let u1 = self.next_f64();
        let u2 = self.next_f64();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_query_shape() {
        // Port exact de format_query (Python).
        let q = format_query("ma question", "instruction X");
        assert_eq!(q, "Instruct: instruction X\nQuery: ma question");
    }

    #[test]
    fn legal_instruction_value() {
        assert_eq!(
            LEGAL_QUERY_INSTRUCTION,
            "Given a French-language legal question, retrieve passages from \
French court decisions that best answer it."
        );
    }

    #[tokio::test]
    async fn dummy_is_deterministic_and_normalized() {
        let e = DummyEmbedder::default();
        let a = e.embed_passages(&["bonjour".to_string()]).await.unwrap();
        let b = e.embed_passages(&["bonjour".to_string()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a.shape(), &[1, EMBEDDING_DIM]);
        let norm: f32 = a.row(0).iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm={norm}");
    }

    #[tokio::test]
    async fn dummy_distinct_texts_differ() {
        let e = DummyEmbedder { dim: 16 };
        let a = e.embed_query(&["a".to_string()]).await.unwrap();
        let b = e.embed_query(&["b".to_string()]).await.unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn backend_health_five_failures_skip_vllm() {
        // Parité du docstring `_BackendHealth` : 5 échecs consécutifs poussent le
        // score > 0.8 (seuil FAILURE_SKIP). Les `record` sont immédiats donc
        // dt≈0 → décroissance temporelle ≈ 1.0, on reste sur l'EMA pure :
        // s_{k} = 0.7 * s_{k-1} + 0.3 ; partant de 0 → 0.3, 0.51, 0.657, 0.7599, 0.83193.
        let h = BackendHealth::new();
        for _ in 0..5 {
            h.record(true);
        }
        let score = h.current_score();
        assert!(
            score > FAILURE_SKIP,
            "score={score} (attendu > {FAILURE_SKIP})"
        );

        // Vérification analytique (dt≈0 sur la fenêtre du test).
        assert!(
            (h.raw_score() - 0.83193).abs() < 1e-3,
            "raw={}",
            h.raw_score()
        );
    }

    #[test]
    fn backend_health_success_decays_score() {
        // Un succès enregistre une observation 0.0 → l'EMA fait baisser le score.
        let h = BackendHealth::new();
        for _ in 0..5 {
            h.record(true);
        }
        let before = h.raw_score();
        h.record(false);
        assert!(
            h.raw_score() < before,
            "after={} before={before}",
            h.raw_score()
        );
        // Un seul succès depuis un score saturé : 0.7 * 0.83193 ≈ 0.5824 < seuil.
        assert!(h.current_score() < FAILURE_SKIP);
    }

    #[tokio::test]
    async fn dummy_batches_rows() {
        let e = DummyEmbedder { dim: 8 };
        let out = e
            .embed_passages(&["x".to_string(), "y".to_string(), "z".to_string()])
            .await
            .unwrap();
        assert_eq!(out.shape(), &[3, 8]);
    }
}
