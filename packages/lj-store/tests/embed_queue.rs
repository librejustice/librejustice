//! Which decisions `embed-missing` rebuilds, keyed on the visit and on the summary.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{DecisionRepository, EmbedScope};
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

/// A second text of the decision, in English, with or without its summary.
async fn english(conn: &Connection, id: i64, name: &str, summary: Option<(&str, i16, i16)>) {
    let source_uid = format!("judilibre/{name}-en");
    conn.execute(
        "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields, lang) \
         VALUES ($1, 'judilibre', $2, 'y', 'json', $3, 'eng')",
        &[&id, &source_uid, &json!({})],
    )
    .await
    .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, 'en', 'Dismisses the appeal', NULL)",
        &[&id, &source_uid],
    )
    .await
    .unwrap();
    if let Some((summary, engine, version)) = summary {
        conn.execute(
            "UPDATE decision_text SET summary = $2, summary_engine = $3, summary_prompt_version = $4 \
             WHERE decision_id = $1 AND lang = 'en'",
            &[&id, &summary, &engine, &version],
        )
        .await
        .unwrap();
    }
}

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
    let current_summary = Seed {
        summary: Some(("Le pourvoi est rejeté.", 28, 12)),
        ..BARE
    };
    let summarised = decision(&conn, "summarised", current_summary).await;
    let both_summarised = decision(&conn, "both-summarised", current_summary).await;
    english(
        &conn,
        both_summarised,
        "both-summarised",
        Some(("The appeal is dismissed.", 28, 12)),
    )
    .await;
    let english_bare = decision(&conn, "english-bare", current_summary).await;
    english(&conn, english_bare, "english-bare", None).await;
    let english_withdrawn = decision(&conn, "english-withdrawn", current_summary).await;
    english(&conn, english_withdrawn, "english-withdrawn", None).await;
    conn.execute(
        "UPDATE decision_sources SET deleted_at = now() WHERE source_uid = 'judilibre/english-withdrawn-en'",
        &[],
    )
    .await
    .unwrap();
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
    let older_summary_arrived = decision(
        &conn,
        "older-summary-arrived",
        Seed {
            embed_version: Some(1),
            summary,
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
        summarised,
        both_summarised,
        english_bare,
        english_withdrawn,
        visited_empty,
        current,
        summary_arrived,
        summary_rewritten,
        behind_current,
        older,
        older_summary_arrived,
        unembedded,
    ];
    let scoped = |stock, shard, scope| {
        let repo = &repo;
        async move {
            let queue = repo.embed_queue(V, stock, shard, scope).await.unwrap();
            let mut ids = repo.embed_scope(&queue, scope).await.unwrap();
            ids.retain(|id| ours.contains(id));
            ids
        }
    };
    let queue = |stock, shard| scoped(stock, shard, EmbedScope::default());

    let mut daily = vec![
        never_cut,
        summarised,
        both_summarised,
        english_bare,
        english_withdrawn,
        summary_arrived,
        unembedded,
    ];
    daily.sort();
    assert_eq!(queue(false, (0, 1)).await, daily);

    let mut stock = vec![
        never_cut,
        summarised,
        both_summarised,
        english_bare,
        english_withdrawn,
        summary_arrived,
        older_summary_arrived,
        unembedded,
        summary_rewritten,
        older,
    ];
    stock.sort();
    assert_eq!(queue(true, (0, 1)).await, stock);

    let odd: Vec<i64> = daily.iter().copied().filter(|id| id % 2 == 1).collect();
    assert_eq!(queue(false, (1, 2)).await, odd);

    // A summary below its text's generation is written again, and a text without one
    // is given one, before the passages are cut.
    let gated = EmbedScope {
        summarised: true,
        by_jurisdiction: true,
        multilingual: false,
    };
    assert_eq!(
        scoped(false, (0, 1), gated).await,
        vec![english_withdrawn, both_summarised, summarised]
    );

    let multilingual = EmbedScope {
        multilingual: true,
        ..EmbedScope::default()
    };
    assert_eq!(
        scoped(true, (0, 1), multilingual).await,
        vec![both_summarised, english_bare]
    );

    // The summary worklist lists the very texts the gate waits on.
    let missing = |langs: &'static [&'static str]| {
        let repo = &repo;
        async move {
            let mut texts: Vec<(i64, String)> = repo
                .iter_summaries_by_jurisdiction(
                    12,
                    100,
                    None,
                    false,
                    &["CA"],
                    &[0],
                    false,
                    &["CA"],
                    false,
                    langs,
                )
                .await
                .unwrap()
                .into_iter()
                .flatten()
                .filter(|row| ours.contains(&row.decision_id))
                .map(|row| (row.decision_id, row.lang))
                .collect();
            texts.sort();
            texts
        }
    };
    let mut bare: Vec<(i64, String)> = [never_cut, current, older, unembedded]
        .into_iter()
        .map(|id| (id, "fr".to_owned()))
        .chain([(english_bare, "en".to_owned())])
        .collect();
    bare.sort();
    assert_eq!(missing(&[]).await, bare);
    assert_eq!(
        missing(&["en"]).await,
        vec![(english_bare, "en".to_owned())]
    );
}
