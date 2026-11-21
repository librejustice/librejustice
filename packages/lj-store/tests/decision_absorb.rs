//! A decision published once more under the number it joins, absorbed into it.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::DecisionRepository;
use serde_json::json;
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
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
                 VALUES ('ca_absorb', 'CA', 'Cour d''appel', 'ca_absorb')",
            )
            .await
            .unwrap();
        })
        .await;
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    (pool, conn)
}

async fn decision(
    conn: &Connection,
    public_id: &str,
    (day, chamber): (u32, &str),
    source_uid: &str,
    sections: &[&str],
    text: &str,
) -> i64 {
    let id: i64 = conn
        .query_one(
            "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture) \
             VALUES ('CA', 'ca_absorb', $1, $2) RETURNING id",
            &[&public_id, &NaiveDate::from_ymd_opt(2022, 9, day).unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    let fields = json!({
        "chamber": chamber,
        "sections": sections.iter().map(|k| json!({ "kind": k })).collect::<Vec<_>>(),
    });
    conn.execute(
        "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields) \
         VALUES ($1, 'judilibre', $2, 'x', 'json', $3)",
        &[&id, &source_uid, &fields],
    )
    .await
    .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, 'fr', $3, NULL)",
        &[&id, &source_uid, &text],
    )
    .await
    .unwrap();
    id
}

async fn pairs_of(repo: &DecisionRepository<'_>, copy: i64) -> Vec<i64> {
    repo.joinder_copy_pairs(1)
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.copy == copy)
        .map(|p| p.keeper)
        .collect()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn an_absorbed_copy_never_leads_its_keeper() {
    let (_pool, conn) = base().await;
    let day = (20, "Chambre 1-1");
    let copy = decision(
        &conn,
        "absorb-copy",
        day,
        "judilibre/absorb-a",
        &["preamble", "expose"],
        "Rôle N° RG 21/12103",
    )
    .await;
    let keeper = decision(
        &conn,
        "absorb-keeper",
        day,
        "judilibre/absorb-b",
        &["preamble", "dispositif"],
        "Ordonne la jonction des deux procédures enrôlées sous les numéros 21 ' 1472 et 21 ' 1203",
    )
    .await;
    let later = decision(
        &conn,
        "absorb-later",
        (21, "Chambre 1-1"),
        "judilibre/absorb-c",
        &["dispositif"],
        "Pourvoi",
    )
    .await;
    conn.execute(
        "INSERT INTO decision_links (decision_id, link_type, target_court, target_decision_id, extract_version) \
         VALUES ($1, 'POURVOI_CONTRE', 'x', $2, 1)",
        &[&later, &copy],
    )
    .await
    .unwrap();

    let repo = DecisionRepository::new(&conn);
    assert_eq!(pairs_of(&repo, copy).await, vec![keeper]);
    repo.absorb_into(copy, keeper).await.unwrap();

    assert_eq!(
        repo.authority_source_uid(keeper).await.unwrap().as_deref(),
        Some("judilibre/absorb-b")
    );
    let (on, text): (i64, String) = conn
        .query_one(
            "SELECT s.decision_id, t.source_uid FROM decision_sources s \
             JOIN decision_text t ON t.decision_id = s.decision_id \
             WHERE s.source_uid = 'judilibre/absorb-a'",
            &[],
        )
        .await
        .map(|r| (r.get(0), r.get(1)))
        .unwrap();
    assert_eq!((on, text.as_str()), (keeper, "judilibre/absorb-b"));
    let gone: i64 = conn
        .query_one("SELECT count(*) FROM decisions WHERE id = $1", &[&copy])
        .await
        .unwrap()
        .get(0);
    assert_eq!(gone, 0);
    let target: i64 = conn
        .query_one(
            "SELECT target_decision_id FROM decision_links WHERE decision_id = $1",
            &[&later],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(target, keeper);
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn another_day_or_chamber_makes_no_pair() {
    let (_pool, conn) = base().await;
    let copy = decision(
        &conn,
        "apart-copy",
        (20, "Chambre 2"),
        "judilibre/apart-a",
        &["preamble"],
        "Rôle N° RG 22/00125",
    )
    .await;
    for (public_id, day, uid) in [
        ("apart-day", (21, "Chambre 2"), "judilibre/apart-b"),
        ("apart-chamber", (20, "Chambre 3"), "judilibre/apart-c"),
    ] {
        decision(
            &conn,
            public_id,
            day,
            uid,
            &["dispositif"],
            "Jonction avec le RG 22/00125",
        )
        .await;
    }
    let repo = DecisionRepository::new(&conn);
    assert!(pairs_of(&repo, copy).await.is_empty());
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_judgment_held_once_per_language_keeps_both_texts_under_its_keeper() {
    let (_pool, conn) = base().await;
    let mut ids = Vec::new();
    for (public_id, lang, uid, text) in [
        (
            "lang-keeper",
            "fr",
            "juricaf/lang-fr",
            "La Cour suprême rejette le pourvoi.",
        ),
        (
            "lang-copy",
            "ar",
            "juricaf/lang-ar",
            "إن المجلس الأعلى يرفض الطلب",
        ),
    ] {
        let id: i64 = conn
            .query_one(
                "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, lang) \
                 VALUES ('CA', 'ca_absorb', $1, '2003-02-27', $2) RETURNING id",
                &[&public_id, &lang],
            )
            .await
            .unwrap()
            .get(0);
        conn.execute(
            "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields, lang) \
             VALUES ($1, 'juricaf', $2, 'x', 'html', '{}', $3)",
            &[&id, &uid, &lang],
        )
        .await
        .unwrap();
        conn.execute(
            "SELECT decision_text_put($1, $2, $3, $4, NULL)",
            &[&id, &uid, &lang, &text],
        )
        .await
        .unwrap();
        ids.push(id);
    }
    let [keeper, copy] = ids[..] else {
        unreachable!()
    };
    conn.execute(
        "UPDATE decision_text SET summary = 'ملخص' WHERE decision_id = $1",
        &[&copy],
    )
    .await
    .unwrap();

    let repo = DecisionRepository::new(&conn);
    assert_eq!(repo.absorb_language_into(copy, keeper).await.unwrap(), 1);

    let texts: Vec<(String, String, bool, Option<String>)> = conn
        .query(
            "SELECT lang, text_status, counted, summary FROM decision_text \
             WHERE decision_id = $1 ORDER BY lang",
            &[&keeper],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect();
    assert_eq!(
        texts,
        [
            ("ar".into(), "official".into(), false, Some("ملخص".into())),
            ("fr".into(), "authentic".into(), true, None),
        ]
    );
    let on: i64 = conn
        .query_one(
            "SELECT decision_id FROM decision_sources WHERE source_uid = 'juricaf/lang-ar'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(on, keeper);
    let gone: i64 = conn
        .query_one("SELECT count(*) FROM decisions WHERE id = $1", &[&copy])
        .await
        .unwrap()
        .get(0);
    assert_eq!(gone, 0);
}
