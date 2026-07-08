//! `lj-store` — accès Postgres (port de `packages/py/librejustice-store`).
//!
//! Driver : tokio-postgres + deadpool-postgres (PAS sqlx). pgvector via crate
//! `pgvector` (feature postgres). Pas de SQL dispersé (règle #2) : toute
//! écriture passe par [`repository::DecisionRepository`]. Le runner de
//! migrations embarque les 45 `.sql` sous `migrations/` (inchangées).

pub mod db;
pub mod error;
pub mod migrator;
pub mod repository;
