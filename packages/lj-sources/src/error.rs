//! Erreurs des sources I/O (thiserror).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] sonic_rs::Error),
    #[error("judilibre api {status} {url}: {body}")]
    JudilibreApi {
        status: u16,
        url: String,
        body: String,
    },
    #[error("legifrance api {status} {url}: {body}")]
    LegifranceApi {
        status: u16,
        url: String,
        body: String,
    },
    #[error("piste oauth {status} {url}: {body}")]
    PisteOAuth {
        status: u16,
        url: String,
        body: String,
    },
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, SourceError>;
