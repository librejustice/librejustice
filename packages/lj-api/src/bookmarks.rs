//! Endpoints `/me/bookmarks` — signets de décisions (port de `bookmarks.py`).
//!
//! Cf. ADR 0036. La ligne `users` parente est garantie par le JIT provisioning ;
//! les FK `user_bookmarks.user_sub` / `decision_id` (ON DELETE CASCADE)
//! maintiennent l'intégrité. La création est gatée par `track_activity`
//! directement dans l'INSERT (`WHERE EXISTS`, ADR 0056).

use deadpool_postgres::Pool;
use lj_dtos::{BookmarkItem, JuridictionType};

use crate::error::ApiError;
use crate::me::ts_to_rfc3339;
use crate::referential::{referential, Referential};
use crate::state::AppState;
use crate::titles::decision_title;

/// Résout un `public_id` opaque en id interne `decisions.id` (parité
/// `decisions.resolve_decision_pk`). Renvoie `None` si la décision n'existe pas.
pub(crate) async fn resolve_decision_pk(
    pool: &Pool,
    public_id: &str,
) -> Result<Option<i64>, ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let row = conn
        .query_opt(
            "SELECT id FROM decisions WHERE public_id = $1",
            &[&public_id],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("resolve_decision_pk: {e}")))?;
    Ok(row.map(|r| r.get::<_, i64>(0)))
}

/// Résout un `public_id` ou renvoie 404 (parité `_resolve_decision_id`).
async fn resolve_decision_id(pool: &Pool, public_id: &str) -> Result<i64, ApiError> {
    resolve_decision_pk(pool, public_id)
        .await?
        .ok_or(ApiError::NotFound) // detail=decision_not_found côté Python
}

/// Désérialise une valeur TEXT de juridiction (`"TA"`, `"CE"`, …) vers l'enum.
fn parse_juridiction_type(raw: &str) -> Result<JuridictionType, ApiError> {
    serde_json::from_value(serde_json::Value::String(raw.to_string()))
        .map_err(|e| ApiError::Internal(format!("juridiction_type invalide {raw:?}: {e}")))
}

/// Page de signets (plus récents d'abord) + total complet (parité
/// `fetch_bookmarks`).
///
/// `total` via `COUNT(*) OVER()` (une seule requête). `limit` `None` → tout.
/// Solution et nom de juridiction résolus depuis le référentiel (ADR 0146).
pub async fn fetch_bookmarks(
    pool: &Pool,
    refs: &Referential,
    user_sub: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<(Vec<BookmarkItem>, i64), ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let rows = conn
        .query(
            "SELECT \
               d.public_id, \
               d.juridiction_type, \
               d.jurisdiction_code, \
               to_char(d.date_lecture, 'YYYY-MM-DD') AS date_lecture, \
               d.docket_numbers, \
               d.solution_uid, \
               d.summary, \
               b.created_at, \
               COUNT(*) OVER() AS total \
             FROM user_bookmarks b \
             JOIN decisions d ON d.id = b.decision_id \
             WHERE b.user_sub = $1 \
             ORDER BY b.created_at DESC \
             LIMIT $2 OFFSET $3",
            &[&user_sub, &limit, &offset],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("fetch_bookmarks: {e}")))?;

    let total = rows.first().map(|r| r.get::<_, i64>(8)).unwrap_or(0);
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let jur_type_raw: &str = row.get(1);
        let jurisdiction_code: Option<String> = row.get(2);
        let date_lecture: Option<String> = row.get(3);
        let docket_numbers: Option<Vec<String>> = row.get(4);
        let solution_uid: Option<String> = row.get(5);
        let jurisdiction_name = jurisdiction_code
            .as_deref()
            .and_then(|c| refs.jurisdiction(c))
            .map(|j| j.label.clone());
        // Titre composé depuis les référentiels (ADR 0146 §4), jamais la colonne
        // `search_title` (formation source brute).
        let title = decision_title(
            refs.juridiction_type_label(jur_type_raw)
                .unwrap_or(jur_type_raw),
            jurisdiction_name.as_deref(),
            None,
            date_lecture.as_deref(),
            docket_numbers.as_deref(),
        );
        items.push(BookmarkItem {
            id: row.get(0),
            title,
            juridiction_type: parse_juridiction_type(jur_type_raw)?,
            jurisdiction_name,
            date_lecture,
            docket_numbers,
            solution: solution_uid.as_deref().map(|u| refs.tag(u)),
            summary: row.get(6),
            bookmarked_at: ts_to_rfc3339(row.get(7)),
        });
    }
    Ok((items, total))
}

/// `GET /me/bookmarks` — liste complète (pas de pagination) : les signets sont
/// curés manuellement donc bornés (parité `list_bookmarks`).
pub async fn list_bookmarks(
    state: &AppState,
    user_sub: &str,
) -> Result<(Vec<BookmarkItem>, i64), ApiError> {
    let refs = referential(state).await?;
    fetch_bookmarks(&state.pool, &refs, user_sub, None, 0).await
}

/// `PUT /me/bookmarks/{decision_id}` — ajoute un signet (idempotent via
/// `ON CONFLICT DO NOTHING`), gaté par `track_activity` (parité `add_bookmark`).
pub async fn add_bookmark(
    state: &AppState,
    user_sub: &str,
    public_id: &str,
) -> Result<(), ApiError> {
    let db_id = resolve_decision_id(&state.pool, public_id).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "INSERT INTO user_bookmarks (user_sub, decision_id) \
         SELECT $1, $2 \
         WHERE EXISTS (SELECT 1 FROM users WHERE sub = $3 AND track_activity) \
         ON CONFLICT (user_sub, decision_id) DO NOTHING",
        &[&user_sub, &db_id, &user_sub],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("add_bookmark: {e}")))?;
    Ok(())
}

/// `DELETE /me/bookmarks/{decision_id}` — retire un signet (parité
/// `remove_bookmark`).
pub async fn remove_bookmark(
    state: &AppState,
    user_sub: &str,
    public_id: &str,
) -> Result<(), ApiError> {
    let db_id = resolve_decision_id(&state.pool, public_id).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "DELETE FROM user_bookmarks WHERE user_sub = $1 AND decision_id = $2",
        &[&user_sub, &db_id],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("remove_bookmark: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_juridiction_type_roundtrip() {
        assert_eq!(parse_juridiction_type("TA").unwrap(), JuridictionType::Ta);
        assert_eq!(parse_juridiction_type("CE").unwrap(), JuridictionType::Ce);
        assert_eq!(
            parse_juridiction_type("TCOM").unwrap(),
            JuridictionType::Tcom
        );
        assert!(parse_juridiction_type("NOPE").is_err());
    }
}
