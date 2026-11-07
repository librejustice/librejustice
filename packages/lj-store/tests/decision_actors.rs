//! A decision's actors as one document, linked and read back through the row
//! view. Against an empty Postgres with ParadeDB and VectorChord,
//! `LIBREJUSTICE_TEST_DB_URL`.

use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{DecisionPartyRow, DecisionRepository};
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
        })
        .await;
    let pool = build_pool(&url(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    conn.batch_execute("BEGIN").await.unwrap();
    (pool, conn)
}

fn row(quality: &str, side: Option<&str>, value: &str, key: &str) -> DecisionPartyRow {
    DecisionPartyRow {
        quality: quality.to_string(),
        side: side.map(str::to_string),
        value: value.to_string(),
        resolve_key: key.to_string(),
        nature: None,
        barreau: None,
        role: None,
        firm_ord: None,
        party_ord: None,
        sub_ord: None,
        mentions: Vec::new(),
    }
}

fn actors() -> Vec<DecisionPartyRow> {
    let mut alpha = row("party", Some("applicant"), "SAS Alpha", "alpha");
    alpha.nature = Some("morale_privee".into());
    alpha.mentions = vec![10, 19];
    let mut a = row("party", Some("defendant"), "A", "a");
    a.nature = Some("physique".into());
    let mut dupont = row("counsel_name", None, "Jean Dupont", "jean dupont");
    dupont.barreau = Some("lyon".into());
    dupont.party_ord = Some(0);
    dupont.mentions = vec![30, 41, 50, 56];
    vec![alpha, a, dupont]
}

type ActorView = (
    i32,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<serde_json::Value>,
);

async fn view(conn: &Connection, id: i64) -> Vec<ActorView> {
    conn.query(
        "SELECT ord, quality, side, value, resolve_key, nature, entity_uid, party_ord, \
                mentions \
         FROM decision_actor WHERE decision_id = $1 ORDER BY ord",
        &[&id],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r.get(0),
            r.get(1),
            r.get(2),
            r.get(3),
            r.get(4),
            r.get(5),
            r.get(6),
            r.get(7),
            r.get(8),
        )
    })
    .collect()
}

async fn terms(conn: &Connection, id: i64) -> Vec<String> {
    let mut t: Vec<String> = conn
        .query_one(
            "SELECT public.lj_actor_terms(actors) FROM decision_actors WHERE decision_id = $1",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    t.sort();
    t
}

async fn links(conn: &Connection, id: i64) -> Vec<Option<String>> {
    view(conn, id).await.into_iter().map(|r| r.6).collect()
}

async fn decision(conn: &Connection, public_id: &str) -> i64 {
    conn.query_one(
        "INSERT INTO decisions (jurisdiction_type, public_id) VALUES ('TA', $1) RETURNING id",
        &[&public_id],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn the_document_keeps_its_links_until_an_actor_changes() {
    let (_pool, conn) = base().await;
    let id = decision(&conn, "test-actors").await;
    let repo = DecisionRepository::new(&conn);
    assert_eq!(
        repo.replace_decision_actors_bulk(&[(id, Some(actors()))])
            .await
            .unwrap(),
        [id]
    );
    let rows = view(&conn, id).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        (
            0,
            "party".into(),
            Some("applicant".into()),
            "SAS Alpha".into(),
            "alpha".into(),
            Some("morale_privee".into()),
            None,
            None,
            Some(serde_json::json!([10, 19])),
        )
    );
    assert_eq!(rows[1].5.as_deref(), Some("physique"));
    assert_eq!(rows[1].8, None);
    assert_eq!(rows[2].2, None);
    assert_eq!(rows[2].7, Some(0));
    assert_eq!(rows[2].8, Some(serde_json::json!([30, 41, 50, 56])));
    assert_eq!(terms(&conn, id).await, ["k|a", "k|alpha", "k|jean dupont"]);

    let set = |ords: &str, keys: &str, uids: &str| {
        format!(
            "UPDATE decision_actors SET actors = public.lj_actor_set_entities(actors, \
             '{ords}'::int[], '{keys}'::text[], '{uids}'::text[]) WHERE decision_id = $1"
        )
    };
    conn.execute(&set("{0}", "{beta}", "{siren:9}"), &[&id])
        .await
        .unwrap();
    assert_eq!(links(&conn, id).await, [None, None, None]);
    conn.execute(&set("{0}", "{alpha}", "{siren:1}"), &[&id])
        .await
        .unwrap();
    assert_eq!(links(&conn, id).await[0].as_deref(), Some("siren:1"));
    assert_eq!(
        terms(&conn, id).await,
        ["e|siren:1", "k|a", "k|jean dupont"]
    );

    assert!(repo
        .replace_decision_actors_bulk(&[(id, Some(actors()))])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(links(&conn, id).await[0].as_deref(), Some("siren:1"));

    let mut moved = actors();
    moved[0].mentions = vec![12, 21];
    assert!(repo
        .replace_decision_actors_bulk(&[(id, Some(moved.clone()))])
        .await
        .unwrap()
        .is_empty());
    let rows = view(&conn, id).await;
    assert_eq!(rows[0].8, Some(serde_json::json!([12, 21])));
    assert_eq!(rows[0].6.as_deref(), Some("siren:1"));

    moved[1].value = "B".into();
    moved[1].resolve_key = "b".into();
    assert_eq!(
        repo.replace_decision_actors_bulk(&[(id, Some(moved))])
            .await
            .unwrap(),
        [id]
    );
    assert_eq!(links(&conn, id).await, [Some("siren:1".into()), None, None]);

    conn.execute(&set("{0,0}", "{alpha,alpha}", "{NULL,NULL}"), &[&id])
        .await
        .unwrap();
    assert_eq!(links(&conn, id).await, [None, None, None]);

    assert_eq!(
        repo.replace_decision_actors_bulk(&[(id, None)])
            .await
            .unwrap(),
        [id]
    );
    assert!(view(&conn, id).await.is_empty());
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL, an empty Postgres with ParadeDB and VectorChord"]
async fn a_written_key_resolves_and_links_every_pending_actor_holding_it() {
    let (_pool, conn) = base().await;
    let repo = DecisionRepository::new(&conn);
    let first = decision(&conn, "test-actors-first").await;
    repo.replace_decision_actors_bulk(&[(first, Some(actors()))])
        .await
        .unwrap();
    assert_eq!(links(&conn, first).await, [None, None, None]);
    conn.execute(
        "INSERT INTO entity (uid, nature, denomination, active, category, denominations) \
         VALUES ('siren:1', 'morale_privee', 'Alpha', true, 'societes', '[{\"d\": \"Alpha\"}]')",
        &[],
    )
    .await
    .unwrap();
    let second = decision(&conn, "test-actors-second").await;
    repo.replace_decision_actors_bulk(&[(second, Some(actors()))])
        .await
        .unwrap();
    for id in [first, second] {
        assert_eq!(links(&conn, id).await, [Some("siren:1".into()), None, None]);
    }

    let (docs, touched) = repo
        .actor_remap_entities(&[("siren:1".into(), "siren:2".into())], true)
        .await
        .unwrap();
    assert_eq!((docs, touched), (2, vec![first, second]));
    assert_eq!(links(&conn, first).await[0].as_deref(), Some("siren:2"));
    assert_eq!(
        repo.actor_unlink_uids(&["siren:2".into()]).await.unwrap(),
        2
    );
    assert_eq!(links(&conn, second).await, [None, None, None]);
    assert_eq!(
        terms(&conn, second).await,
        ["k|a", "k|alpha", "k|jean dupont"]
    );
}
