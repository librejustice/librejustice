//! Scraper de métriques Postgres → OTLP (remplace le `postgresqlreceiver` du
//! collector, retiré du pipeline metrics).
//!
//! Le collector émettait via `postgresqlreceiver` un jeu de métriques (taille
//! des tables/index, taille de la base, vacuum, âge du WAL) que le row
//! « PostgreSQL » du cockpit Grafana interroge. On reproduit **les mêmes noms
//! et unités** que le receiver pour que, après conversion OTLP → Prometheus
//! côté Grafana Cloud (points → underscores, suffixe d'unité ajouté), on
//! retrouve `postgresql_db_size_bytes`, `postgresql_table_size_bytes`,
//! `postgresql_index_size_bytes`, `postgresql_table_vacuum_count` et
//! `postgresql_wal_age_seconds`.
//!
//! Modèle d'export : des observable gauges OTel dont le callback (synchrone)
//! lit un snapshot rafraîchi par une boucle async toutes les 60 s. 60 s = 1 DPM
//! = la limite incluse au free tier Grafana Cloud (`included_dpm_per_series`),
//! reprise telle quelle de l'ancien `collection_interval` du receiver.
//!
//! Point clé (parité `resource_to_telemetry_conversion` du collector) :
//! `postgresql.database.name`, `postgresql.table.name`, `postgresql.index.name`
//! sont posés comme **attributs de datapoint** sur chaque mesure — ils
//! deviennent des labels Prometheus. Posés comme attributs de `Resource`, ils
//! finiraient noyés dans la série `target_info` (une seule série globale, perte
//! de la dimension par table/index).

use deadpool_postgres::Pool;
use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

/// Période de scrape = ancien `collection_interval` du receiver (1 DPM free tier).
const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

/// Une mesure de taille (table ou index), portant ses labels de datapoint.
struct Sized {
    database: String,
    table: String,
    /// `Some` pour un index, `None` pour une table.
    index: Option<String>,
    bytes: i64,
}

/// Compteur de vacuum manuel par table.
struct Vacuum {
    database: String,
    table: String,
    count: i64,
}

/// Snapshot des métriques Postgres, rafraîchi par la boucle async et lu par les
/// callbacks des observable gauges.
#[derive(Default)]
struct PgStatsSnapshot {
    db_size: Vec<(String, i64)>,
    tables: Vec<Sized>,
    indexes: Vec<Sized>,
    vacuums: Vec<Vacuum>,
    /// Âge du WAL courant en secondes (un point par base, partagé instance-wide).
    wal_age_seconds: Option<f64>,
}

/// État global partagé entre la boucle de scrape et les callbacks OTel.
static SNAPSHOT: Mutex<Option<PgStatsSnapshot>> = Mutex::new(None);

/// Démarre le scraper : enregistre les observable gauges OTel, puis boucle de
/// rafraîchissement du snapshot toutes les 60 s. Conçu pour `tokio::spawn`.
///
/// Le `Meter` vient du `MeterProvider` global posé par `telemetry::init_telemetry`
/// (cf. A1) ; sans provider global, les instruments sont des no-op silencieux.
pub async fn run(pool: Pool) {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        "pg_metrics : scraper démarré"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        match scrape(&pool).await {
            Ok(snapshot) => {
                *SNAPSHOT.lock().unwrap() = Some(snapshot);
            }
            Err(err) => warn!(error = %err, "pg_metrics : scrape échoué (retry au prochain tick)"),
        }
    }
}

/// Enregistre les observable gauges. Chaque callback projette le dernier
/// snapshot en mesures, une par couple (table/index/db) avec ses labels.
fn register_gauges() {
    let meter = global::meter("librejustice-pg");

    // postgresql.db_size (By) → postgresql_db_size_bytes
    let _db_size = meter
        .i64_observable_gauge("postgresql.db_size")
        .with_description("The database disk usage.")
        .with_unit("By")
        .with_callback(|observer| {
            if let Some(snap) = SNAPSHOT.lock().unwrap().as_ref() {
                for (database, bytes) in &snap.db_size {
                    observer.observe(
                        *bytes,
                        &[KeyValue::new("postgresql.database.name", database.clone())],
                    );
                }
            }
        })
        .build();

    // postgresql.table.size (By) → postgresql_table_size_bytes
    let _table_size = meter
        .i64_observable_gauge("postgresql.table.size")
        .with_description("Disk space used by a table.")
        .with_unit("By")
        .with_callback(|observer| {
            if let Some(snap) = SNAPSHOT.lock().unwrap().as_ref() {
                for t in &snap.tables {
                    observer.observe(
                        t.bytes,
                        &[
                            KeyValue::new("postgresql.database.name", t.database.clone()),
                            KeyValue::new("postgresql.table.name", t.table.clone()),
                        ],
                    );
                }
            }
        })
        .build();

    // postgresql.index.size (By) → postgresql_index_size_bytes
    let _index_size = meter
        .i64_observable_gauge("postgresql.index.size")
        .with_description("The size of the index on disk.")
        .with_unit("By")
        .with_callback(|observer| {
            if let Some(snap) = SNAPSHOT.lock().unwrap().as_ref() {
                for i in &snap.indexes {
                    let index = i.index.as_deref().unwrap_or_default();
                    observer.observe(
                        i.bytes,
                        &[
                            KeyValue::new("postgresql.database.name", i.database.clone()),
                            KeyValue::new("postgresql.table.name", i.table.clone()),
                            KeyValue::new("postgresql.index.name", index.to_string()),
                        ],
                    );
                }
            }
        })
        .build();

    // postgresql.table.vacuum.count → postgresql_table_vacuum_count
    let _vacuum = meter
        .i64_observable_gauge("postgresql.table.vacuum.count")
        .with_description("Number of times a table has manually been vacuumed.")
        .with_callback(|observer| {
            if let Some(snap) = SNAPSHOT.lock().unwrap().as_ref() {
                for v in &snap.vacuums {
                    observer.observe(
                        v.count,
                        &[
                            KeyValue::new("postgresql.database.name", v.database.clone()),
                            KeyValue::new("postgresql.table.name", v.table.clone()),
                        ],
                    );
                }
            }
        })
        .build();

    // postgresql.wal.age (s) → postgresql_wal_age_seconds
    let _wal_age = meter
        .f64_observable_gauge("postgresql.wal.age")
        .with_description("Age of the oldest WAL file.")
        .with_unit("s")
        .with_callback(|observer| {
            if let Some(snap) = SNAPSHOT.lock().unwrap().as_ref() {
                if let Some(age) = snap.wal_age_seconds {
                    observer.observe(age, &[]);
                }
            }
        })
        .build();
}

/// Interroge Postgres et construit un snapshot complet.
async fn scrape(pool: &Pool) -> anyhow::Result<PgStatsSnapshot> {
    let conn = pool.get().await?;

    // Taille de chaque base non-template (parité `postgresql.db_size`).
    let db_rows = conn
        .query(
            "SELECT datname, pg_database_size(datname) \
             FROM pg_database WHERE datistemplate = false",
            &[],
        )
        .await?;
    let db_size = db_rows
        .iter()
        .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
        .collect();

    // Taille des tables utilisateur (`pg_relation_size`, comme le receiver).
    let table_rows = conn
        .query(
            "SELECT current_database(), \
                    schemaname || '.' || relname, \
                    pg_relation_size(relid) \
             FROM pg_stat_user_tables",
            &[],
        )
        .await?;
    let tables = table_rows
        .iter()
        .map(|r| Sized {
            database: r.get(0),
            table: r.get(1),
            index: None,
            bytes: r.get(2),
        })
        .collect();

    // Taille des index utilisateur (label `postgresql.table.name` sans schéma,
    // comportement par défaut du receiver hors feature gate `separateSchemaAttr`).
    let index_rows = conn
        .query(
            "SELECT current_database(), relname, indexrelname, \
                    pg_relation_size(indexrelid) \
             FROM pg_stat_user_indexes",
            &[],
        )
        .await?;
    let indexes = index_rows
        .iter()
        .map(|r| Sized {
            database: r.get(0),
            table: r.get(1),
            index: Some(r.get(2)),
            bytes: r.get(3),
        })
        .collect();

    // Nombre de vacuum manuels par table (`postgresql.table.vacuum.count`).
    let vacuum_rows = conn
        .query(
            "SELECT current_database(), \
                    schemaname || '.' || relname, \
                    vacuum_count \
             FROM pg_stat_user_tables",
            &[],
        )
        .await?;
    let vacuums = vacuum_rows
        .iter()
        .map(|r| Vacuum {
            database: r.get(0),
            table: r.get(1),
            count: r.get(2),
        })
        .collect();

    // Âge du WAL : secondes écoulées depuis le dernier checkpoint réussi
    // (proxy de `postgresql.wal.age`, qui mesure l'âge du plus vieux segment).
    let wal_row = conn
        .query_one(
            "SELECT EXTRACT(EPOCH FROM (now() - checkpoint_time))::float8 \
             FROM pg_control_checkpoint()",
            &[],
        )
        .await?;
    let wal_age_seconds = wal_row.get::<_, Option<f64>>(0);

    Ok(PgStatsSnapshot {
        db_size,
        tables,
        indexes,
        vacuums,
        wal_age_seconds,
    })
}
