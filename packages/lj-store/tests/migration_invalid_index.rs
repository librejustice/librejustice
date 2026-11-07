//! A concurrent index build is recorded only over a valid index.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use lj_pg::db::build_pool;
use lj_pg::error::PgError;
use lj_pg::migrator::{applied_versions, apply_migrations_with, Migration};

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_build_skipped_over_an_invalid_index_is_not_recorded() {
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    conn.batch_execute(
        "DROP TABLE IF EXISTS invalid_index_t; DROP TABLE IF EXISTS invalid_index_versions; \
         CREATE TABLE invalid_index_t (c int); INSERT INTO invalid_index_t VALUES (1), (1);",
    )
    .await
    .unwrap();
    // A unique build over duplicates fails and leaves its index INVALID.
    conn.batch_execute("CREATE UNIQUE INDEX CONCURRENTLY invalid_index_c ON invalid_index_t (c)")
        .await
        .unwrap_err();
    let replay = Migration {
        version: 1,
        name: "0001_invalid_index_c.sql",
        sql: "-- lj:sans-transaction\n\
              CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS invalid_index_c ON invalid_index_t (c);\n",
    };
    let err = apply_migrations_with(&conn, vec![replay.clone()], "invalid_index_versions")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, PgError::InvalidIndex(i) if i == "invalid_index_c"),
        "{err}"
    );
    assert!(applied_versions(&conn, "invalid_index_versions")
        .await
        .unwrap()
        .is_empty());

    conn.batch_execute("DROP INDEX CONCURRENTLY invalid_index_c")
        .await
        .unwrap();
    conn.batch_execute("DELETE FROM invalid_index_t WHERE ctid <> '(0,1)'")
        .await
        .unwrap();
    assert_eq!(
        apply_migrations_with(&conn, vec![replay], "invalid_index_versions")
            .await
            .unwrap(),
        vec![1]
    );
}
