//! The product's migrations, embedded with `include_str!`, and the check that the
//! schema is current.

use crate::error::{Result, StoreError};
use lj_pg::db::Connection;
use lj_pg::migrator::{applied_versions, apply_migrations_with, Migration};

/// Liste statique des migrations embarquées, triées par version croissante.
/// Source : `packages/lj-store/migrations/NNNN_*.sql`.
pub fn embedded_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 118,
            name: "0118_release_2026_07_08.sql",
            sql: include_str!("../migrations/0118_release_2026_07_08.sql"),
        },
        Migration {
            version: 163,
            name: "0163_release_2026_07_24.sql",
            sql: include_str!("../migrations/0163_release_2026_07_24.sql"),
        },
        Migration {
            version: 166,
            name: "0166_release_2026_07_24.sql",
            sql: include_str!("../migrations/0166_release_2026_07_24.sql"),
        },
        Migration {
            version: 207,
            name: "0207_release_2026_08_12.sql",
            sql: include_str!("../migrations/0207_release_2026_08_12.sql"),
        },
        Migration {
            version: 289,
            name: "0289_release_2026_09_04.sql",
            sql: include_str!("../migrations/0289_release_2026_09_04.sql"),
        },
        Migration {
            version: 338,
            name: "0338_release_2026_09_29.sql",
            sql: include_str!("../migrations/0338_release_2026_09_29.sql"),
        },
        Migration {
            version: 395,
            name: "0395_release_2026_11_07.sql",
            sql: include_str!("../migrations/0395_release_2026_11_07.sql"),
        },
        Migration {
            version: 476,
            name: "0476_release_2026_11_21.sql",
            sql: include_str!("../migrations/0476_release_2026_11_21.sql"),
        },
    ]
}

/// Table de suivi du produit.
pub const SCHEMA_MIGRATIONS: &str = "schema_migrations";

/// Applies the pending migrations under `Apply`, checks the schema is current
/// under `Verify`. Returns the versions applied.
pub async fn apply_migrations(conn: &Connection, intent: MigrationIntent) -> Result<Vec<i32>> {
    match intent {
        MigrationIntent::Apply => {
            Ok(apply_migrations_with(conn, embedded_migrations(), SCHEMA_MIGRATIONS).await?)
        }
        MigrationIntent::Verify => {
            verify_schema_current(conn).await?;
            Ok(Vec::new())
        }
    }
}

/// What a caller allows itself on the schema. Every command states it: an implicit
/// `Apply` lets any command run a pending migration and hold its locks unasked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MigrationIntent {
    /// Refuses a schema that lags, and writes nothing.
    Verify,
    /// Applies what is pending. Reserved for `lj-ingest migrate`.
    Apply,
}

/// The check of [`apply_migrations`] without its writing: raises when the base
/// holds a version the binary lacks that [`newer_than_disk`](lj_pg::migrator::newer_than_disk)
/// refuses, then when the binary holds one the base lacks. It does not create `schema_migrations`, a check writing nothing.
async fn verify_schema_current(conn: &Connection) -> Result<()> {
    let table: Option<String> = conn
        .query_one(
            &format!("SELECT to_regclass('{SCHEMA_MIGRATIONS}')::text"),
            &[],
        )
        .await?
        .get(0);
    let available: Vec<i32> = embedded_migrations().iter().map(|m| m.version).collect();
    let applied = match table {
        Some(_) => applied_versions(conn, SCHEMA_MIGRATIONS).await?,
        None => Vec::new(),
    };
    let newer = lj_pg::migrator::newer_than_disk(&available, &applied)?;
    if !newer.is_empty() {
        tracing::warn!(?newer, "migrations: the database is ahead of this binary");
        return Ok(());
    }
    let pending: Vec<i32> = available
        .into_iter()
        .filter(|v| !applied.contains(v))
        .collect();
    if pending.is_empty() {
        Ok(())
    } else {
        Err(StoreError::PendingMigrations { pending })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_pg::migrator::{segments, Segment};

    #[test]
    fn embedded_migrations_ascend() {
        let migs = embedded_migrations();
        assert!(!migs.is_empty());
        for w in migs.windows(2) {
            assert!(
                w[0].version < w[1].version,
                "{} after {}",
                w[1].name,
                w[0].name
            );
        }
    }

    /// A `.sql` dropped into `migrations/` without its line in
    /// `embedded_migrations` breaks no build and no other test: the migrator
    /// ignores it, and the code that depends on it runs without its schema.
    #[test]
    fn every_sql_file_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut sur_disque: Vec<String> = std::fs::read_dir(&dir)
            .expect("dossier migrations lisible")
            .map(|e| {
                e.expect("entrée lisible")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|n| n.ends_with(".sql"))
            .collect();
        sur_disque.sort();

        let mut embarquees: Vec<String> = embedded_migrations()
            .iter()
            .map(|m| m.name.to_string())
            .collect();
        embarquees.sort();

        assert_eq!(
            sur_disque, embarquees,
            "migrations/ et embedded_migrations() divergent — ajouter la ligne manquante"
        );
    }

    #[test]
    fn migration_names_match_versions() {
        for m in embedded_migrations() {
            let prefix: String = m.name.chars().take(4).collect();
            let parsed: i32 = prefix.parse().expect("prefix NNNN numérique");
            assert_eq!(parsed, m.version, "nom {} vs version {}", m.name, m.version);
            assert!(m.name.ends_with(".sql"));
        }
    }

    #[test]
    fn sans_transaction_segments_carry_a_single_statement() {
        for m in embedded_migrations() {
            for segment in segments(m.sql) {
                let Segment::SansTransaction(sql) = segment else {
                    continue;
                };
                let body: String = sql
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .collect();
                assert_eq!(body.matches(';').count(), 1, "{}: {sql}", m.name);
            }
        }
    }

    #[test]
    fn first_migration_sql_is_non_empty() {
        let migs = embedded_migrations();
        let init = &migs[0];
        assert!(init.sql.contains("schema_migrations") || init.sql.contains("decisions"));
        assert!(!init.sql.trim().is_empty());
    }
}
