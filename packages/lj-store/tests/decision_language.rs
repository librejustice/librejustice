//! A provenance whose text changes language leaves one text, and its lone decision takes it.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use lj_core::decision::Decision;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::DecisionRepository;
use serde_json::{json, Value};

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

async fn base() -> (deadpool_postgres::Pool, Connection) {
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .unwrap();
    apply_migrations(&conn, MigrationIntent::Apply)
        .await
        .unwrap();
    (pool, conn)
}

fn decision(uid: &str, text: &str) -> Decision {
    serde_json::from_value(json!({
        "source_uid": uid,
        "member_name": uid,
        "jurisdiction_type": "CA",
        "publication_codes": [],
        "listed_counsel": [],
        "themes": [],
        "texte_integral_raw": text,
        "texte_integral_clean": text,
        "sections": [],
        "metadata_header": "",
        "parse_warnings": [],
    }))
    .unwrap()
}

async fn texts(conn: &Connection, id: i64) -> (String, Vec<(String, String)>) {
    let lang = conn
        .query_one("SELECT lang FROM decisions WHERE id = $1", &[&id])
        .await
        .unwrap()
        .get(0);
    let rows = conn
        .query(
            "SELECT lang, text_status FROM decision_text WHERE decision_id = $1 ORDER BY lang",
            &[&id],
        )
        .await
        .unwrap();
    (lang, rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

async fn write(repo: &DecisionRepository<'_>, text: &str, checksum: &str, fields: Value) -> i64 {
    repo.upsert(
        &decision("juricaf/MAROC-COURDAPPEL-19690918-1", text),
        checksum,
        "language-a",
        None,
        None,
        &fields,
        "html",
        true,
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_provenance_that_changes_language_leaves_one_text_in_it() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);

    let id = write(
        &repo,
        "Synthèse de la décision",
        "a",
        json!({ "lang": "fra" }),
    )
    .await;
    assert_eq!(
        texts(&conn, id).await,
        ("fr".into(), vec![("fr".into(), "authentic".into())])
    );

    let again = write(&repo, "حكمت محكمة الاستئناف", "b", json!({ "lang": "ara" })).await;
    assert_eq!(again, id);
    assert_eq!(
        texts(&conn, id).await,
        ("ar".into(), vec![("ar".into(), "authentic".into())])
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_juportal_page_in_dutch_joins_the_decision_its_french_page_holds() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let ecli = "ECLI:BE:CASS:2015:ARR.20150130.7";
    let mut ids = Vec::new();
    for (uid, text, lang, public_id) in [
        (
            format!("juportal/{ecli}"),
            "La Cour rejette le pourvoi.",
            "fra",
            "juportal-fr",
        ),
        (
            format!("juportal/{ecli}/nl"),
            "Het Hof verwerpt het cassatieberoep.",
            "nld",
            "juportal-nl",
        ),
    ] {
        let written = repo
            .upsert(
                &decision(&uid, text),
                lang,
                public_id,
                None,
                None,
                &json!({ "lang": lang }),
                "html",
                false,
            )
            .await
            .unwrap();
        ids.push(written.id);
    }
    assert_eq!(ids[0], ids[1]);
    let langs: Vec<String> = conn
        .query(
            "SELECT lang FROM decision_text WHERE decision_id = $1 ORDER BY lang",
            &[&ids[0]],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(langs, ["fr", "nl"]);
}
