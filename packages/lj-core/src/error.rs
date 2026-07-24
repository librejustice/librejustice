//! Erreurs des libs pures (thiserror). Pas de `unwrap()` en chemin non-test.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("XML parse error: {0}")]
    Xml(String),
    #[error("unknown jurisdiction_type: {0:?}")]
    UnknownJuridiction(Option<String>),
}

pub type Result<T> = std::result::Result<T, CoreError>;
