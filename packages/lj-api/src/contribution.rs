//! `/decision/contribute` posts here: a file and its box, kept until review. Every
//! answer is a 303 back to the page, `sent=1` or `error=<code>`.

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::Router;
use lj_store::repository::DecisionRepository;

use crate::auth::RequiredUser;
use crate::ratelimit::{Cost, Verdict};
use crate::state::AppState;

const FILE_MAX: usize = 5 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/contribution", post(contribution))
        .layer(DefaultBodyLimit::max(FILE_MAX + 64 * 1024))
}

async fn contribution(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    headers: HeaderMap,
    mut form: Multipart,
) -> Response {
    let key = crate::ratelimit::caller_key(Some(&sub), &headers);
    if state.budgets.take(Cost::Contribution, key.as_ref()).await == Verdict::Exhausted {
        return back("error", "trop");
    }

    let mut file = None;
    let mut attested = false;
    loop {
        match form.next_field().await {
            Ok(Some(field)) => match field.name() {
                Some("fichier") => match field.bytes().await {
                    Ok(bytes) => file = Some(bytes),
                    Err(e) => return refused(e.status()),
                },
                Some("atteste") => attested = true,
                _ => {}
            },
            Ok(None) => break,
            Err(e) => return refused(e.status()),
        }
    }

    if !attested {
        return back("error", "case");
    }
    let Some(file) = file else {
        return back("error", "format");
    };
    if file.len() > FILE_MAX {
        return back("error", "trop_lourd");
    }
    if !(file.starts_with(b"%PDF") || file.starts_with(b"PK\x03\x04")) {
        return back("error", "format");
    }

    let conn = match state.pool.get().await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!("submission not stored, pool: {e}");
            return back("error", "echec");
        }
    };
    match DecisionRepository::new(&conn)
        .insert_submission(&sub, &file)
        .await
    {
        Ok(id) => {
            tracing::info!(id, bytes = file.len(), "submission received");
            back("sent", "1")
        }
        Err(e) => {
            tracing::error!("submission not stored: {e}");
            back("error", "echec")
        }
    }
}

fn refused(status: StatusCode) -> Response {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => back("error", "trop_lourd"),
        _ => back("error", "echec"),
    }
}

fn back(key: &str, value: &str) -> Response {
    Redirect::to(&format!("/decision/contribute?{key}={value}")).into_response()
}
