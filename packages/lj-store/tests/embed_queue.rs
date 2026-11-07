//! Which decisions `embed-missing` rebuilds, keyed on the visit and on the summary.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::DecisionRepository;
use serde_json::json;

const V: i16 = 2;

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

#[derive(Clone, Copy)]
struct Seed<'a> {
    embed_version: Option<i16>,
    /// One passage, embedded or not; `None` for no passage.
    passage: Option<bool>,
    embedded_behind: (Option<i16>, Option<i16>),
    summary: Option<(&'a str, i16, i16)>,
}

const BARE: Seed = Seed {
    embed_version: None,
    passage: None,
    embedded_behind: (None, None),
    summary: None,
};

async fn decision(conn: &Connection, name: &str, seed: Seed<'_>) -> i64 {
    let id: i64 = conn
        .query_one(
            "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, \
             embed_version, embed_summary_engine, embed_summary_prompt_version) \
             VALUES ('CA', 'ca_embed', $1, $2, $3, $4, $5) RETURNING id",
            &[
                &name,
                &NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                &seed.embed_version,
                &seed.embedded_behind.0,
                &seed.embedded_behind.1,
            ],
        )
        .await
        .unwrap()
        .get(0);
    let source_uid = format!("judilibre/{name}");
    conn.execute(
        "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields) \
         VALUES ($1, 'judilibre', $2, 'x', 'json', $3)",
        &[&id, &source_uid, &json!({})],
    )
    .await
    .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, 'fr', 'Rejette le pourvoi', NULL)",
        &[&id, &source_uid],
    )
    .await
    .unwrap();
    if let Some((summary, engine, version)) = seed.summary {
        conn.execute(
            "UPDATE decision_text SET summary = $2, summary_engine = $3, summary_prompt_version = $4 \
             WHERE decision_id = $1",
            &[&id, &summary, &engine, &version],
        )
        .await
        .unwrap();
    }
    if let Some(embedded) = seed.passage {
        conn.execute(
            "INSERT INTO decision_chunks (decision_id, chunk_index, jurisdiction_type, char_start, char_end, embedding, zones) \
             VALUES ($1, 0, 'CA', 0, 18, CASE WHEN $2 THEN \
             quantize_to_rabitq8(array_fill(0.1::real, ARRAY[1024])::vector)::rabitq8(1024) END, '{text}')",
            &[&id, &embedded],
        )
        .await
        .unwrap();
    }
    id
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn the_queue_takes_what_was_never_visited_and_what_a_summary_now_frames() {
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
         VALUES ('ca_embed', 'CA', 'Cour d''appel', 'ca_embed')",
    )
    .await
    .unwrap();

    let embedded = Seed {
        embed_version: Some(V),
        passage: Some(true),
        ..BARE
    };
    let summary = Some(("Le pourvoi est rejeté.", 1, 4));
    let never_cut = decision(&conn, "never-cut", BARE).await;
    let visited_empty = decision(
        &conn,
        "visited-empty",
        Seed {
            embed_version: Some(V),
            summary,
            ..BARE
        },
    )
    .await;
    let current = decision(&conn, "current", embedded).await;
    let summary_arrived = decision(
        &conn,
        "summary-arrived",
        Seed {
            summary,
            ..embedded
        },
    )
    .await;
    let summary_rewritten = decision(
        &conn,
        "summary-rewritten",
        Seed {
            embedded_behind: (Some(1), Some(3)),
            summary,
            ..embedded
        },
    )
    .await;
    let behind_current = decision(
        &conn,
        "behind-current",
        Seed {
            embedded_behind: (Some(1), Some(4)),
            summary,
            ..embedded
        },
    )
    .await;
    let older = decision(
        &conn,
        "older",
        Seed {
            embed_version: Some(1),
            ..embedded
        },
    )
    .await;
    let unembedded = decision(
        &conn,
        "unembedded",
        Seed {
            passage: Some(false),
            ..embedded
        },
    )
    .await;

    let repo = DecisionRepository::new(&conn);
    let ours = [
        never_cut,
        visited_empty,
        current,
        summary_arrived,
        summary_rewritten,
        behind_current,
        older,
        unembedded,
    ];
    let queue = |stock, shard| {
        let repo = &repo;
        async move {
            let mut ids = repo.embed_queue(V, stock, shard).await.unwrap();
            ids.retain(|id| ours.contains(id));
            ids
        }
    };

    let mut daily = vec![never_cut, summary_arrived, unembedded];
    daily.sort();
    assert_eq!(queue(false, (0, 1)).await, daily);

    let mut stock = vec![
        never_cut,
        summary_arrived,
        unembedded,
        summary_rewritten,
        older,
    ];
    stock.sort();
    assert_eq!(queue(true, (0, 1)).await, stock);

    let odd: Vec<i64> = daily.iter().copied().filter(|id| id % 2 == 1).collect();
    assert_eq!(queue(false, (1, 2)).await, odd);
}
