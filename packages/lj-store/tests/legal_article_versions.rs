//! One row per DILA article version, whatever the order its dates arrive in.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::{Datelike, NaiveDate};
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{
    DecisionRepository, Fraicheur, LegalArticleRow, LegalLinkOwner, LegalLinkRow,
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

fn version(text_uid: &str, id: &str, debut: Option<&str>) -> LegalArticleRow {
    let date_debut = debut.map(jour);
    LegalArticleRow {
        text_uid: text_uid.to_string(),
        num: "L. 1".to_string(),
        num_key: "l1".to_string(),
        position: None,
        title_path: None,
        status: "VIGUEUR".to_string(),
        date_debut,
        date_fin: None,
        texte: Some(format!("{id} {debut:?}")),
        texte_original: None,
        lang_original: None,
        translation: "officiel".to_string(),
        nota: None,
        content_checksum: date_debut.map_or(0, |d| d.num_days_from_ce() as u64),
        source: "legifrance".to_string(),
        source_uid: id.to_string(),
        source_url: None,
        source_asof: None,
        source_upstream_url: None,
    }
}

async fn ecrire(conn: &Connection, row: &LegalArticleRow, fraicheur: Fraicheur) -> bool {
    DecisionRepository::new(conn)
        .upsert_legal_article(row, fraicheur)
        .await
        .unwrap()
}

async fn fenetres(
    conn: &Connection,
    text_uid: &str,
) -> Vec<(String, NaiveDate, Option<NaiveDate>)> {
    conn.query(
        "SELECT source_uid, date_debut, date_fin FROM legal_article \
          WHERE text_uid = $1 ORDER BY date_debut",
        &[&text_uid],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2)))
    .collect()
}

fn attendu(lignes: &[(&str, &str, Option<&str>)]) -> Vec<(String, NaiveDate, Option<NaiveDate>)> {
    lignes
        .iter()
        .map(|(id, debut, fin)| (id.to_string(), jour(debut), fin.map(jour)))
        .collect()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_postponed_entry_into_force_leaves_no_row_at_the_old_date() {
    let (_pool, conn) = base().await;
    let texte = "LEGITEXT_REPORT";
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_A", Some("2020-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2025-03-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    assert!(
        ecrire(
            &conn,
            &version(texte, "LEGIARTI_B", Some("2027-01-01")),
            Fraicheur::Autoritaire
        )
        .await
    );
    assert_eq!(
        fenetres(&conn, texte).await,
        attendu(&[
            ("LEGIARTI_A", "2020-01-01", Some("2026-12-31")),
            ("LEGIARTI_B", "2027-01-01", None),
        ])
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn an_unknown_start_date_fixed_later_leaves_no_phantom_end() {
    let (_pool, conn) = base().await;
    let texte = "LEGITEXT_INCONNUE";
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_A", Some("2020-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2222-02-22")),
        Fraicheur::Autoritaire,
    )
    .await;
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2026-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    assert_eq!(
        fenetres(&conn, texte).await,
        attendu(&[
            ("LEGIARTI_A", "2020-01-01", Some("2025-12-31")),
            ("LEGIARTI_B", "2026-01-01", None),
        ])
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_replay_older_than_the_base_does_not_bring_the_stale_date_back() {
    let (_pool, conn) = base().await;
    let texte = "LEGITEXT_REJEU";
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_A", Some("2020-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2027-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    assert!(
        !ecrire(
            &conn,
            &version(texte, "LEGIARTI_B", Some("2025-03-01")),
            Fraicheur::Rejeu
        )
        .await
    );
    assert_eq!(
        fenetres(&conn, texte).await,
        attendu(&[
            ("LEGIARTI_A", "2020-01-01", Some("2026-12-31")),
            ("LEGIARTI_B", "2027-01-01", None),
        ])
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_dated_delivery_replaces_the_undated_row_of_its_version() {
    let (_pool, conn) = base().await;
    let texte = "LEGITEXT_SANS_DATE";
    ecrire(&conn, &version(texte, "LEGIARTI_B", None), Fraicheur::Rejeu).await;
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2024-01-01")),
        Fraicheur::Rejeu,
    )
    .await;
    assert_eq!(
        fenetres(&conn, texte).await,
        attendu(&[("LEGIARTI_B", "2024-01-01", None)])
    );
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_moved_version_takes_no_edge_with_it_from_its_old_date() {
    let (_pool, conn) = base().await;
    let texte = "LEGITEXT_LIENS";
    let ancienne = version(texte, "LEGIARTI_B", Some("2025-03-01"));
    ecrire(&conn, &ancienne, Fraicheur::Autoritaire).await;
    let lien = LegalLinkRow {
        typelien: "CITATION".to_string(),
        verb: "cite".to_string(),
        direction: "outgoing".to_string(),
        target_kind: "article".to_string(),
        target_uid: Some("LEGIARTI_CIBLE".to_string()),
        target_text_uid: None,
        target_num: None,
        target_num_key: None,
        target_nature: None,
        target_label: "article cité".to_string(),
        target_date: None,
        target_nor: None,
    };
    DecisionRepository::new(&conn)
        .replace_legal_links(&[(
            LegalLinkOwner {
                text_uid: texte.to_string(),
                num_key: ancienne.num_key.clone(),
                date_debut: ancienne.date_debut,
            },
            vec![lien],
        )])
        .await
        .unwrap();
    ecrire(
        &conn,
        &version(texte, "LEGIARTI_B", Some("2027-01-01")),
        Fraicheur::Autoritaire,
    )
    .await;
    let restants: i64 = conn
        .query_one(
            "SELECT count(*) FROM legal_link WHERE owner_text_uid = $1",
            &[&texte],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(restants, 0);
}
