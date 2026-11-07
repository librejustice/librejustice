//! Chars-to-tokens calibration of the reference tokenizer (Qwen3-Embedding-0.6B).
//!
//! Empirical constants shared by every token estimate made
//! from a length in characters: the visa-trim budget of `lj-core` and the
//! Cloudflare embedder heuristic. The exact BPE chunker loads the tokenizer
//! and lives in `lj-ingest`.

/// Median chars per token over the calibration corpus.
pub const CHARS_PER_TOKEN_MEDIAN: f64 = 3.41;
pub const CHARS_PER_TOKEN_STDEV: f64 = 0.124;
pub const CHARS_PER_TOKEN_SAFETY_SIGMAS: f64 = 1.7;
/// The median less `CHARS_PER_TOKEN_SAFETY_SIGMAS` deviations, rounded: the
/// conservative ratio of the char-mode budget.
pub const CHARS_PER_TOKEN_SAFE: f64 = 3.20;

/// The conservative chars per token of a text in `lang` (ISO 639-1), which the
/// char-mode chunker budgets with: French its `CHARS_PER_TOKEN_SAFE`, another
/// language the 5th percentile over a sample of its texts. A language without a
/// measure is budgeted as French.
pub fn chars_per_token_safe(lang: &str) -> f64 {
    match lang {
        "en" => 3.41,
        "es" => 3.23,
        "pt" => 2.99,
        "ro" => 2.68,
        "sv" => 2.65,
        "pl" => 2.57,
        "it" => 2.47,
        "da" => 2.44,
        "nl" => 2.32,
        "fi" => 2.30,
        "de" => 2.27,
        "hr" | "et" => 2.25,
        "sl" | "ga" => 2.24,
        "lt" | "bg" => 2.16,
        "hu" => 2.15,
        "sk" => 2.08,
        "mt" => 2.07,
        "cs" => 2.01,
        "lv" => 1.98,
        "el" => 1.14,
        _ => CHARS_PER_TOKEN_SAFE,
    }
}
