//! A text's versions in another language, replaced per text and language.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::build_pool;
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{
    ArticleLanguageWrite, DecisionRepository, Fraicheur, LegalArticleRow, LegalTextRow,
};

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2020, 1, 2).unwrap()
}

fn text(uid: &str) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "CH".to_string(),
        title: format!("Loi {uid}"),
        title_key: format!("loi {uid}"),
        nature: "LOI".to_string(),
        last_modified: None,
        date_texte: Some(day()),
        date_publi: None,
        eli: None,
        nor: None,
        instrument_key: None,
        status: None,
    }
}

fn article(uid: &str, num: &str) -> LegalArticleRow {
    LegalArticleRow {
        text_uid: uid.to_string(),
        num: num.to_string(),
        num_key: num.to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: Some(day()),
        date_fin: None,
        texte: Some(format!("Article {num} en français.")),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: 1,
        source: "fedlex".to_string(),
        source_uid: format!("{uid}-{num}"),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

fn german(article_id: i64, texte: &str) -> ArticleLanguageWrite {
    ArticleLanguageWrite {
        article_id,
        text_status: "official".to_string(),
        texte: texte.to_string(),
        title_path: Some("1. Titel".to_string()),
        content_checksum: 7,
    }
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_text_in_another_language_is_replaced_per_text_and_language() {
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .unwrap();
    apply_migrations(&conn, MigrationIntent::Apply)
        .await
        .unwrap();
    let repo = DecisionRepository::new(&conn);
    for uid in ["CHLANG1", "CHLANG2"] {
        repo.upsert_legal_text(&text(uid)).await.unwrap();
    }
    for (uid, num) in [("CHLANG1", "1"), ("CHLANG1", "2"), ("CHLANG2", "1")] {
        repo.upsert_legal_article(&article(uid, num), Fraicheur::Autoritaire)
            .await
            .unwrap();
    }
    let id = |uid: &'static str, num: &'static str| {
        let conn = &conn;
        async move {
            conn.query_one(
                "SELECT id FROM legal_article WHERE text_uid = $1 AND num_key = $2",
                &[&uid, &num],
            )
            .await
            .unwrap()
            .get::<_, i64>(0)
        }
    };
    let (one, two, other) = (
        id("CHLANG1", "1").await,
        id("CHLANG1", "2").await,
        id("CHLANG2", "1").await,
    );

    let both = [german(one, "Artikel 1."), german(two, "Artikel 2.")];
    assert_eq!(
        repo.replace_language_texts("CHLANG1", "de", &both)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        repo.replace_language_texts("CHLANG1", "de", &both)
            .await
            .unwrap(),
        0,
        "an unchanged text is not rewritten"
    );
    let read = repo.language_texts(&[one, two, other], "de").await.unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!(read[&one].texte, "Artikel 1.");
    assert!(repo.language_texts(&[one], "it").await.unwrap().is_empty());

    repo.replace_language_texts("CHLANG1", "de", &[german(two, "Artikel 2, neu.")])
        .await
        .unwrap();
    let read = repo.language_texts(&[one, two], "de").await.unwrap();
    assert!(
        !read.contains_key(&one),
        "a version left out loses its text"
    );
    assert_eq!(read[&two].texte, "Artikel 2, neu.");

    assert!(
        repo.replace_language_texts("CHLANG1", "de", &[german(other, "Fremd.")])
            .await
            .is_err(),
        "a version of another text"
    );
    assert!(
        repo.replace_language_texts("CHLANG1", "fr", &[german(one, "Doublon.")])
            .await
            .is_err(),
        "the language of texte"
    );
}
