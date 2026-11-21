//! A JO text that names its addressees leaves the norms for its publication.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{BodyWrite, DecisionRepository, LegalTextRow};
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

/// No surrounding transaction: the move opens and commits its own.
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

fn text(uid: &str, title: &str) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: "FR".to_string(),
        title: title.to_string(),
        title_key: uid.to_lowercase(),
        nature: "DECRET".to_string(),
        last_modified: None,
        date_texte: chrono::NaiveDate::from_ymd_opt(1978, 4, 27),
        date_publi: None,
        eli: None,
        nor: Some("MOVX7800001D".to_string()),
        instrument_key: None,
        status: None,
    }
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_moved_text_leaves_its_slugs_nature_and_nor_to_its_notice() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let measure = "Décret du 27 avril 1978 M. BLANC MARCEL EST NOMME PREFET";
    repo.upsert_legal_text(&text("MOVETEXT1", measure))
        .await
        .unwrap();
    repo.upsert_legal_text(&text("MOVETEXT2", measure))
        .await
        .unwrap();
    repo.write_text_bodies(
        "jorf",
        &[("MOVETEXT2".to_string(), "Le préfet est nommé.".to_string())],
        BodyWrite::Replace,
    )
    .await
    .unwrap();
    repo.upsert_legal_text(&text(
        "MOVETEXT3",
        "Décret du 3 mai 1978 relatif aux crédits",
    ))
    .await
    .unwrap();
    for (uid, slug) in [
        ("MOVETEXT1", "move-decret-blanc"),
        ("MOVETEXT2", "move-decret-blanc-corps"),
        ("MOVETEXT3", "move-decret-credits"),
    ] {
        conn.execute(
            "UPDATE legal_text SET slug = $2 WHERE text_uid = $1",
            &[&uid, &slug],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO text_publication (text_uid, language, status, journal_code, date_publi) \
             VALUES ($1, 'fr', 'authentic', 'JORF', '1978-04-28')",
            &[&uid],
        )
        .await
        .unwrap();
    }
    conn.batch_execute(
        "INSERT INTO legal_text_slug_redirect (ancien_slug, text_uid, jurisdiction) \
           VALUES ('move-ancien-decret-blanc', 'MOVETEXT1', 'FR'); \
         INSERT INTO legal_link (owner_text_uid, owner_num_key, owner_date_debut, seq, \
           typelien, verb, direction, target_kind, target_text_uid, target_label) \
           VALUES ('MOVETEXT1', '', '1978-04-27', 0, 'CITATION', 'cite', 'outgoing', \
           'texte', 'MOVETEXT3', 'Décret')",
    )
    .await
    .unwrap();

    let candidates: Vec<String> = repo
        .journal_move_candidates(
            lj_vocab::PROTECTED_ACCESS_MARKER,
            lj_norm::individual::INDIVIDUAL_MEASURE,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.text_uid)
        .filter(|uid| uid.starts_with("MOVETEXT"))
        .collect();
    assert_eq!(candidates, ["MOVETEXT1"]);

    let trace = repo.journal_move_trace(&candidates).await.unwrap();
    let line: serde_json::Value = serde_json::from_str(&trace[0]).unwrap();
    assert_eq!(line["legal_text"]["slug"], "move-decret-blanc");
    assert_eq!(line["legal_link"][0]["target_text_uid"], "MOVETEXT3");
    assert_eq!(
        line["legal_text_slug_redirect"][0]["ancien_slug"],
        "move-ancien-decret-blanc"
    );

    let moved = repo.move_to_journal(&candidates, &[]).await.unwrap();
    assert_eq!(
        (moved.publications, moved.slugs, moved.links, moved.texts),
        (1, 2, 1, 1)
    );
    for slug in ["move-decret-blanc", "move-ancien-decret-blanc"] {
        assert_eq!(
            repo.notice_of_slug(slug).await.unwrap().as_deref(),
            Some("jorf:MOVETEXT1")
        );
    }
    assert!(repo
        .moved_text_uids("jorf")
        .await
        .unwrap()
        .contains("MOVETEXT1"));
    let publication = conn
        .query_one(
            "SELECT nature, nor, date_texte::text FROM text_publication WHERE text_uid = 'MOVETEXT1'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        (
            publication.get::<_, Option<String>>(0),
            publication.get::<_, Option<String>>(1),
            publication.get::<_, Option<String>>(2)
        ),
        (
            Some("DECRET".to_string()),
            Some("MOVX7800001D".to_string()),
            Some("1978-04-27".to_string())
        )
    );
    let left: i64 = conn
        .query_one(
            "SELECT count(*) FROM legal_text WHERE text_uid IN ('MOVETEXT1', 'MOVETEXT2', 'MOVETEXT3')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 2);
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_moved_measure_leaves_its_articles_and_body_to_its_publication() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let title = "Arrêté du 2 mai 1978 portant nomination";
    for uid in ["MOVEBODY1", "MOVEBODY2", "MOVEBODY3"] {
        repo.upsert_legal_text(&text(uid, title)).await.unwrap();
    }
    repo.upsert_legal_text(&LegalTextRow {
        jurisdiction: "DJ".to_string(),
        ..text(
            "MOVEBODY4",
            "Décision n° 12 du 2 mai 1978 portant nomination",
        )
    })
    .await
    .unwrap();
    repo.write_text_bodies(
        "jorf",
        &[("MOVEBODY3".to_string(), "M. Noir est nommé.".to_string())],
        BodyWrite::Replace,
    )
    .await
    .unwrap();
    conn.batch_execute(
        "UPDATE legal_text SET slug = lower(text_uid), role = 'individuel' \
           WHERE text_uid IN ('MOVEBODY1', 'MOVEBODY2', 'MOVEBODY3', 'MOVEBODY4'); \
         INSERT INTO text_publication (text_uid, language, status, journal_code, date_publi, \
           structure, articles) VALUES \
           ('MOVEBODY1', 'fr', 'authentic', 'JORF', '1978-05-03', \
            '[{\"num\": \"1\", \"article\": \"JORFARTI9\"}, {\"article\": \"JORFARTI2\"}]', \
            '{\"JORFARTI2\": {\"text\": \"As printed.\"}}'), \
           ('MOVEBODY2', 'fr', 'authentic', 'JORF', '1978-05-03', NULL, '{}'), \
           ('MOVEBODY3', 'fr', 'authentic', 'JORF', '1978-05-03', NULL, '{}'), \
           ('MOVEBODY4', 'fr', 'authentic', 'JO-DJ', '1978-05-03', NULL, '{}'); \
         INSERT INTO legal_article (text_uid, num, num_key, status, date_debut, texte, \
           content_checksum, source, source_uid, translation) VALUES \
           ('MOVEBODY1', '1', '1', '', '1978-05-03', 'M. Blanc est nommé.', 0, 'jorf', \
            'JORFARTI9', 'officiel'), \
           ('MOVEBODY1', '', 'jorfarti2', '', '1978-05-03', 'As stored.', 0, 'jorf', \
            'JORFARTI2', 'officiel'), \
           ('MOVEBODY2', '1', '1', '', '1978-05-03', 'M. Gris est nommé.', 0, 'legifrance', \
            'LEGIARTI5', 'officiel'), \
           ('MOVEBODY4', '1', '1', '', '1978-05-03', 'M. Roux est nommé.', 0, \
            'journalofficiel-dj', 'DJ-12-1', 'officiel')",
    )
    .await
    .unwrap();

    let measures: Vec<(String, bool)> = repo
        .journal_move_measures()
        .await
        .unwrap()
        .into_iter()
        .filter(|(c, _)| c.text_uid.starts_with("MOVEBODY"))
        .map(|(c, consolidated)| (c.text_uid, consolidated))
        .collect();
    assert_eq!(
        measures,
        [
            ("MOVEBODY1".to_string(), false),
            ("MOVEBODY2".to_string(), true),
            ("MOVEBODY3".to_string(), false)
        ]
    );

    let uids = ["MOVEBODY1", "MOVEBODY3", "MOVEBODY4"].map(str::to_string);
    let trace = repo.journal_move_trace(&uids).await.unwrap();
    let line: serde_json::Value = serde_json::from_str(&trace[0]).unwrap();
    assert_eq!(line["legal_article"].as_array().map(Vec::len), Some(2));

    let moved = repo.move_to_journal(&uids, &[]).await.unwrap();
    assert_eq!((moved.articles, moved.bodies, moved.texts), (4, 1, 3));
    let publications = conn
        .query(
            "SELECT articles, body FROM text_publication \
             WHERE text_uid IN ('MOVEBODY1', 'MOVEBODY3', 'MOVEBODY4') ORDER BY text_uid",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        publications[0].get::<_, serde_json::Value>(0),
        serde_json::json!({
            "JORFARTI9": {"text": "M. Blanc est nommé.", "num": "1"},
            "JORFARTI2": {"text": "As printed."}
        })
    );
    assert_eq!(
        publications[1].get::<_, Option<String>>(1).as_deref(),
        Some("M. Noir est nommé.")
    );
    assert_eq!(
        publications[2].get::<_, serde_json::Value>(0),
        serde_json::json!({"DJ-12-1": {"text": "M. Roux est nommé.", "num": "1"}})
    );
    let left: i64 = conn
        .query_one(
            "SELECT count(*) FROM legal_article WHERE text_uid IN ('MOVEBODY1', 'MOVEBODY3', 'MOVEBODY4')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0);
    assert_eq!(
        repo.notice_of_slug("movebody1").await.unwrap().as_deref(),
        Some("jorf:MOVEBODY1")
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_moved_ruling_leads_to_the_decision_of_its_docket_and_date() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let title = "Décision n° 2017-990 QPC du 19 octobre 2017";
    repo.upsert_legal_text(&LegalTextRow {
        nature: "DECISION".to_string(),
        date_texte: chrono::NaiveDate::from_ymd_opt(2017, 10, 19),
        ..text("MOVERULING1", title)
    })
    .await
    .unwrap();
    conn.batch_execute(
        "UPDATE legal_text SET slug = 'move-qpc-2017-990' WHERE text_uid = 'MOVERULING1'; \
         INSERT INTO text_publication (text_uid, language, status, journal_code, date_publi) \
           VALUES ('MOVERULING1', 'fr', 'authentic', 'JORF', '2017-10-21'); \
         INSERT INTO jurisdiction (code, jurisdiction_type, label, source_code) \
           VALUES ('constit', 'CC', 'Conseil constitutionnel', 'constit') ON CONFLICT DO NOTHING; \
         INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, \
           docket_numbers) VALUES \
           ('CC', 'constit', 'moveRulingOther', '2017-06-02', '{2017-990}'), \
           ('CC', 'constit', 'moveRulingSame', '2017-10-19', '{2017-990}')",
    )
    .await
    .unwrap();
    let (court, docket) = lj_norm::individual::printed_ruling(title).unwrap();
    let rulings = repo
        .printed_rulings(&[("MOVERULING1".to_string(), court, docket)])
        .await
        .unwrap();
    let moved = repo
        .move_to_journal(&["MOVERULING1".to_string()], &rulings)
        .await
        .unwrap();
    assert_eq!((moved.slugs, moved.texts), (1, 1));
    assert_eq!(
        repo.notice_of_slug("move-qpc-2017-990").await.unwrap(),
        None
    );
    assert_eq!(
        repo.decision_of_moved_slug("move-qpc-2017-990")
            .await
            .unwrap()
            .map(|(_, _, public_id)| public_id)
            .as_deref(),
        Some("moveRulingSame")
    );
}
