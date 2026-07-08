//! Calibrage chars↔tokens du tokenizer de référence (Qwen3-Embedding-0.6B).
//!
//! Constantes empiriques (2 700 décisions) partagées par l'estimation de tokens
//! à partir d'une longueur en caractères : budget visa-trim (`decision`) et
//! heuristique de l'embedder Cloudflare. Le chunker BPE *exact*, lui, charge le
//! tokenizer (I/O) et vit côté `lj-ingest`.

/// Médiane chars/token sur le corpus de calibrage.
pub const CHARS_PER_TOKEN_MEDIAN: f64 = 3.41;
pub const CHARS_PER_TOKEN_STDEV: f64 = 0.124;
pub const CHARS_PER_TOKEN_SAFETY_SIGMAS: f64 = 1.7;
/// 3.20 (= MEDIAN − 1.7σ, arrondi) : ratio conservateur pour le budget char-mode.
pub const CHARS_PER_TOKEN_SAFE: f64 = 3.20;
