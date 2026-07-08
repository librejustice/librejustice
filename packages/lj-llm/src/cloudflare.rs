//! Backend Cloudflare Workers AI (port de `embedding/cloudflare.py`).
//!
//! POST `accounts/{id}/ai/run/{model}` avec `{"text": [...]}`. Chaque chunk est
//! envoyé seul (1 vecteur attendu par appel). Les passages trop longs sont
//! re-chunkés par caractères puis moyennés et re-normalisés (parité Python).

use crate::backend::{format_query, Embedder, EMBEDDING_DIM, LEGAL_QUERY_INSTRUCTION};
use crate::error::{EmbedError, Result};
use lj_core::tokens::CHARS_PER_TOKEN_MEDIAN;
use ndarray::{Array2, Axis};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::Value;
use std::sync::Mutex;

/// Cap tokens par document côté Workers AI (Qwen3-Embedding-0.6B).
const CF_DOC_TOKEN_CAP: usize = 8_192;
/// Taille de chunk (caractères) par défaut.
const CF_CHUNK_CHARS: usize = 24_000;
/// Plancher de facturation neurons (CF tronque le header à 2 décimales).
const CF_NEURON_FLOOR: f64 = 0.01;

/// Embedder Cloudflare Workers AI. Estime les tokens (heuristique chars/médiane
/// Qwen) pour respecter le budget par requête et re-chunke si nécessaire.
pub struct CloudflareWorkersAIEmbedder {
    pub account_id: String,
    pub token: String,
    pub model: String,
    dim: usize,
    chunk_chars: usize,
    timeout_s: f64,
    neuron_budget: Option<usize>,
    // ClientWithMiddleware : TracingMiddleware emet un span HTTP par appel
    // (url.full inclus) — alimente le row "Cloudflare Workers AI" du cockpit.
    client: ClientWithMiddleware,
    /// Neurons cumulés (header `cf-ai-neurons`), pour le budget. Mutex pour
    /// rester `&self` (le trait `Embedder` n'accorde qu'un emprunt partagé).
    neurons_used: Mutex<f64>,
}

impl CloudflareWorkersAIEmbedder {
    /// Modèle par défaut côté serveur (égal à `DEFAULT_MODEL` Python).
    pub const DEFAULT_MODEL: &'static str = "@cf/qwen/qwen3-embedding-0.6b";

    pub fn new(
        account_id: impl Into<String>,
        token: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            token: token.into(),
            model: model.into(),
            dim: EMBEDDING_DIM,
            chunk_chars: CF_CHUNK_CHARS,
            timeout_s: 60.0,
            neuron_budget: None,
            client: ClientBuilder::new(reqwest::Client::new())
                .with(TracingMiddleware::<SpanBackendWithUrl>::new())
                .build(),
            neurons_used: Mutex::new(0.0),
        }
    }

    /// Fixe un budget neurons (au-delà → `TokenBudgetExceeded`).
    pub fn with_neuron_budget(mut self, budget: Option<usize>) -> Self {
        self.neuron_budget = budget;
        self
    }

    fn url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{}",
            self.account_id, self.model
        )
    }

    /// Port de `_chunk_with` : découpe récursive par caractères sous le cap
    /// tokens. Opère sur les *char boundaries* (le slicing Python est par
    /// codepoint Unicode).
    fn chunk_with(&self, text: &str, chunk_chars: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        if n <= chunk_chars {
            if estimate_tokens(text) <= CF_DOC_TOKEN_CAP {
                return vec![text.to_string()];
            }
            let mid = (n / 2).max(1);
            let left: String = chars[..mid].iter().collect();
            let right: String = chars[mid..].iter().collect();
            let mut out = self.chunk_with(&left, chunk_chars);
            out.extend(self.chunk_with(&right, chunk_chars));
            return out;
        }
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        while i < n {
            let end = (i + chunk_chars).min(n);
            let piece: String = chars[i..end].iter().collect();
            if estimate_tokens(&piece) > CF_DOC_TOKEN_CAP {
                out.extend(self.chunk_with(&piece, (chunk_chars / 2).max(1)));
            } else {
                out.push(piece);
            }
            i += chunk_chars;
        }
        out
    }

    fn chunk(&self, text: &str) -> Vec<String> {
        self.chunk_with(text, self.chunk_chars)
    }

    /// POST d'un batch d'inputs ; renvoie `(n_inputs, dim)`. Applique le budget
    /// neurons et vérifie la forme (port de `_post`).
    async fn post(&self, inputs: &[String]) -> Result<Array2<f32>> {
        let expected = inputs.len();
        let resp = self
            .client
            .post(self.url())
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs_f64(self.timeout_s))
            .json(&serde_json::json!({ "text": inputs }))
            .send()
            .await?;

        let neurons_header = resp
            .headers()
            .get("cf-ai-neurons")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let resp = resp.error_for_status()?;

        let neurons_header = neurons_header.ok_or_else(|| {
            EmbedError::Invalid(
                "Workers AI : header cf-ai-neurons manquant — budget non-enforceable".into(),
            )
        })?;
        let neurons_value: f64 = neurons_header.parse().map_err(|_| {
            EmbedError::Invalid(format!(
                "Workers AI : header cf-ai-neurons malformé : {neurons_header:?}"
            ))
        })?;

        let body: Value = resp.json().await?;
        if body.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(EmbedError::Invalid(format!(
                "Workers AI réponse erreur : {}",
                body.get("errors").unwrap_or(&Value::Null)
            )));
        }

        // Plancher 0.01 (CF tronque le header à 2 décimales).
        {
            let mut used = self.neurons_used.lock().expect("neurons mutex");
            *used += neurons_value.max(CF_NEURON_FLOOR);
            if let Some(budget) = self.neuron_budget {
                if *used > budget as f64 {
                    return Err(EmbedError::TokenBudgetExceeded {
                        used: *used as usize,
                        budget,
                    });
                }
            }
        }

        let data = body
            .get("result")
            .and_then(|r| r.get("data"))
            .and_then(Value::as_array)
            .ok_or_else(|| EmbedError::Invalid("Workers AI : champ result.data manquant".into()))?;
        if data.len() != expected {
            return Err(EmbedError::Invalid(format!(
                "Workers AI : format inattendu (attendu {expected} vecteurs, reçu {})",
                data.len()
            )));
        }

        let mut arr = Array2::<f32>::zeros((expected, self.dim));
        for (i, row) in data.iter().enumerate() {
            let vec = row
                .as_array()
                .ok_or_else(|| EmbedError::Invalid("Workers AI : vecteur non-array".into()))?;
            if vec.len() != self.dim {
                return Err(EmbedError::Invalid(format!(
                    "Workers AI dimension ({}, {}), attendu ({expected}, {})",
                    expected,
                    vec.len(),
                    self.dim
                )));
            }
            for (j, x) in vec.iter().enumerate() {
                arr[[i, j]] = x.as_f64().unwrap_or(0.0) as f32;
            }
        }
        Ok(arr)
    }

    /// Embed chaque chunk seul puis concatène (port de `_embed_chunks`).
    async fn embed_chunks(&self, chunks: &[String]) -> Result<Array2<f32>> {
        if chunks.is_empty() {
            return Ok(Array2::<f32>::zeros((0, self.dim)));
        }
        let mut out = Array2::<f32>::zeros((chunks.len(), self.dim));
        for (i, chunk) in chunks.iter().enumerate() {
            let v = self.post(std::slice::from_ref(chunk)).await?;
            out.row_mut(i).assign(&v.row(0));
        }
        Ok(out)
    }
}

/// Heuristique tokens (port de `_estimate_tokens`).
fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count() as f64;
    let est = (chars / CHARS_PER_TOKEN_MEDIAN).round() as usize;
    est.max(1)
}

impl Embedder for CloudflareWorkersAIEmbedder {
    /// Port de `embed_passages` : chunk par texte, embed tous les chunks à plat,
    /// puis pour chaque texte : 1 chunk → tel quel, n>1 → moyenne re-normalisée.
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        let chunks_per_text: Vec<Vec<String>> = texts.iter().map(|t| self.chunk(t)).collect();
        let flat: Vec<String> = chunks_per_text.iter().flatten().cloned().collect();
        let flat_vecs = self.embed_chunks(&flat).await?;

        let mut out = Array2::<f32>::zeros((texts.len(), self.dim));
        let mut idx = 0;
        for (i, cs) in chunks_per_text.iter().enumerate() {
            let n = cs.len();
            if n == 0 {
                continue;
            }
            let vecs = flat_vecs.slice(ndarray::s![idx..idx + n, ..]);
            idx += n;
            if n == 1 {
                out.row_mut(i).assign(&vecs.row(0));
            } else {
                let avg = vecs.mean_axis(Axis(0)).expect("mean over >=1 rows");
                let norm: f32 = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    out.row_mut(i).assign(&avg.mapv(|x| x / norm));
                } else {
                    out.row_mut(i).assign(&avg);
                }
            }
        }
        Ok(out)
    }

    /// Port de `embed_query` : préfixe l'instruction puis embed chunk par chunk.
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format_query(t, LEGAL_QUERY_INSTRUCTION))
            .collect();
        self.embed_chunks(&prefixed).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_uses_account_and_model() {
        let e = CloudflareWorkersAIEmbedder::new(
            "acct123",
            "tok",
            CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
        );
        assert_eq!(
            e.url(),
            "https://api.cloudflare.com/client/v4/accounts/acct123/ai/run/@cf/qwen/qwen3-embedding-0.6b"
        );
    }

    #[test]
    fn estimate_tokens_floor_and_median() {
        // max(1, round(len / 3.41)).
        assert_eq!(estimate_tokens(""), 1);
        assert_eq!(estimate_tokens("a"), 1);
        // 341 chars / 3.41 = 100.
        let s = "a".repeat(341);
        assert_eq!(estimate_tokens(&s), 100);
    }

    #[test]
    fn chunk_short_text_single() {
        let e = CloudflareWorkersAIEmbedder::new("a", "t", "m");
        // Texte court, sous le cap tokens → un seul chunk.
        let chunks = e.chunk("petit texte");
        assert_eq!(chunks, vec!["petit texte".to_string()]);
    }

    #[test]
    fn chunk_splits_by_chars() {
        let e = CloudflareWorkersAIEmbedder::new("a", "t", "m");
        // 60000 chars > CF_CHUNK_CHARS (24000) → plusieurs morceaux, recouvrant
        // tout le texte sans chevauchement (window stride = chunk_chars).
        let text = "x".repeat(60_000);
        let chunks = e.chunk(&text);
        assert!(chunks.len() >= 3);
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert_eq!(total, 60_000);
    }
}
