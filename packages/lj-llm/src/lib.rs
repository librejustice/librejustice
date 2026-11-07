//! `lj-llm` — backends d'inférence modèle : embeddings + client Mistral (chat/OCR).
//!
//! Embeddings : Cloudflare Workers AI, OpenAI-HTTP, cache
//! in-process (`moka`), quantisation L2 + sérialisation vector JSON. Client
//! Mistral (`mistral`) : chat completions + OCR document, rotation de clés et
//! registre de quota (`ratelimit`), partagé entre l'ingest (résumés, OCR CNDA),
//! l'API (rerank listwise) et les bancs.
//!
//! Gemini (`gemini`) writes the summaries. It shares neither the ledger nor the
//! rotation: its quota is a daily count held per project, announced only in the
//! body of a 429, so a refused key is parked rather than metered.

pub mod anthropic;
pub mod backend;
pub mod cache;
pub mod cloudflare;
pub mod codex;
pub mod error;
pub mod gemini;
pub mod hosts;
pub mod luna;
pub mod mistral;
pub mod oauth;
pub mod openai_http;
pub mod quantize;
pub mod ratelimit;
