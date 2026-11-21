//! A curated reading of an act becomes closed versions of the official text, and the official
//! text's next load keeps them.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_norm::text_aliases::TextAlias;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{DecisionRepository, Fraicheur, LegalArticleRow, LegalTextRow};
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

/// No surrounding transaction: the merge opens and commits its own.
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

fn day(iso: &str) -> NaiveDate {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap()
}

fn text(uid: &str, title: &str) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "FR".to_string(),
        title: title.to_string(),
        title_key: title.to_lowercase(),
        nature: "LOI".to_string(),
        last_modified: None,
        date_texte: Some(day("2006-01-28")),
        date_publi: None,
        eli: None,
        nor: None,
        instrument_key: None,
        status: None,
    }
}

fn article(uid: &str, num: &str, debut: Option<&str>, source: &str) -> LegalArticleRow {
    LegalArticleRow {
        text_uid: uid.to_string(),
        num: num.to_string(),
        num_key: num.to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: debut.map(day),
        date_fin: None,
        texte: Some(format!("{source} {num}")),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: 0,
        source: source.to_string(),
        source_uid: format!("{uid}#{num}@{debut:?}"),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

async fn seed(repo: &DecisionRepository<'_>, official: &str, curated: &str) {
    repo.upsert_legal_text(&text(official, "Loi portant code du travail"))
        .await
        .unwrap();
    repo.upsert_legal_text(&text(curated, "Code du travail"))
        .await
        .unwrap();
    for row in [
        article(official, "1", Some("2006-02-28"), "official"),
        article(official, "2", Some("2010-01-01"), "official"),
        article(curated, "1", None, "curated"),
        article(curated, "2", None, "curated"),
        article(curated, "3", None, "curated"),
    ] {
        repo.upsert_legal_article(&row, Fraicheur::Autoritaire)
            .await
            .unwrap();
    }
}

async fn curated_versions(conn: &Connection) -> Vec<(String, String, String, String)> {
    conn.query(
        "SELECT num_key, status, date_debut::text, date_fin::text FROM legal_article \
          WHERE text_uid = 'MERGEOFF1' AND source = 'curated' ORDER BY num_key",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
    .collect()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_curated_reading_closes_where_the_official_text_starts_and_outlives_its_reload() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    seed(&repo, "MERGEOFF1", "MERGECUR1").await;
    conn.execute(
        "UPDATE legal_text SET slug = 'merge-code-du-travail' WHERE text_uid = 'MERGECUR1'",
        &[],
    )
    .await
    .unwrap();

    let plan = repo
        .text_version_merge("MERGECUR1", "MERGEOFF1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            plan.from_articles,
            plan.into_articles,
            plan.shared_keys,
            plan.from_only_keys,
            plan.close,
            plan.empty_windows,
            plan.collisions
        ),
        (3, 2, 2, 1, Some(day("2006-02-28")), 0, 0)
    );

    let counts = repo
        .merge_legal_text_version(
            "MERGECUR1",
            "MERGEOFF1",
            &[TextAlias::title("fr", "Code du travail")],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counts.articles, 3);

    let closed = |num: &str, fin: &str| {
        (
            num.to_string(),
            "MODIFIE".to_string(),
            "0001-01-01".to_string(),
            fin.to_string(),
        )
    };
    let merged = vec![
        closed("1", "2006-02-28"),
        closed("2", "2010-01-01"),
        closed("3", "2006-02-28"),
    ];
    assert_eq!(curated_versions(&conn).await, merged);
    let redirect: String = conn
        .query_one(
            "SELECT text_uid FROM legal_text_slug_redirect WHERE ancien_slug = 'merge-code-du-travail'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(redirect, "MERGEOFF1");
    let alias: i64 = conn
        .query_one(
            "SELECT count(*) FROM legal_text_alias \
              WHERE text_uid = 'MERGEOFF1' AND kind = 'title' AND value = 'Code du travail'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(alias, 1);

    let purged = repo
        .delete_replaced_legal_articles("MERGEOFF1", &["official"])
        .await
        .unwrap();
    assert_eq!(purged, 2);
    assert_eq!(curated_versions(&conn).await, merged);
    assert!(repo
        .merge_legal_text_version("MERGECUR1", "MERGEOFF1", &[])
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_curated_row_the_official_text_already_dates_refuses_the_merge() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    seed(&repo, "MERGEOFF2", "MERGECUR2").await;
    repo.upsert_legal_article(
        &article("MERGECUR2", "1", Some("2006-02-28"), "curated"),
        Fraicheur::Autoritaire,
    )
    .await
    .unwrap();

    assert!(repo
        .merge_legal_text_version("MERGECUR2", "MERGEOFF2", &[])
        .await
        .is_err());
    let left: i64 = conn
        .query_one(
            "SELECT count(*) FROM legal_article WHERE text_uid = 'MERGECUR2'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 4);
}
