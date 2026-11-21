//! An article's copies of its text agree with it after every committed write of either.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{
    BodyWrite, DecisionRepository, Fraicheur, LegalArticleRow, LegalTextRow,
};

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

fn jour(iso: &str) -> NaiveDate {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap()
}

fn text(uid: &str, title: &str, nature: &str, date: &str) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "FR".to_string(),
        title: title.to_string(),
        title_key: title.to_lowercase(),
        nature: nature.to_string(),
        last_modified: None,
        date_texte: Some(jour(date)),
        date_publi: None,
        eli: None,
        nor: None,
        instrument_key: None,
        status: None,
    }
}

fn article(uid: &str, num: &str, checksum: u64) -> LegalArticleRow {
    LegalArticleRow {
        text_uid: uid.to_string(),
        num: num.to_string(),
        num_key: num.to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: Some(jour("2020-01-02")),
        date_fin: None,
        texte: Some(format!("Article {num}, version {checksum}.")),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: checksum,
        source: "jorf".to_string(),
        source_uid: format!("{uid}-{num}"),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

async fn exec(conn: &Connection, sql: &str) {
    conn.batch_execute(sql).await.unwrap();
}

async fn versions(conn: &Connection, uid: &str) -> Vec<String> {
    conn.query(
        "SELECT ctid::text FROM legal_article WHERE text_uid = $1 ORDER BY num_key",
        &[&uid],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| r.get(0))
    .collect()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn the_copies_follow_every_write_of_the_text_and_the_article() {
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    exec(&conn, "SET statement_timeout = 0").await;
    apply_migrations(&conn, MigrationIntent::Apply)
        .await
        .unwrap();
    let repo = DecisionRepository::new(&conn);
    let drift = || async { repo.denorm_derive().await.unwrap() };
    let (a, b) = ("JORFTEXTCOPIES1", "JORFTEXTCOPIES2");

    repo.upsert_legal_text(&text(
        a,
        "Décret relatif aux copies",
        "DECRET",
        "2020-01-01",
    ))
    .await
    .unwrap();
    for num in ["1", "2"] {
        repo.upsert_legal_article(&article(a, num, 1), Fraicheur::Autoritaire)
            .await
            .unwrap();
    }
    assert_eq!(drift().await, 0, "inserted articles");
    repo.upsert_legal_article(&article(a, "1", 2), Fraicheur::Autoritaire)
        .await
        .unwrap();
    assert_eq!(drift().await, 0, "an article upserted again");

    repo.upsert_legal_text(&text(
        a,
        "Arrêté relatif aux copies",
        "ARRETE",
        "2021-03-04",
    ))
    .await
    .unwrap();
    assert_eq!(drift().await, 0, "title, nature and date rewritten");
    exec(
        &conn,
        &format!("UPDATE legal_text SET slug = 'copies' WHERE text_uid = '{a}'"),
    )
    .await;
    assert_eq!(drift().await, 0, "slug");
    repo.write_text_bodies(
        "jorf",
        &[(a.to_string(), "Corps.".to_string())],
        BodyWrite::Replace,
    )
    .await
    .unwrap();
    exec(
        &conn,
        &format!("UPDATE legal_text SET status = 'ABROGE' WHERE text_uid = '{a}'"),
    )
    .await;
    assert_eq!(drift().await, 0, "status, read by the body piece");
    exec(
        &conn,
        &format!("UPDATE legal_text SET role = 'individuel' WHERE text_uid = '{a}'"),
    )
    .await;
    assert_eq!(drift().await, 0, "role");

    exec(
        &conn,
        &format!("UPDATE legal_article SET code_title = 'faux' WHERE text_uid = '{a}'"),
    )
    .await;
    assert_eq!(drift().await, 0, "a copy written by hand");

    repo.upsert_legal_article(&article(b, "1", 1), Fraicheur::Autoritaire)
        .await
        .unwrap();
    repo.upsert_legal_text(&text(
        b,
        "Décret reçu après ses articles",
        "DECRET",
        "2022-05-06",
    ))
    .await
    .unwrap();
    assert_eq!(drift().await, 0, "a text written after its articles");
    exec(
        &conn,
        &format!(
            "UPDATE legal_article SET text_uid = '{b}' WHERE text_uid = '{a}' AND num_key = '2'"
        ),
    )
    .await;
    assert_eq!(drift().await, 0, "an article moved to another text");

    let before = versions(&conn, a).await;
    exec(
        &conn,
        &format!(
            "BEGIN; UPDATE legal_text SET role = 'instrument' WHERE text_uid = '{a}'; \
             UPDATE legal_text SET role = 'individuel' WHERE text_uid = '{a}'; COMMIT"
        ),
    )
    .await;
    assert_eq!(
        versions(&conn, a).await,
        before,
        "a role reset and given back in one transaction"
    );
    repo.upsert_legal_text(&text(
        a,
        "Décret portant nomination",
        "DECRET",
        "2021-03-04",
    ))
    .await
    .unwrap();
    repo.classify_text_roles_scoped(&[a.to_string()])
        .await
        .unwrap();
    let before = versions(&conn, a).await;
    repo.classify_text_roles_scoped(&[a.to_string()])
        .await
        .unwrap();
    assert_eq!(
        versions(&conn, a).await,
        before,
        "roles classified again, unchanged"
    );
    assert_eq!(drift().await, 0, "roles classified");
}
