//! Which article versions `embed-norms` embeds: a searched norm with a text, once per checksum,
//! and again when its text is rewritten under the same checksum.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::build_pool;
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{
    ArticleTextDrift, ArticleTextWrite, DecisionRepository, Fraicheur, LegalArticleRow,
    LegalTextRow, NormPassageWrite,
};

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2020, 1, 2).unwrap()
}

fn article(num: &str, texte: &str, checksum: u64) -> LegalArticleRow {
    LegalArticleRow {
        text_uid: "JORFTEXTQUEUE1".to_string(),
        num: num.to_string(),
        num_key: num.to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: Some(day()),
        date_fin: None,
        texte: Some(texte.to_string()),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: checksum,
        source: "jorf".to_string(),
        source_uid: format!("queue-{num}"),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

fn passages(article_id: i64, checksum: i64, n: usize) -> NormPassageWrite {
    NormPassageWrite {
        article_id,
        content_checksum: checksum,
        embeddings: vec![vec![0.1; 512]; n],
    }
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_norm_version_is_embedded_once_per_text() {
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .unwrap();
    apply_migrations(&conn, MigrationIntent::Apply)
        .await
        .unwrap();
    let repo = DecisionRepository::new(&conn);
    repo.upsert_legal_text(&LegalTextRow {
        text_uid: "JORFTEXTQUEUE1".to_string(),
        jurisdiction: "FR".to_string(),
        title: "Décret relatif à la file".to_string(),
        title_key: "decret relatif a la file".to_string(),
        nature: "DECRET".to_string(),
        last_modified: None,
        date_texte: Some(day()),
        date_publi: None,
        eli: None,
        nor: None,
        instrument_key: None,
        status: None,
    })
    .await
    .unwrap();
    conn.batch_execute("UPDATE legal_text SET slug = 'file' WHERE text_uid = 'JORFTEXTQUEUE1'")
        .await
        .unwrap();
    for row in [
        article("1", "Le délai est d'un mois.", 1),
        article("2", "Le délai court de la notification.", 1),
        article("3", "A modifié les dispositions suivantes : article 1.", 1),
        article("4", "", 1),
    ] {
        repo.upsert_legal_article(&row, Fraicheur::Autoritaire)
            .await
            .unwrap();
    }
    let id = |num: &'static str| {
        let conn = &conn;
        async move {
            conn.query_one(
                "SELECT id FROM legal_article WHERE text_uid = 'JORFTEXTQUEUE1' AND num_key = $1",
                &[&num],
            )
            .await
            .unwrap()
            .get::<_, i64>(0)
        }
    };
    let (one, two) = (id("1").await, id("2").await);
    let ours = [one, two, id("3").await, id("4").await];
    let queue = || {
        let repo = &repo;
        async move {
            let mut queue = repo.norm_embed_queue().await.unwrap();
            queue.retain(|id| ours.contains(id));
            queue
        }
    };
    assert_eq!(
        queue().await,
        [one.max(two), one.min(two)],
        "the searched norms with a text, newest first"
    );

    let written = repo
        .replace_norm_passages(&[passages(one, 1, 2), passages(two, 1, 1)])
        .await
        .unwrap();
    assert_eq!(written, 3);
    assert!(queue().await.is_empty());

    repo.upsert_legal_article(
        &article("1", "Le délai est de deux mois.", 2),
        Fraicheur::Autoritaire,
    )
    .await
    .unwrap();
    assert_eq!(queue().await, [one], "a new checksum");

    repo.rewrite_article_texts(&[ArticleTextWrite {
        drift: ArticleTextDrift {
            text_uid: "JORFTEXTQUEUE1".to_string(),
            num_key: "2".to_string(),
            date_debut: day(),
            content_checksum: 1,
            texte: Some("Le délai court de la notification de la décision.".to_string()),
            nota: None,
        },
        cases: Vec::new(),
        cites: Vec::new(),
    }])
    .await
    .unwrap();
    assert_eq!(
        queue().await,
        [one.max(two), one.min(two)],
        "a text rewritten under its checksum"
    );
}
