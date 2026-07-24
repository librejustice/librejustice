//! Backend OpenAI-HTTP `/v1/embeddings` (vLLM compatible) — port de
//! `embedding/openai_http.py`.

use crate::backend::{format_query, Embedder, EMBEDDING_DIM, LEGAL_QUERY_INSTRUCTION};
use crate::error::{EmbedError, Result};
use ndarray::Array2;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::Value;

const DEFAULT_TIMEOUT_S: f64 = 600.0;
/// Plafond de la phase d'établissement TCP. Sans lui, un backend injoignable
/// (ex. vLLM sur une machine éteinte : SYN sans réponse, pas de RST) fait pendre
/// le connect sur le timeout TCP de l'OS (~30 s) avant que `AnyEmbedder::Auto`
/// ne bascule sur Cloudflare. Le connect vLLM légitime (tailnet) est <100 ms.
const CONNECT_TIMEOUT_S: f64 = 2.0;
const DEFAULT_MODEL: &str = "Qwen/Qwen3-Embedding-0.6B";

/// Phrases présentes dans les réponses d'erreur des APIs compatibles OpenAI
/// quand le contexte est dépassé (port de `_CONTEXT_OVERFLOW_PHRASES`).
const CONTEXT_OVERFLOW_PHRASES: &[&str] = &["context length", "context_length_exceeded"];

/// Embedder via endpoint OpenAI-compatible (`/v1/embeddings`). Détecte les
/// overflows de contexte (HTTP) pour basculer en fallback chunking.
pub struct OpenAIHttpEmbedder {
    /// Endpoint complet (`…/v1/embeddings`).
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    dim: usize,
    timeout_s: f64,
    // ClientWithMiddleware : TracingMiddleware emet un span HTTP par appel
    // (url.full inclus) — alimente le row "Cloudflare Workers AI" du cockpit.
    client: ClientWithMiddleware,
}

impl OpenAIHttpEmbedder {
    pub fn new(url: impl Into<String>, api_key: Option<String>, model: impl Into<String>) -> Self {
        let raw: String = url.into();
        let trimmed = raw.trim_end_matches('/');
        let endpoint = if trimmed.ends_with("/v1/embeddings") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/v1/embeddings")
        };
        let model: String = model.into();
        let model = if model.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            model
        };
        Self {
            url: endpoint,
            api_key,
            model,
            dim: EMBEDDING_DIM,
            timeout_s: DEFAULT_TIMEOUT_S,
            client: ClientBuilder::new(
                reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs_f64(CONNECT_TIMEOUT_S))
                    .build()
                    .expect("reqwest client openai-http"),
            )
            .with(TracingMiddleware::<SpanBackendWithUrl>::new())
            .build(),
        }
    }

    /// POST des textes, parse + L2-normalise par lignes (port de `_post`).
    async fn post(&self, texts: &[String]) -> Result<Array2<f32>> {
        let mut req = self
            .client
            .post(&self.url)
            .timeout(std::time::Duration::from_secs_f64(self.timeout_s))
            .json(&serde_json::json!({ "model": self.model, "input": texts }));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status == 400 || status == 413 {
            let body = resp.text().await?;
            if CONTEXT_OVERFLOW_PHRASES.iter().any(|p| body.contains(p)) {
                return Err(EmbedError::InputTooLong);
            }
            return Err(EmbedError::Invalid(format!(
                "openai-http {status} : {body}"
            )));
        }
        let resp = resp.error_for_status()?;
        let body: Value = resp.json().await?;

        let data = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| EmbedError::Invalid("openai-http : champ data manquant".into()))?;

        // Tri par index (port de `sorted(..., key=lambda x: x["index"])`).
        let mut rows: Vec<(i64, &Value)> = data
            .iter()
            .map(|item| {
                let idx = item.get("index").and_then(Value::as_i64).unwrap_or(0);
                (idx, item)
            })
            .collect();
        rows.sort_by_key(|(idx, _)| *idx);

        let n = rows.len();
        let mut arr = Array2::<f32>::zeros((n, self.dim));
        for (i, (_, item)) in rows.iter().enumerate() {
            let emb = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| EmbedError::Invalid("openai-http : embedding manquant".into()))?;
            if emb.len() != self.dim {
                return Err(EmbedError::Invalid(format!(
                    "openai-http dimension {}, attendu {}",
                    emb.len(),
                    self.dim
                )));
            }
            for (j, x) in emb.iter().enumerate() {
                arr[[i, j]] = x.as_f64().unwrap_or(0.0) as f32;
            }
        }
        normalize_batch(&mut arr);
        Ok(arr)
    }
}

/// L2-normalisation par ligne avec plancher `1e-12` (port de `_normalize_batch`).
fn normalize_batch(arr: &mut Array2<f32>) {
    for mut row in arr.rows_mut() {
        let norm = row.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        row.mapv_inplace(|x| x / norm);
    }
}

impl Embedder for OpenAIHttpEmbedder {
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        self.post(texts).await
    }

    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format_query(t, LEGAL_QUERY_INSTRUCTION))
            .collect();
        self.post(&prefixed).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_appends_v1_embeddings() {
        let e = OpenAIHttpEmbedder::new("http://127.0.0.1:8400", None, "");
        assert_eq!(e.url, "http://127.0.0.1:8400/v1/embeddings");
    }

    #[test]
    fn endpoint_strips_trailing_slash() {
        let e = OpenAIHttpEmbedder::new("http://host:8400/", None, "m");
        assert_eq!(e.url, "http://host:8400/v1/embeddings");
    }

    #[test]
    fn endpoint_idempotent_when_already_full() {
        let e = OpenAIHttpEmbedder::new("http://host/v1/embeddings", None, "m");
        assert_eq!(e.url, "http://host/v1/embeddings");
    }

    #[test]
    fn normalize_batch_unit_rows() {
        let mut arr = Array2::<f32>::from_shape_vec((2, 2), vec![3.0, 4.0, 0.0, 0.0]).unwrap();
        normalize_batch(&mut arr);
        assert!((arr[[0, 0]] - 0.6).abs() < 1e-6);
        assert!((arr[[0, 1]] - 0.8).abs() < 1e-6);
        // Ligne nulle : divisée par 1e-12 → reste ~0.
        assert!(arr[[1, 0]].abs() < 1e-3);
    }
}
