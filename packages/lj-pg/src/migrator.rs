//! Idempotent migration runner. Each pending version is applied in its own
//! transaction, cut around the statements it marks to run outside one, and a
//! version the database holds without its file is an error.
//!
//! Several runners may start at once: the whole run sits under a session
//! advisory lock. The loser waits, rereads the versions, and finds nothing left.

use crate::db::Connection;
use crate::error::{PgError, Result};

/// One migration of a series: its version and its embedded SQL.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Key of the advisory lock serialising concurrent runs. The two-int4 form
/// (`pg_advisory_lock(int4, int4)`) is a lock space disjoint from bigint locks
/// keyed on a row id. `classid` = `'LJ'`, `objid` = `'mig'`.
const MIGRATION_LOCK_CLASS: i32 = 0x4C4A;
const MIGRATION_LOCK_OBJ: i32 = 0x6D6967;

/// Takes the session advisory lock serialising migrations. The `SELECT
/// pg_advisory_lock` can block well past the pool's 30 s `statement_timeout`,
/// hence a transaction with `SET LOCAL statement_timeout = 0`. A session lock
/// outlives the COMMIT, held until `pg_advisory_unlock` or the session's end.
async fn acquire_migration_lock(conn: &Connection) -> Result<()> {
    tracing::info!("migrations: acquiring the advisory lock");
    let waited = std::time::Instant::now();
    conn.batch_execute("BEGIN").await?;
    let acquired = async {
        conn.batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        conn.execute(
            "SELECT pg_advisory_lock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await?;
        Ok::<(), PgError>(())
    }
    .await;
    match acquired {
        Ok(()) => {
            conn.batch_execute("COMMIT").await?;
            tracing::info!(
                waited_ms = waited.elapsed().as_millis() as u64,
                "migrations: advisory lock acquired"
            );
            Ok(())
        }
        Err(err) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            Err(err)
        }
    }
}

/// Creates the tracking table if absent, and its schema when the name is
/// qualified: the table precedes the first migration, which cannot create it.
///
/// Existence is tested before the `CREATE`: `IF NOT EXISTS` first checks the
/// `CREATE` privilege on the database, which the schema's owner need not hold.
/// Safe under the migration lock, held by the caller.
async fn ensure_schema_migrations(conn: &Connection, table: &str) -> Result<()> {
    if let Some((schema, _)) = table.split_once('.') {
        let existe: bool = conn
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
                &[&schema],
            )
            .await?
            .get(0);
        if !existe {
            conn.batch_execute(&format!("CREATE SCHEMA {schema}"))
                .await?;
        }
    }
    conn.batch_execute(&format!(
        "CREATE TABLE IF NOT EXISTS {table} (\
           version INTEGER PRIMARY KEY,\
           applied_at TIMESTAMPTZ NOT NULL\
         )"
    ))
    .await?;
    Ok(())
}

/// Versions already applied.
pub async fn applied_versions(conn: &Connection, table: &str) -> Result<Vec<i32>> {
    let rows = conn
        .query(&format!("SELECT version FROM {table}"), &[])
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, i32>(0)).collect())
}

/// Applies a series tracked in its own table: two series sharing a table would
/// refuse each other, each raising on a version missing from its disk. The lock
/// is common, both touching the same database. `table` is an SQL object name,
/// hence the `&'static`. Returns the versions applied.
pub async fn apply_migrations_with(
    conn: &Connection,
    migrations: Vec<Migration>,
    table: &'static str,
) -> Result<Vec<i32>> {
    acquire_migration_lock(conn).await?;
    let result = apply_migrations_locked(conn, migrations, table).await;
    let _ = conn
        .execute(
            "SELECT pg_advisory_unlock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await;
    result
}

/// Body of [`apply_migrations_with`], run under the lock. The applied versions
/// are read after the lock, so a loser sees what the winner committed.
async fn apply_migrations_locked(
    conn: &Connection,
    available: Vec<Migration>,
    table: &str,
) -> Result<Vec<i32>> {
    ensure_schema_migrations(conn, table).await?;

    let available_versions: Vec<i32> = available.iter().map(|m| m.version).collect();

    let applied = applied_versions(conn, table).await?;

    let missing: Vec<i32> = applied
        .iter()
        .copied()
        .filter(|v| !available_versions.contains(v))
        .collect();
    if !missing.is_empty() {
        let mut missing_sorted = missing;
        missing_sorted.sort_unstable();
        let max = *missing_sorted.last().expect("non-empty");
        return Err(PgError::UnknownMigrations {
            version: max,
            missing: missing_sorted,
        });
    }

    let pending: Vec<&Migration> = available
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }

    let mut newly_applied: Vec<i32> = Vec::with_capacity(pending.len());
    for migration in pending {
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "applying migration"
        );
        let started = std::time::Instant::now();
        if let Err(err) = apply_one(conn, migration, table).await {
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(err);
        }
        tracing::info!(
            version = migration.version,
            name = migration.name,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "migration applied"
        );
        newly_applied.push(migration.version);
    }
    Ok(newly_applied)
}

/// Marker of a statement run outside a transaction, alone on its line, the
/// statement running to the first line ending in `;`. One real need:
/// `CREATE INDEX CONCURRENTLY`, which Postgres refuses inside a transaction
/// block, and the only way to change a bm25 index without cutting search,
/// ParadeDB allowing no two bm25 indexes on a table through the ordinary path.
pub const SANS_TRANSACTION: &str = "-- lj:sans-transaction";

/// A stretch of a migration, as the runner applies it.
#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Transaction(&'a str),
    /// One statement marked [`SANS_TRANSACTION`], run in autocommit.
    SansTransaction(&'a str),
}

/// Cuts a migration on its [`SANS_TRANSACTION`] markers. A migration without
/// one is a single [`Segment::Transaction`], and a blank stretch yields none.
pub fn segments(sql: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let (mut start, mut pos, mut marked) = (0, 0, false);
    for line in sql.split_inclusive('\n') {
        let end = pos + line.len();
        if marked && line.trim_end().ends_with(';') {
            out.push(Segment::SansTransaction(&sql[start..end]));
            (start, marked) = (end, false);
        } else if !marked && line.trim_end() == SANS_TRANSACTION {
            if !sql[start..pos].trim().is_empty() {
                out.push(Segment::Transaction(&sql[start..pos]));
            }
            (start, marked) = (pos, true);
        }
        pos = end;
    }
    let rest = &sql[start..];
    if marked {
        out.push(Segment::SansTransaction(rest));
    } else if !rest.trim().is_empty() || out.is_empty() {
        out.push(Segment::Transaction(rest));
    }
    out
}

/// Applies a migration segment by segment, its version recorded with the last.
/// A migration cut in several is not atomic: an interruption leaves the
/// segments before applied and the version unrecorded.
async fn apply_one(conn: &Connection, migration: &Migration, table: &str) -> Result<()> {
    let segments = segments(migration.sql);
    let last = segments.len() - 1;
    for (i, segment) in segments.into_iter().enumerate() {
        let version = (i == last).then_some(migration.version);
        match segment {
            Segment::Transaction(sql) => apply_in_transaction(conn, sql, version, table).await?,
            Segment::SansTransaction(sql) => {
                apply_sans_transaction(conn, sql, version, table).await?
            }
        }
    }
    Ok(())
}

/// Applies SQL in an explicit transaction, the version recorded inside it.
async fn apply_in_transaction(
    conn: &Connection,
    sql: &str,
    version: Option<i32>,
    table: &str,
) -> Result<()> {
    conn.batch_execute("BEGIN").await?;
    // Some migrations rebuild large indexes, far past the `statement_timeout` the
    // pool arms on every connection. `SET LOCAL` ends with the transaction.
    conn.batch_execute("SET LOCAL statement_timeout = 0")
        .await?;
    // `lock_timeout` bounds the wait for a lock: a long index rebuild passes, a
    // DDL that cannot take its lock in 5 s fails. A migration runs with the server
    // up, and a DDL waiting behind a long query queues every later query on the table.
    conn.batch_execute("SET LOCAL lock_timeout = '5s'").await?;
    conn.batch_execute(sql).await?;
    if let Some(version) = version {
        record(conn, version, table).await?;
    }
    conn.batch_execute("COMMIT").await?;
    Ok(())
}

/// Applies a [`Segment::SansTransaction`]: the DDL, then the version, each in
/// autocommit.
///
/// - **One statement per segment**: a simple query carrying several is itself
///   a transaction block, where `CONCURRENTLY` is rejected.
/// - **No rollback**: an interrupted `CREATE INDEX CONCURRENTLY` leaves an
///   INVALID index and the version unrecorded. The version is recorded only
///   over a valid index, so a replay that `IF NOT EXISTS` turns into a no-op
///   fails too. Recovery is the operator's `DROP INDEX`.
/// - **No `lock_timeout`**: `CONCURRENTLY` takes only a `SHARE UPDATE
///   EXCLUSIVE`, queueing in front of nobody.
///
/// `SET LOCAL` has no scope outside a transaction: the `statement_timeout` is
/// set for the session then `RESET`, which restores the pool's, not the server
/// default.
async fn apply_sans_transaction(
    conn: &Connection,
    sql: &str,
    version: Option<i32>,
    table: &str,
) -> Result<()> {
    conn.batch_execute("SET statement_timeout = 0").await?;
    let ddl = conn.batch_execute(sql).await;
    conn.batch_execute("RESET statement_timeout").await?;
    ddl?;
    if let Some(index) = concurrent_index_name(sql) {
        let valid: Option<bool> = conn
            .query_one(
                "SELECT (SELECT indisvalid FROM pg_index WHERE indexrelid = to_regclass($1::text))",
                &[&index],
            )
            .await?
            .get(0);
        if valid != Some(true) {
            return Err(PgError::InvalidIndex(index.to_string()));
        }
    }
    if let Some(version) = version {
        record(conn, version, table).await?;
    }
    Ok(())
}

/// The index a `CREATE [UNIQUE] INDEX CONCURRENTLY [IF NOT EXISTS] <name>`
/// builds, comment lines skipped; `None` for any other statement.
fn concurrent_index_name(sql: &str) -> Option<&str> {
    let mut words = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .flat_map(str::split_whitespace)
        .peekable();
    let mut keyword = |kw: &str| words.next_if(|w| w.eq_ignore_ascii_case(kw)).is_some();
    if !keyword("CREATE") {
        return None;
    }
    keyword("UNIQUE");
    if !(keyword("INDEX") && keyword("CONCURRENTLY")) {
        return None;
    }
    if keyword("IF") && !(keyword("NOT") && keyword("EXISTS")) {
        return None;
    }
    words.next()
}

/// `applied_at` is `now()` on the server, independent of the client's clock.
async fn record(conn: &Connection, version: i32, table: &str) -> Result<()> {
    conn.execute(
        &format!("INSERT INTO {table} (version, applied_at) VALUES ($1, now())"),
        &[&version],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_concurrent_build_names_the_index_it_creates() {
        let name = concurrent_index_name;
        assert_eq!(
            name("-- lj:sans-transaction\n-- a comment\nCREATE INDEX CONCURRENTLY i\n    ON t (c);\n"),
            Some("i")
        );
        assert_eq!(
            name("create unique index concurrently if not exists public.i on t (c);"),
            Some("public.i")
        );
        assert_eq!(name("DROP INDEX CONCURRENTLY i;"), None);
        assert_eq!(name("CREATE INDEX i ON t (c);"), None);
        assert_eq!(name("REINDEX INDEX CONCURRENTLY i;"), None);
    }

    #[test]
    fn a_migration_without_marker_is_one_transaction() {
        let sql = "ALTER TABLE t ADD COLUMN c int;\nUPDATE t SET c = 1;\n";
        assert_eq!(segments(sql), vec![Segment::Transaction(sql)]);
        assert_eq!(segments(""), vec![Segment::Transaction("")]);
    }

    #[test]
    fn a_marked_file_is_one_statement_outside_a_transaction() {
        let sql = "-- lj:sans-transaction\nCREATE INDEX CONCURRENTLY i\n    ON t (c);\n";
        assert_eq!(segments(sql), vec![Segment::SansTransaction(sql)]);
    }

    #[test]
    fn a_marker_cuts_the_transactions_around_its_statement() {
        let sql =
            "-- 0001_a.sql\nCREATE TABLE t (c int);\n\n-- 0002_b.sql\n-- lj:sans-transaction\n\
                   CREATE INDEX CONCURRENTLY i\n    ON t (c);\n\n-- lj:sans-transaction\n\
                   DROP INDEX CONCURRENTLY j;\n\n-- 0004_d.sql\nDROP TABLE u;\n";
        assert_eq!(
            segments(sql),
            vec![
                Segment::Transaction("-- 0001_a.sql\nCREATE TABLE t (c int);\n\n-- 0002_b.sql\n"),
                Segment::SansTransaction(
                    "-- lj:sans-transaction\nCREATE INDEX CONCURRENTLY i\n    ON t (c);\n"
                ),
                Segment::SansTransaction("-- lj:sans-transaction\nDROP INDEX CONCURRENTLY j;\n"),
                Segment::Transaction("\n-- 0004_d.sql\nDROP TABLE u;\n"),
            ]
        );
    }
}
