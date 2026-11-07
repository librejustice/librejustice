use lj_pg::error::{pg_detail, PgError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("postgres: {}", pg_detail(.0))]
    Postgres(#[from] tokio_postgres::Error),
    #[error(transparent)]
    Pg(#[from] PgError),
    #[error(
        "schéma en retard de {} migration(s) ({pending:?}) : lancez `lj-ingest migrate`, seule commande qui applique",
        pending.len()
    )]
    PendingMigrations { pending: Vec<i32> },
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
