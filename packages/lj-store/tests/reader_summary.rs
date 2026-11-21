//! A text keeps its own summary, a reader in another language reads a reader summary.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{pick, DecisionRepository};
use serde_json::json;

const EMBED_VERSION: i16 = 4;

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

/// A decision with one text in `lang`, summarised, and embedded behind it when `embedded`.
async fn decision(
    conn: &Connection,
    name: &str,
    lang: (&str, &str),
    summary: (&str, i16, i16),
    embedded: bool,
) -> i64 {
    let (engine, version) = (summary.1, summary.2);
    let stamps = if embedded {
        (Some(EMBED_VERSION), Some(engine), Some(version))
    } else {
        (None, None, None)
    };
    let id: i64 = conn
        .query_one(
            "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, lang, \
             embed_version, embed_summary_engine, embed_summary_prompt_version) \
             VALUES ('CA', 'ca_reader', $1, $2, $3, $4, $5, $6) RETURNING id",
            &[
                &name,
                &NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                &lang.1,
                &stamps.0,
                &stamps.1,
                &stamps.2,
            ],
        )
        .await
        .unwrap()
        .get(0);
    let source_uid = format!("judilibre/{name}");
    conn.execute(
        "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields, lang) \
         VALUES ($1, 'judilibre', $2, 'x', 'json', $3, $4)",
        &[&id, &source_uid, &json!({}), &lang.0],
    )
    .await
    .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, $3, 'Texte', NULL)",
        &[&id, &source_uid, &lang.1],
    )
    .await
    .unwrap();
    conn.execute(
        "UPDATE decision_text SET summary = $2, summary_engine = $3, summary_prompt_version = $4 \
         WHERE decision_id = $1",
        &[&id, &summary.0, &engine, &version],
    )
    .await
    .unwrap();
    if embedded {
        conn.execute(
            "INSERT INTO decision_chunks (decision_id, chunk_index, jurisdiction_type, char_start, char_end, embedding, zones) \
             VALUES ($1, 0, 'CA', 0, 5, quantize_to_rabitq8(array_fill(0.1::real, ARRAY[1024])::vector)::rabitq8(1024), '{text}')",
            &[&id],
        )
        .await
        .unwrap();
    }
    id
}

async fn served(conn: &Connection, id: i64, lang: &str) -> Option<String> {
    conn.query_one(
        &format!(
            "SELECT {} FROM decisions d WHERE d.id = $1",
            pick::summary("d", "$2")
        ),
        &[&id, &lang],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_french_summary_on_a_foreign_text_moves_to_its_french_reader_summary() {
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
         VALUES ('ca_reader', 'CA', 'Cour d''appel', 'ca_reader')",
    )
    .await
    .unwrap();

    let french_on_german = "Le recours est rejeté.";
    let moved = decision(
        &conn,
        "french-on-german",
        ("deu", "de"),
        (french_on_german, 25, 11),
        true,
    )
    .await;
    let french = decision(
        &conn,
        "french",
        ("fra", "fr"),
        ("Le pourvoi est rejeté.", 25, 11),
        true,
    )
    .await;
    let own = decision(
        &conn,
        "german-own",
        ("deu", "de"),
        ("Die Beschwerde wird abgewiesen.", 28, 12),
        false,
    )
    .await;

    let repo = DecisionRepository::new(&conn);
    assert_eq!(
        repo.move_reader_summaries(11, EMBED_VERSION, 10)
            .await
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        repo.move_reader_summaries(11, EMBED_VERSION, 10)
            .await
            .unwrap(),
        (0, 0)
    );

    let row = conn
        .query_one(
            "SELECT t.summary, r.summary, r.summary_engine, r.summary_prompt_version, \
                    d.embed_version, d.embed_summary_engine \
             FROM decisions d JOIN decision_text t ON t.decision_id = d.id \
             JOIN decision_text_reader_summary r ON r.decision_text_id = t.id AND r.lang = 'fr' \
             WHERE d.id = $1",
            &[&moved],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, Option<String>>(0), None);
    assert_eq!(row.get::<_, String>(1), french_on_german);
    assert_eq!(row.get::<_, i16>(2), 25);
    assert_eq!(row.get::<_, i16>(3), 11);
    assert_eq!(row.get::<_, Option<i16>>(4), Some(EMBED_VERSION));
    assert_eq!(row.get::<_, Option<i16>>(5), None);

    assert_eq!(
        served(&conn, moved, "fr").await.as_deref(),
        Some(french_on_german)
    );
    assert_eq!(served(&conn, moved, "de").await, None);
    assert_eq!(
        served(&conn, own, "fr").await.as_deref(),
        Some("Die Beschwerde wird abgewiesen.")
    );
    assert_eq!(
        served(&conn, french, "de").await.as_deref(),
        Some("Le pourvoi est rejeté.")
    );

    let ours = [moved, french, own];
    assert_eq!(waiting(&repo, &ours).await, vec![own]);

    repo.set_reader_summary(own, "de", "fr", "Le recours est rejeté.", None, 12, 28)
        .await
        .unwrap();
    assert_eq!(
        served(&conn, own, "fr").await.as_deref(),
        Some("Le recours est rejeté.")
    );
    assert!(waiting(&repo, &ours).await.is_empty());
}

/// The decisions of `ours` waiting for a French reader summary, the database being shared
/// with the other test binaries.
async fn waiting(repo: &DecisionRepository<'_>, ours: &[i64]) -> Vec<i64> {
    repo.iter_reader_summaries("fr", 12, 100, None, true, &[], &[])
        .await
        .unwrap()
        .into_iter()
        .flatten()
        .map(|row| row.decision_id)
        .filter(|id| ours.contains(id))
        .collect()
}
