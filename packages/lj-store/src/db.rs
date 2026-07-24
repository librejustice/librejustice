//! Pool de connexions Postgres (deadpool-postgres) + helpers (port de `db.py`).
//!
//! Côté Python, `db.py` ouvre soit une connexion psycopg unique (ingest CLI),
//! soit un `ConnectionPool` (serveur API), et enregistre l'adaptateur
//! `pgvector` via `register_vector` sur chaque connexion ouverte. En Rust,
//! le binding `vector` du crate `pgvector` (feature `postgres`) fonctionne par
//! lookup du nom de type au moment de la requête (`ty.name() == "vector"`) —
//! il n'y a donc pas d'enregistrement runtime à faire (cf.
//! [`register_extension_types`]). Le DSN libpq porte lui-même
//! `application_name` ; `build_pool` y ajoute `statement_timeout` (paramètre
//! `options`) et arme un `wait_timeout` sur le pool (parité `db.py`).

use crate::error::{Result, StoreError};
use deadpool_postgres::{Manager, ManagerConfig, Object, RecyclingMethod, Runtime};
// `Pool` fait partie du contrat public (`build_pool` le renvoie ; les pipelines
// concurrents le clonent pour donner une connexion par worker).
pub use deadpool_postgres::Pool;
use std::str::FromStr;
use std::time::Duration;
use tokio_postgres::NoTls;

/// Connexion checkout du pool (alias de l'objet deadpool).
pub type Connection = Object;

/// Délai max d'attente d'une connexion au pool avant échec (`pool.get()`).
/// Sans lui, deadpool attend **indéfiniment** : sous saturation, le pattern
/// hold-and-wait (une recherche tient N conns et en attend une N+1) fige le
/// process pour toujours. Parité du timeout d'acquisition par défaut de
/// `psycopg_pool` (30 s) que le port avait perdu. Le sémaphore de recherche
/// (`AppState.search_permits`) empêche d'atteindre la saturation sur le chemin
/// nominal ; ce timeout est le filet de sécurité (échec recouvrable plutôt que
/// gel) pour tout autre consommateur du pool.
const POOL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// `statement_timeout` posé côté serveur sur chaque connexion via le paramètre
/// de démarrage libpq `options`. Borne la durée qu'une requête (donc la
/// rétention de sa connexion) peut atteindre : au-delà, Postgres annule la
/// requête → la connexion retourne au pool. Parité directe du
/// `options="-c statement_timeout=30s"` de `db.py`.
const STATEMENT_TIMEOUT_OPTION: &str = "-c statement_timeout=30s";

/// Construit un pool deadpool depuis une DSN Postgres (forme libpq
/// `postgresql://user:pass@host:port/db?application_name=...`).
///
/// `pool_max` borne le nombre de connexions (cf. `Settings.pool_max`). Le
/// recyclage joue `BEGIN; ROLLBACK;` au checkout (`RecyclingMethod::Custom`) :
/// il clôt toute transaction laissée ouverte sur la connexion (future annulée
/// par le `try_join` d'une jambe sœur) avant de la rendre, et fait échouer le
/// recyclage d'une connexion en transaction **avortée** (jambe ANN `SET LOCAL
/// … ; <query>` annulée par `statement_timeout`) — `BEGIN` y est rejeté
/// (« current transaction is aborted ») → deadpool jette la connexion et en
/// ouvre une neuve. Sans cela une telle connexion — que
/// `RecyclingMethod::Fast` ne détecte pas et
/// qu'`idle_in_transaction_session_timeout` (0) ne tue jamais — serait
/// re-servie indéfiniment. Sur une connexion saine (cas nominal), la paire est
/// silencieuse : un `ROLLBACK` nu émettrait « WARNING: there is no transaction
/// in progress » à chaque checkout, loggé deux fois (serveur + relais notices
/// tokio_postgres) — mesuré à ~3 Go/jour de syslog en prod. Ne touche ni au
/// schéma de session ni au cache de requêtes préparées. C'est le
/// reset-sur-retour de `psycopg_pool`.
pub fn build_pool(dsn: &str, pool_max: usize) -> Result<Pool> {
    let mut pg_config = tokio_postgres::Config::from_str(dsn)
        .map_err(|e| StoreError::Pool(format!("DSN invalide: {e}")))?;

    // `statement_timeout` côté serveur (parité `db.py`). On préserve d'éventuelles
    // `options` déjà portées par la DSN en les concaténant (syntaxe libpq : flags
    // séparés par une espace) plutôt que de les écraser.
    let options = match pg_config.get_options() {
        Some(existing) => format!("{existing} {STATEMENT_TIMEOUT_OPTION}"),
        None => STATEMENT_TIMEOUT_OPTION.to_string(),
    };
    pg_config.options(&options);

    let mgr_config = ManagerConfig {
        recycling_method: RecyclingMethod::Custom("BEGIN; ROLLBACK;".to_string()),
    };
    let manager = Manager::from_config(pg_config, NoTls, mgr_config);

    // `runtime(Tokio1)` est requis pour que `wait_timeout` soit armé (deadpool
    // s'appuie sur le timer du runtime) ; sans lui `build()` échouerait.
    Pool::builder(manager)
        .max_size(pool_max)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(POOL_WAIT_TIMEOUT))
        .build()
        .map_err(|e| StoreError::Pool(format!("construction du pool: {e}")))
}

/// Enregistre les types d'extension custom (`vector`, `rabitq8`) sur la
/// connexion si nécessaire pour le binding pgvector.
///
/// No-op en Rust : le crate `pgvector` implémente `ToSql`/`FromSql` pour
/// `Vector` en acceptant par nom de type (`ty.name() == "vector"`), donc aucun
/// enregistrement runtime n'est requis — contrairement à psycopg, où
/// `register_vector(conn)` est obligatoire. Conservé dans l'API publique pour
/// rester aligné sur le contrat du workspace ; appelable sans effet par les
/// appelants qui ouvrent une connexion brute.
pub async fn register_extension_types(_conn: &Connection) -> Result<()> {
    Ok(())
}
