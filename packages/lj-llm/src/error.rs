//! Erreurs embedding (thiserror).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("http middleware: {0}")]
    Middleware(#[from] reqwest_middleware::Error),
    #[error("input too long for embedder")]
    InputTooLong,
    #[error("token budget exceeded: used {used}, budget {budget}")]
    TokenBudgetExceeded { used: usize, budget: usize },
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, EmbedError>;
