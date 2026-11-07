use crate::error::{PgError, Result};
pub use deadpool_postgres::Pool;
use deadpool_postgres::{Manager, ManagerConfig, Object, RecyclingMethod, Runtime};
use std::str::FromStr;
use std::time::Duration;
use tokio_postgres::NoTls;

pub type Connection = Object;

/// How long `pool.get()` waits for a connection before failing.
const POOL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A wait past this leaves a line. At rest the pool serves in microseconds: a
/// visible wait is already a queue.
const POOL_WAIT_LOG: Duration = Duration::from_millis(250);

/// Session options, set server-side through libpq's `options` startup parameter.
/// A connection keeps the `search_path` it opened with: a schema may be listed
/// here before it exists, which is harmless.
const SESSION_OPTIONS: &str = "-c search_path=public,paradedb,comptes \
                               -c statement_timeout=30s -c jit=off \
                               -c client_min_messages=error";

/// Injects `password` into a DSN that carries none: `tokio-postgres` does not
/// read `PGPASSWORD`, unlike libpq, and the password lives in its own variable
/// rather than in every URL.
pub fn dsn_with_password(dsn: &str, password: Option<&str>) -> String {
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return dsn.to_string();
    };
    let Some((scheme, apres)) = dsn.split_once("://") else {
        return dsn.to_string();
    };
    let (netloc, reste) = match apres.find(['/', '?', '#']) {
        Some(i) => (&apres[..i], &apres[i..]),
        None => (apres, ""),
    };
    let (userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((info, hp)) => (info, hp),
        None => return dsn.to_string(),
    };
    if userinfo.contains(':') {
        return dsn.to_string();
    }
    format!("{scheme}://{userinfo}:{password}@{hostport}{reste}")
}

/// Binary name, used as `application_name`: without it `pg_stat_activity`
/// reads `[unknown]` and maintenance cannot tell its connections from others'.
fn binary_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "lj".to_string())
}

/// Builds a pool from a libpq DSN
/// (`postgresql://user:pass@host:port/db?application_name=...`).
pub fn build_pool(dsn: &str, pool_max: usize) -> Result<Pool> {
    let mut pg_config = tokio_postgres::Config::from_str(dsn)
        .map_err(|e| PgError::Pool(format!("invalid DSN: {e}")))?;

    let options = match pg_config.get_options() {
        Some(existing) => format!("{existing} {SESSION_OPTIONS}"),
        None => SESSION_OPTIONS.to_string(),
    };
    pg_config.options(&options);
    if pg_config.get_application_name().is_none() {
        pg_config.application_name(binary_name());
    }

    let mgr_config = ManagerConfig {
        recycling_method: RecyclingMethod::Custom("BEGIN; ROLLBACK;".to_string()),
    };
    let manager = Manager::from_config(pg_config, NoTls, mgr_config);

    Pool::builder(manager)
        .max_size(pool_max)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(POOL_WAIT_TIMEOUT))
        .build()
        .map_err(|e| PgError::Pool(format!("building the pool: {e}")))
}

/// `pool.get()` returning the measured wait, logged past [`POOL_WAIT_LOG`].
/// Without it, [`POOL_WAIT_TIMEOUT`] lets a request wait up to 30 s with no
/// trace telling the queue from a slow handler.
pub async fn get_timed(pool: &Pool) -> Result<(Connection, Duration)> {
    let started = std::time::Instant::now();
    let conn = pool
        .get()
        .await
        .map_err(|e| PgError::Pool(format!("{e}")))?;
    let attente = started.elapsed();
    if attente >= POOL_WAIT_LOG {
        let ms = attente.as_millis();
        tracing::warn!(
            pool_wait_ms = ms as u64,
            "waited {ms} ms for a pool connection"
        );
    }
    Ok((conn, attente))
}
