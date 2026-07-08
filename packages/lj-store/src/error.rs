//! Erreurs du store (thiserror).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("postgres: {0}")]
    Postgres(#[from] tokio_postgres::Error),
    #[error("pool: {0}")]
    Pool(String),
    #[error(
        "migration {version:04}: la base contient des versions absentes sur disque: {missing:?}"
    )]
    UnknownMigrations { version: i32, missing: Vec<i32> },
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
