use thiserror::Error;

#[derive(Debug, Error)]
pub enum PgError {
    #[error("postgres: {}", pg_detail(.0))]
    Postgres(#[from] tokio_postgres::Error),
    #[error("pool: {0}")]
    Pool(String),
    #[error("migration {version:04}: the database holds versions missing from disk: {missing:?}")]
    UnknownMigrations { version: i32, missing: Vec<i32> },
    #[error(
        "index {0} is not valid after its build: once no build of it runs, \
         DROP INDEX CONCURRENTLY {0} and migrate again"
    )]
    InvalidIndex(String),
}

/// A Postgres error made diagnosable, on one line.
///
/// The `Display` of `tokio_postgres::Error` prints only the category of its
/// `Kind`; SQLSTATE, server message, table and constraint live in `source()`.
/// Every site that logs a raw Postgres error goes through here.
pub fn pg_detail(err: &tokio_postgres::Error) -> String {
    if let Some(db) = err.as_db_error() {
        let mut out = format!("{} {} {}", db.severity(), db.code().code(), db.message());
        for (label, value) in [
            ("table", db.table()),
            ("column", db.column()),
            ("constraint", db.constraint()),
            ("detail", db.detail()),
            ("hint", db.hint()),
            ("where", db.where_()),
        ] {
            if let Some(v) = value {
                out.push_str(&format!(" [{label}={v}]"));
            }
        }
        return out.replace('\n', " ");
    }
    let mut out = err.to_string();
    let mut cause = std::error::Error::source(err);
    while let Some(e) = cause {
        out.push_str(&format!(": {e}"));
        cause = e.source();
    }
    out.replace('\n', " ")
}

/// The same for a deadpool checkout error, whose `Backend` variant carries a
/// Postgres error.
pub fn pool_detail(err: &deadpool_postgres::PoolError) -> String {
    match err {
        deadpool_postgres::PoolError::Backend(e) => format!("backend: {}", pg_detail(e)),
        other => other.to_string(),
    }
}

pub type Result<T> = std::result::Result<T, PgError>;
