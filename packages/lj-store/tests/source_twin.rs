//! A court site that posts one decision twice: the second post finds the first at its address
//! by its text.

use chrono::NaiveDate;
use lj_pg::db::{build_pool, Connection};
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::DecisionRepository;
use serde_json::json;

fn url() -> String {
    std::env::var("LIBREJUSTICE_TEST_DB_URL").expect("LIBREJUSTICE_TEST_DB_URL")
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2023, 11, 9).unwrap()
}

const RULING: &str = "La Cour, statuant publiquement, contradictoirement et en dernier ressort, \
    casse et annule en toutes ses dispositions l'arrêt rendu le 12 mars 2021 par la cour d'appel \
    d'Abidjan, renvoie la cause et les parties devant la même juridiction autrement composée.";

const ORDER: &str = "La Cour ordonne la jonction des requêtes, réserve sa décision sur la \
    recevabilité jusqu'au jugement au fond, et invite l'État défendeur à déposer sa réponse \
    dans un délai de soixante jours à compter de la signification de la présente ordonnance.";

async fn decision(
    conn: &Connection,
    public_id: &str,
    dockets: &[&str],
    source_uid: &str,
    text: &str,
) {
    let dockets: Vec<String> = dockets.iter().map(|d| d.to_string()).collect();
    let id: i64 = conn
        .query_one(
            "INSERT INTO decisions (jurisdiction_type, jurisdiction_code, public_id, date_lecture, docket_numbers) \
             VALUES ('COUR_CASSATION', 'ci_twin', $1, $2, $3) RETURNING id",
            &[&public_id, &day(), &dockets],
        )
        .await
        .unwrap()
        .get(0);
    let source = source_uid.split_once('/').unwrap().0;
    conn.execute(
        "INSERT INTO decision_sources (decision_id, source, source_uid, content_checksum, payload_format, source_fields) \
         VALUES ($1, $2, $3, 'x', 'pdf', $4)",
        &[&id, &source, &source_uid, &json!({})],
    )
    .await
    .unwrap();
    conn.execute(
        "SELECT decision_text_put($1, $2, 'fr', $3, NULL)",
        &[&id, &source_uid, &text],
    )
    .await
    .unwrap();
}

async fn twin(
    repo: &DecisionRepository<'_>,
    uid: &str,
    dockets: &[&str],
    text: &str,
) -> Option<String> {
    let dockets: Vec<String> = dockets.iter().map(|d| d.to_string()).collect();
    let source = uid.split_once('/').unwrap().0;
    repo.source_twin(source, uid, "ci_twin", day(), &dockets, text)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_decision_posted_twice_finds_its_first_post_and_another_decision_at_its_address_does_not()
{
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
         VALUES ('ci_twin', 'COUR_CASSATION', 'Cour de cassation', 'ci_twin')",
    )
    .await
    .unwrap();
    let first = "cassation-ci/arret-n-1008-23-du-09-novembre-2023-2-1";
    decision(&conn, "twin-first", &["1008/23"], first, RULING).await;
    decision(
        &conn,
        "order",
        &["1008/23"],
        "cassation-ci/ordonnance-1008",
        ORDER,
    )
    .await;
    decision(
        &conn,
        "case-file",
        &["42", "118/02-CO"],
        "cassation-ci/arret-42",
        RULING,
    )
    .await;
    let repo = DecisionRepository::new(&conn);

    assert_eq!(
        twin(
            &repo,
            "cassation-ci/arret-n-1008-23-du-09-novembre-2023-1",
            &["1008/23"],
            RULING
        )
        .await,
        Some(first.to_string())
    );
    assert_eq!(
        twin(
            &repo,
            "cassation-ci/arret-n-1008-23-du-09-novembre-2023-1",
            &["1008/23"],
            &format!("{RULING} Opinion individuelle du juge.")
        )
        .await,
        Some(first.to_string()),
        "a post that adds an opinion holds the first"
    );
    assert_eq!(
        twin(
            &repo,
            "cassation-ci/ordonnance-1008-bis",
            &["1008/23"],
            "La Cour rejette la demande de mesures provisoires présentée par le requérant."
        )
        .await,
        None,
        "another decision of the same day and docket"
    );
    assert_eq!(
        twin(&repo, first, &["1008/23"], RULING).await,
        None,
        "a post is not its own twin"
    );
    assert_eq!(
        twin(&repo, "juricaf/CI-1008", &["1008/23"], RULING).await,
        None,
        "another source attaches, it does not twin"
    );
    assert_eq!(
        twin(&repo, "cassation-ci/arret-43", &["43", "118/02-CO"], RULING).await,
        None,
        "another arrêt of the same case file"
    );
    assert_eq!(twin(&repo, "cassation-ci/arret-x", &[], RULING).await, None);
}
