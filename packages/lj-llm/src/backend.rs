//! Trait commun des embedders (port de `embedding/base.py`).

use crate::error::Result;
use ndarray::Array2;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Dispatcher statique des backends (évite `dyn Embedder`, non object-safe à
/// cause des `async fn`). Construit par `auto`/`cloudflare`/`openai-http`.
pub enum AnyEmbedder {
    Dummy(DummyEmbedder),
    Cloudflare(crate::cloudflare::CloudflareWorkersAIEmbedder),
    OpenAiHttp(crate::openai_http::OpenAIHttpEmbedder),
    /// vLLM (OpenAI-HTTP) primaire, repli Cloudflare Workers AI en disjoncteur
    /// binaire (ADR 0221). `degraded=false` : vLLM seul. À son premier échec
    /// (typiquement le `connect_timeout` quand la machine vLLM est éteinte) on
    /// latch `degraded=true` ; chaque requête lance alors vLLM et Cloudflare en
    /// même temps, le premier qui répond gagne — un succès vLLM ré-arme
    /// `degraded=false`.
    Auto {
        vllm: crate::openai_http::OpenAIHttpEmbedder,
        cloudflare: crate::cloudflare::CloudflareWorkersAIEmbedder,
        degraded: AtomicBool,
    },
}

/// Disjoncteur du mode `auto` (ADR 0221), factorisé entre passages et requêtes
/// via deux fabriques de futures (une par backend), ré-appelables (`Fn`).
///
/// `degraded=false` : vLLM seul. Son premier échec latch `degraded=true` et sert
/// la requête courante via Cloudflare (le seul appel qui « attend » vLLM — borné
/// par son `connect_timeout`). `degraded=true` : vLLM et Cloudflare sont lancés
/// **en même temps**, `tokio::select!` renvoie le premier qui répond. Cloudflare
/// répond vite donc l'utilisateur n'attend jamais l'échec de vLLM ; le vLLM
/// concurrent est la sonde de reprise — dès qu'il regagne la course (vLLM local
/// bat Cloudflare quand il est up), on ré-arme `degraded=false`.
async fn auto_embed<VF, CF, VFut, CFut>(
    degraded: &AtomicBool,
    vllm: VF,
    cloudflare: CF,
) -> Result<Array2<f32>>
where
    VF: Fn() -> VFut,
    CF: Fn() -> CFut,
    VFut: std::future::Future<Output = Result<Array2<f32>>>,
    CFut: std::future::Future<Output = Result<Array2<f32>>>,
{
    if !degraded.load(Ordering::Relaxed) {
        match vllm().await {
            Ok(r) => return Ok(r),
            Err(exc) => {
                degraded.store(true, Ordering::Relaxed);
                tracing::warn!(error = %exc, "vllm KO → dégradé, repli Cloudflare");
                return cloudflare().await;
            }
        }
    }
    tokio::select! {
        r = vllm() => match r {
            Ok(v) => {
                degraded.store(false, Ordering::Relaxed);
                tracing::info!("vllm a regagné la course → mode nominal réarmé");
                Ok(v)
            }
            Err(_) => cloudflare().await,
        },
        r = cloudflare() => r,
    }
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
                degraded,
            } => {
                auto_embed(
                    degraded,
                    || vllm.embed_passages(texts),
                    || cloudflare.embed_passages(texts),
                )
                .await
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
                degraded,
            } => {
                auto_embed(
                    degraded,
                    || vllm.embed_query(texts),
                    || cloudflare.embed_query(texts),
                )
                .await
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
