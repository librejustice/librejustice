//! A text's date and jurisdiction paths, stamped by every writer and copied onto
//! its articles. Against an empty Postgres with ParadeDB and VectorChord,
//! `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{
    BodyWrite, DecisionRepository, Fraicheur, LegalArticleRow, LegalTextRow,
};
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

/// A connection inside a transaction the test never commits.
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
    conn.batch_execute("BEGIN").await.unwrap();
    (pool, conn)
}

fn jour(iso: &str) -> NaiveDate {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap()
}

fn text(
    uid: &str,
    jurisdiction: &str,
    nature: &str,
    texte: Option<&str>,
    publi: Option<&str>,
) -> LegalTextRow {
    LegalTextRow {
        text_uid: uid.to_string(),
        jurisdiction: jurisdiction.to_string(),
        title: uid.to_string(),
        title_key: uid.to_lowercase(),
        nature: nature.to_string(),
        last_modified: None,
        date_texte: texte.map(jour),
        date_publi: publi.map(jour),
        eli: None,
        nor: None,
        instrument_key: None,
        status: None,
    }
}

async fn stamp(conn: &Connection, uid: &str) -> (Option<NaiveDate>, Option<Vec<String>>) {
    let r = conn
        .query_one(
            "SELECT text_date, jurisdiction_paths FROM legal_text WHERE text_uid = $1",
            &[&uid],
        )
        .await
        .unwrap();
    (r.get(0), r.get(1))
}

fn paths(p: &[&str]) -> Option<Vec<String>> {
    Some(p.iter().map(|s| s.to_string()).collect())
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_sentinel_text_date_falls_back_on_the_publication() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    repo.upsert_legal_text(&text(
        "JORFTEXT1",
        "FR",
        "LOI",
        Some("2999-01-01"),
        Some("2026-03-02"),
    ))
    .await
    .unwrap();
    assert_eq!(
        stamp(&conn, "JORFTEXT1").await,
        (Some(jour("2026-03-02")), paths(&["FR", "FR.LOI"]))
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_law_without_a_publication_date_takes_its_jo_issue() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    repo.upsert_legal_text(&text("JORFTEXT4", "FR", "LOI_ORGANIQUE", None, None))
        .await
        .unwrap();
    conn.execute(
        "INSERT INTO text_publication (text_uid, language, status, date_publi) \
         VALUES ('JORFTEXT4', 'fr', 'authentic', '2026-08-25')",
        &[],
    )
    .await
    .unwrap();
    assert!(repo
        .date_texts_from_publications()
        .await
        .unwrap()
        .contains(&"JORFTEXT4".to_string()));
    assert_eq!(
        stamp(&conn, "JORFTEXT4").await,
        (
            Some(jour("2026-08-25")),
            paths(&["FR", "FR.LOI", "FR.LOI.ORGANIQUE"])
        )
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn an_undated_instrument_takes_its_earliest_decree_and_follows_its_order() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    repo.upsert_legal_text(&text("INTL/T", "INTL", "TRAITE", None, None))
        .await
        .unwrap();
    for (uid, publi) in [("JORFTEXT2", "1990-05-04"), ("JORFTEXT3", "1989-01-02")] {
        repo.upsert_legal_text(&text(uid, "FR", "DECRET", None, Some(publi)))
            .await
            .unwrap();
    }
    for (seq, vehicle) in [(0i32, "JORFTEXT2"), (1, "JORFTEXT3")] {
        conn.execute(
            "INSERT INTO legal_link (owner_text_uid, owner_num_key, owner_date_debut, seq, \
               typelien, verb, direction, target_kind, target_text_uid, target_label) \
             VALUES ('INTL/T', '', '2999-01-01', $1, 'PUBLICATION', 'publie', 'incoming', \
               'texte', $2, 'Décret')",
            &[&seq, &vehicle],
        )
        .await
        .unwrap();
    }
    let article = LegalArticleRow {
        text_uid: "INTL/T".to_string(),
        num: "1".to_string(),
        num_key: "1".to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: None,
        date_fin: None,
        texte: Some("Article premier".to_string()),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: 1,
        source: "treaty".to_string(),
        source_uid: "T-1".to_string(),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    };
    repo.upsert_legal_article(&article, Fraicheur::Autoritaire)
        .await
        .unwrap();

    let dated = repo
        .date_instruments_from_vehicles(Some(&["JORFTEXT2".to_string()]))
        .await
        .unwrap();
    assert_eq!(dated, ["INTL/T"]);
    assert_eq!(
        stamp(&conn, "INTL/T").await,
        (Some(jour("1990-05-04")), paths(&["INTL", "INTL.TREATY"]))
    );

    repo.set_text_jurisdiction("INTL/T", "EU").await.unwrap();
    repo.refresh_article_denorm(Some(&["INTL/T".to_string()]))
        .await
        .unwrap();
    let r = conn
        .query_one(
            "SELECT text_date, jurisdiction_paths FROM legal_article WHERE text_uid = 'INTL/T'",
            &[],
        )
        .await
        .unwrap();
    let copied: (Option<NaiveDate>, Option<Vec<String>>) = (r.get(0), r.get(1));
    assert_eq!(
        copied,
        (Some(jour("1990-05-04")), paths(&["EU", "EU.TREATY"]))
    );
}

fn version(uid: &str, num: &str, debut: Option<&str>) -> LegalArticleRow {
    LegalArticleRow {
        text_uid: uid.to_string(),
        num: num.to_string(),
        num_key: num.to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut: debut.map(jour),
        date_fin: None,
        texte: Some("La prévention des infections nosocomiales.".to_string()),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: 1,
        source: "legifrance".to_string(),
        source_uid: format!("{uid}-{num}-{}", debut.unwrap_or("?")),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

type Listed = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static [Option<&'static str>],
);

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn the_bounds_keep_what_entered_into_force_between_them() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    // (uid, order, nature, own date, slug, versions)
    let texts: [Listed; 8] = [
        (
            "BOUNDS/CODE",
            "FR",
            "CODE",
            None,
            "code-t",
            &[Some("2010-01-01"), Some("2026-01-01")],
        ),
        (
            "BOUNDS/A",
            "FR",
            "LOI",
            Some("2026-03-02"),
            "loi-a",
            &[Some("2026-03-03")],
        ),
        (
            "BOUNDS/B",
            "FR",
            "LOI_ORGANIQUE",
            Some("2025-01-10"),
            "loi-b",
            &[Some("2025-01-11")],
        ),
        (
            "BOUNDS/D",
            "FR",
            "LOI",
            Some("2019-05-05"),
            "loi-d",
            &[Some("2019-05-06"), Some("2026-02-01")],
        ),
        ("BOUNDS/E", "FR", "LOI", None, "loi-e", &[]),
        (
            "BOUNDS/F",
            "EU",
            "DIRECTIVE_EURO",
            Some("2026-02-02"),
            "directive-f",
            &[None],
        ),
        (
            "BOUNDS/G",
            "FR",
            "LOI",
            Some("2026-05-05"),
            "loi-g",
            &[Some("2026-05-06")],
        ),
        (
            "BOUNDS/H",
            "FR",
            "DECRET",
            Some("2026-04-01"),
            "decret-h",
            &[],
        ),
    ];
    let mut uids = Vec::new();
    for (uid, jurisdiction, nature, date, slug, versions) in texts {
        repo.upsert_legal_text(&text(uid, jurisdiction, nature, date, None))
            .await
            .unwrap();
        if versions.is_empty() && date.is_some() {
            repo.write_text_bodies(
                "circulaire",
                &[(
                    uid.to_string(),
                    "Les infections nosocomiales, par décret.".to_string(),
                )],
                BodyWrite::Replace,
            )
            .await
            .unwrap();
        }
        conn.execute(
            "UPDATE legal_text SET slug = $2 WHERE text_uid = $1",
            &[&uid, &slug],
        )
        .await
        .unwrap();
        for debut in versions {
            repo.upsert_legal_article(&version(uid, "1", *debut), Fraicheur::Autoritaire)
                .await
                .unwrap();
        }
        uids.push(uid.to_string());
    }
    conn.execute(
        "UPDATE legal_text SET role = 'individuel' WHERE text_uid = 'BOUNDS/G'",
        &[],
    )
    .await
    .unwrap();
    repo.refresh_article_denorm(Some(&uids)).await.unwrap();

    let home = lj_norm::jurisdictions::DOMESTIC_REGIONS;
    let day = |d: &str| Some(jour(d));
    let listed = |rows: Vec<lj_store::repository::TextListRow>| {
        rows.into_iter()
            .map(|r| (r.text_uid, r.valid_from.to_string()))
            .collect::<Vec<_>>()
    };
    let pairs = |p: &[(&str, &str)]| {
        p.iter()
            .map(|(u, d)| (u.to_string(), d.to_string()))
            .collect::<Vec<_>>()
    };
    let filters = |jurisdictions: &'static [&'static str], valid_from, at| {
        lj_store::repository::ArticleBrowseFilters {
            home: &home,
            text_uid: None,
            jurisdictions,
            source: None,
            category: None,
            at,
            valid_from,
        }
    };

    // An old law amended in the period comes up on its amendment.
    let laws = repo
        .list_texts(filters(&["FR.LOI"], None, None), 10, 0)
        .await
        .unwrap();
    assert_eq!(
        listed(laws),
        pairs(&[
            ("BOUNDS/A", "2026-03-03"),
            ("BOUNDS/D", "2026-02-01"),
            ("BOUNDS/B", "2025-01-11"),
        ])
    );
    // A code has no date of its own and comes up on its new version; a text read
    // whole on its own date, a version of unknown start on its text's date.
    let recent = repo
        .list_texts(filters(&[], day("2026-01-01"), None), 10, 0)
        .await
        .unwrap();
    assert_eq!(
        listed(recent),
        pairs(&[
            ("BOUNDS/H", "2026-04-01"),
            ("BOUNDS/A", "2026-03-03"),
            ("BOUNDS/F", "2026-02-02"),
            ("BOUNDS/D", "2026-02-01"),
            ("BOUNDS/CODE", "2026-01-01"),
        ])
    );
    // Nothing that starts after `as_of` is read.
    let then = repo
        .list_texts(filters(&[], day("2025-01-01"), day("2025-12-31")), 10, 0)
        .await
        .unwrap();
    assert_eq!(listed(then), pairs(&[("BOUNDS/B", "2025-01-11")]));
    let organic = repo
        .list_texts(
            filters(&["FR.LOI.ORGANIQUE", "EU"], day("2025-01-01"), None),
            10,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        listed(organic),
        pairs(&[("BOUNDS/F", "2026-02-02"), ("BOUNDS/B", "2025-01-11")])
    );

    // A law on both legs, by its own date and by its articles, counts once.
    let stats = repo
        .text_list_stats(filters(&["FR.LOI"], day("2025-06-01"), None))
        .await
        .unwrap();
    assert_eq!(stats.total, 2);
    let count = |path: &str| stats.path.iter().find(|c| c.value == path).map(|c| c.count);
    assert_eq!(count("FR.LOI"), Some(2));
    assert_eq!(count("FR.LOI.ORGANIQUE"), None);

    let search = |jurisdictions: &'static [&'static str], from: &'static str| {
        let repo = &repo;
        async move {
            let mut hits: Vec<String> = repo
                .search_articles(
                    "nosocomiales",
                    &[],
                    &home,
                    &lj_store::repository::PlaceFilter::default(),
                    None,
                    jurisdictions,
                    None,
                    None,
                    None,
                    None,
                    day(from),
                    10,
                    0,
                )
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.text_uid)
                .collect();
            hits.sort();
            hits
        }
    };
    assert_eq!(
        search(&[], "2026-01-01").await,
        [
            "BOUNDS/A",
            "BOUNDS/CODE",
            "BOUNDS/D",
            "BOUNDS/F",
            "BOUNDS/H"
        ]
    );
    assert_eq!(
        search(&["FR.LOI"], "2026-01-01").await,
        ["BOUNDS/A", "BOUNDS/D"]
    );
}
