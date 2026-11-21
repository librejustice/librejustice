//! A text's body is its `body` piece in `legal_article`; needs `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{BodyWrite, DecisionRepository, LegalTextRow};
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

/// No surrounding transaction: the denormalisation refresh opens and commits its own.
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
        })
        .await;
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    (pool, conn)
}

fn circulaire(uid: &str, signed: &str, status: &str) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "FR".to_string(),
        title: format!("Circulaire {uid}"),
        title_key: uid.to_lowercase(),
        nature: "CIRCULAIRE".to_string(),
        last_modified: None,
        date_texte: NaiveDate::parse_from_str(signed, "%Y-%m-%d").ok(),
        date_publi: None,
        eli: None,
        nor: None,
        instrument_key: None,
        status: Some(status.to_string()),
    }
}

/// The text's `body` pieces: date, text, searchable.
async fn pieces(conn: &Connection, uid: &str) -> Vec<(String, String, Option<bool>)> {
    conn.query(
        "SELECT date_debut::text, texte, searchable FROM legal_article \
         WHERE text_uid = $1 AND num_key = 'body' AND num = '' ORDER BY date_debut",
        &[&uid],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2)))
    .collect()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_body_is_one_piece_dated_by_its_text() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let uid = "BODYCIR1";
    repo.upsert_legal_text(&circulaire(uid, "2020-03-01", "VIGUEUR"))
        .await
        .unwrap();
    conn.execute(
        "UPDATE legal_text SET slug = 'body-cir-1' WHERE text_uid = $1",
        &[&uid],
    )
    .await
    .unwrap();
    let body = |t: &str| vec![(uid.to_string(), t.to_string())];

    repo.write_text_bodies("circulaire", &body("Résumé."), BodyWrite::IfAbsent)
        .await
        .unwrap();
    assert_eq!(
        pieces(&conn, uid).await,
        [("2020-03-01".into(), "Résumé.".into(), Some(true))]
    );

    repo.write_text_bodies("circulaire", &body("Le texte du PDF."), BodyWrite::Replace)
        .await
        .unwrap();
    assert_eq!(pieces(&conn, uid).await[0].1, "Le texte du PDF.");

    repo.write_text_bodies("circulaire", &body("Résumé."), BodyWrite::IfAbsent)
        .await
        .unwrap();
    assert_eq!(pieces(&conn, uid).await[0].1, "Le texte du PDF.");

    repo.upsert_legal_text(&circulaire(uid, "2020-04-01", "VIGUEUR"))
        .await
        .unwrap();
    repo.write_text_bodies("circulaire", &body("Le texte du PDF."), BodyWrite::Replace)
        .await
        .unwrap();
    assert_eq!(
        pieces(&conn, uid).await,
        [("2020-04-01".into(), "Le texte du PDF.".into(), Some(true))]
    );

    repo.set_legal_texts_status("CIRCULAIRE", &[uid.to_string()], "ABROGE")
        .await
        .unwrap();
    assert_eq!(pieces(&conn, uid).await[0].2, Some(false));
    assert_eq!(repo.denorm_derive().await.unwrap(), 0);

    let summary = repo.law_code_summary(uid).await.unwrap().unwrap();
    assert_eq!(
        (summary.article_count, summary.body.as_deref()),
        (0, Some("Le texte du PDF."))
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_text_with_no_date_takes_no_piece() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let mut row = circulaire("BODYCIR2", "2020-03-01", "VIGUEUR");
    row.date_texte = None;
    repo.upsert_legal_text(&row).await.unwrap();
    let err = repo
        .write_text_bodies(
            "circulaire",
            &[("BODYCIR2".to_string(), "Sans date.".to_string())],
            BodyWrite::Replace,
        )
        .await;
    assert!(err.is_err());
    assert!(pieces(&conn, "BODYCIR2").await.is_empty());
}
