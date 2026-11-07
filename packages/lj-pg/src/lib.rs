//! The Postgres pool every binary opens, and the runner that applies a series of migrations.

pub mod db;
pub mod error;
pub mod migrator;
