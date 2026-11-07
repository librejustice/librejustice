//! `lj-store` — accès Postgres.
//!
//! Driver : tokio-postgres + deadpool-postgres. Toute écriture passe par
//! [`repository::DecisionRepository`].

pub mod error;
pub mod migrator;
pub mod repository;
