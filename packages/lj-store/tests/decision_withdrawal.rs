//! A decision withdrawn by hand stays down, whatever source writes at its address.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{DecisionRepository, ExtractedFields, JurisdictionRow};
use serde_json::json;
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 1, d).unwrap()
}

async fn base() -> (deadpool_postgres::Pool, Connection) {
    MIGRATED
        .get_or_init(|| async {
            let pool = build_pool(&url(), 1).unwrap();
            let conn = pool.get().await.unwrap();
            conn.batch_execute("SET statement_timeout = 0")
                .await
                .unwrap();
            apply_migrations(&conn, MigrationIntent::Apply)
                .await
                .unwrap();
            conn.batch_execute(
                "INSERT INTO jurisdiction (code, jurisdiction_type, label, source_code) \
                 VALUES ('ta_withdraw', 'TA', 'Tribunal administratif', 'ta_withdraw')",
            )
            .await
            .unwrap();
        })
        .await;
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    (pool, conn)
}

fn record(date: NaiveDate, docket: &str) -> ExtractedFields {
    ExtractedFields {
        jurisdiction: Some(JurisdictionRow {
            code: "ta_withdraw".into(),
            source_code: "ta_withdraw".into(),
            jurisdiction_type: "TA".into(),
            city: None,
            label: "Tribunal administratif".into(),
            place: None,
        }),
        date_lecture: Some(date),
        docket_numbers: vec![docket.into()],
        ..Default::default()
    }
}

async fn tombstoned(conn: &Connection, uid: &str) -> bool {
    conn.query_one(
        "SELECT deleted_at IS NOT NULL FROM decision_sources WHERE source_uid = $1",
        &[&uid],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_withdrawn_address_stays_down_until_lifted() {
    let (_pool, conn) = base().await;
    let uid = "TA_202401.zip/TA99/DTA_2400001_20240115.xml";
    let id: i64 = conn
        .query_one(
            "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, docket_numbers) \
             VALUES ('TA', 'ta_withdraw', 'withdraw-a', $1, ARRAY['2400001']) RETURNING id",
            &[&day(15)],
        )
        .await
        .unwrap()
        .get(0);
    let repo = DecisionRepository::new(&conn);
    repo.upsert_decision_source(id, uid, "a", "xml", &json!({}))
        .await
        .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, 'fr', 'Considérant ce qui suit', NULL)",
        &[&id, &uid],
    )
    .await
    .unwrap();

    assert_eq!(
        repo.withdraw("withdraw-a", "test").await.unwrap(),
        vec![uid.to_string()]
    );
    let row = conn
        .query_one(
            "SELECT deleted_at IS NOT NULL, (SELECT count(*) FROM decision_text WHERE decision_id = $1) \
             FROM decisions WHERE id = $1",
            &[&id],
        )
        .await
        .unwrap();
    assert!(row.get::<_, bool>(0));
    assert_eq!(row.get::<_, i64>(1), 0);

    assert!(repo
        .is_withdrawn(&record(day(15), "24-00001"))
        .await
        .unwrap());
    assert!(!repo
        .is_withdrawn(&record(day(16), "2400001"))
        .await
        .unwrap());
    repo.upsert_decision_source(id, uid, "b", "xml", &json!({}))
        .await
        .unwrap();
    assert!(tombstoned(&conn, uid).await);

    assert!(repo.lift_withdrawal("withdraw-a").await.unwrap());
    assert!(!repo
        .is_withdrawn(&record(day(15), "2400001"))
        .await
        .unwrap());
    repo.upsert_decision_source(id, uid, "c", "xml", &json!({}))
        .await
        .unwrap();
    assert!(!tombstoned(&conn, uid).await);
}
