//! Endpoints `/me/decision-views` + hook d'enregistrement des consultations
//! (port de `decision_views.py`). Cf. ADR 0053.
//!
//! Modèle dédupliqué : une ligne par `(user, décision)`. Chaque ouverture bump
//! `last_viewed_at` + `view_count` et met à jour `last_source` (`web` | `mcp`)
//! avec **web prioritaire** : dès qu'une décision a été ouverte une fois à la
//! main, `last_source` reste `web` même si le MCP la rouvre.

use deadpool_postgres::Pool;
use lj_dtos::{ActivitySource, DecisionViewItem, JurisdictionType};

use crate::bookmarks::resolve_decision_pk;
use crate::error::ApiError;
use crate::me::ts_to_rfc3339;
use crate::referential::{referential, Referential};
use crate::search_history::source_value;
use crate::state::AppState;

/// Désérialise une valeur TEXT de juridiction vers l'enum.
fn parse_jurisdiction_type(raw: &str) -> Result<JurisdictionType, ApiError> {
    serde_json::from_value(serde_json::Value::String(raw.to_string()))
        .map_err(|e| ApiError::Internal(format!("jurisdiction_type invalide {raw:?}: {e}")))
}

/// Désérialise un TEXT `web`/`mcp` lu en base vers l'enum.
fn parse_source(raw: &str) -> Result<ActivitySource, ApiError> {
    match raw {
        "web" => Ok(ActivitySource::Web),
        "mcp" => Ok(ActivitySource::Mcp),
        other => Err(ApiError::Internal(format!("source invalide {other:?}"))),
    }
}

/// Upsert d'une consultation (parité `_upsert_view`).
///
/// `ON CONFLICT` : bump `view_count`, `last_viewed_at = now()`, et `last_source`
/// avec priorité `web` (si l'EXCLUDED ou l'existant est `web`, on reste `web`).
/// Gaté par `track_activity` (`WHERE EXISTS`).
async fn upsert_view(
    pool: &Pool,
    user_sub: &str,
    decision_id: i64,
    source: ActivitySource,
) -> Result<(), ApiError> {
    let src = source_value(source);
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "INSERT INTO user_decision_views (user_sub, decision_id, last_source) \
         SELECT $1, $2, $3 \
         WHERE EXISTS (SELECT 1 FROM users WHERE sub = $4 AND track_activity) \
         ON CONFLICT (user_sub, decision_id) DO UPDATE \
         SET view_count     = user_decision_views.view_count + 1, \
             last_source    = CASE \
                 WHEN EXCLUDED.last_source = 'web' \
                   OR user_decision_views.last_source = 'web' \
                 THEN 'web' \
                 ELSE EXCLUDED.last_source \
             END, \
             last_viewed_at = now()",
        &[&user_sub, &decision_id, &src, &user_sub],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("upsert_view: {e}")))?;
    Ok(())
}

/// Enregistre une consultation. Échec silencieux (best-effort, parité
/// `record_decision_view`).
///
/// Utilisé par le hook MCP `get_decision` : ne doit jamais faire échouer la
/// lecture. La résolution `public_id` → `id` interne sert d'existence-check.
pub async fn record_decision_view(
    pool: &Pool,
    user_sub: &str,
    decision_public_id: &str,
    source: ActivitySource,
) {
    let res: Result<(), ApiError> = async {
        let db_id = match resolve_decision_pk(pool, decision_public_id).await? {
            None => return Ok(()),
            Some(id) => id,
        };
        upsert_view(pool, user_sub, db_id, source).await
    }
    .await;
    if let Err(exc) = res {
        tracing::warn!(error = %exc, "user_decision_views upsert failed");
    }
}

/// Page de lectures (plus récentes d'abord) + total complet (parité
/// `fetch_views`). `total` via `COUNT(*) OVER()`. Solution et nom de
/// juridiction résolus depuis le référentiel (ADR 0146).
pub async fn fetch_views(
    pool: &Pool,
    refs: &Referential,
    user_sub: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<(Vec<DecisionViewItem>, i64), ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let rows = conn
        .query(
            "SELECT \
               d.public_id, \
               d.jurisdiction_type, \
               d.jurisdiction_code, \
               to_char(d.date_lecture, 'YYYY-MM-DD') AS date_lecture, \
               d.docket_numbers, \
               d.solution_uid, \
               d.summary, \
               v.view_count, \
               v.last_source, \
               v.last_viewed_at, \
               COUNT(*) OVER() AS total, \
               d.chamber_position, \
               d.formation_uid, \
               d.office_uid \
             FROM user_decision_views v \
             JOIN decisions d ON d.id = v.decision_id \
             WHERE v.user_sub = $1 \
             ORDER BY v.last_viewed_at DESC \
             LIMIT $2 OFFSET $3",
            &[&user_sub, &limit, &offset],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("fetch_views: {e}")))?;

    let total = rows.first().map(|r| r.get::<_, i64>(10)).unwrap_or(0);
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
        // Titre canonique (ADR 0146 §4 / 0170) : siège recomposé depuis les
        // axes structurés.
        let jur_display = crate::titles::decision_jurisdiction(
            refs.jurisdiction_type_label(jur_type_raw)
                .unwrap_or(jur_type_raw),
            jurisdiction_name.as_deref(),
        );
        let seat = crate::titles::decision_seat(
            &jur_display,
            row.get::<_, Option<String>>(11).as_deref(),
            row.get::<_, Option<String>>(12).as_deref(),
            row.get::<_, Option<String>>(13).as_deref(),
        );
        let title = lj_core::titles::decision_title(
            &jur_display,
            seat.as_deref(),
            date_lecture.as_deref(),
            docket_numbers
                .as_deref()
                .and_then(|d| d.first())
                .map(String::as_str),
        );
        items.push(DecisionViewItem {
            id: row.get(0),
            title,
            jurisdiction_type: parse_jurisdiction_type(jur_type_raw)?,
            jurisdiction_name,
            date_lecture,
            docket_numbers,
            solution: solution_uid.as_deref().map(|u| refs.tag(u)),
            summary: row.get(6),
            // view_count est INTEGER en base ; le DTO l'expose en i64.
            view_count: i64::from(row.get::<_, i32>(7)),
            last_source: parse_source(row.get::<_, &str>(8))?,
            last_viewed_at: ts_to_rfc3339(row.get(9)),
        });
    }
    Ok((items, total))
}

/// `GET /me/decision-views` (parité `list_views`). `limit` borné `[1, 100]` à la
/// frontière HTTP.
pub async fn list_views(
    state: &AppState,
    user_sub: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<DecisionViewItem>, i64), ApiError> {
    let refs = referential(state).await?;
    fetch_views(&state.pool, &refs, user_sub, Some(limit), offset).await
}

/// `POST /me/decision-views/{decision_id}` — enregistre une consultation web
/// (parité `record_view`). 404 si la décision n'existe pas.
pub async fn record_view(
    state: &AppState,
    user_sub: &str,
    public_id: &str,
) -> Result<(), ApiError> {
    let db_id = resolve_decision_pk(&state.pool, public_id)
        .await?
        .ok_or(ApiError::NotFound)?; // detail=decision_not_found
    upsert_view(&state.pool, user_sub, db_id, ActivitySource::Web).await
}

/// `DELETE /me/decision-views/{decision_id}` (parité `delete_view`).
/// 404 si la décision n'existe pas (`decision_not_found`) ou si aucune ligne de
/// consultation (`view_not_found`).
pub async fn delete_view(
    state: &AppState,
    user_sub: &str,
    public_id: &str,
) -> Result<(), ApiError> {
    let db_id = resolve_decision_pk(&state.pool, public_id)
        .await?
        .ok_or(ApiError::NotFound)?; // detail=decision_not_found
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let affected = conn
        .execute(
            "DELETE FROM user_decision_views WHERE user_sub = $1 AND decision_id = $2",
            &[&user_sub, &db_id],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("delete_view: {e}")))?;
    if affected == 0 {
        return Err(ApiError::NotFound); // detail=view_not_found
    }
    Ok(())
}

/// `DELETE /me/decision-views` — purge complète (parité `clear_views`).
pub async fn clear_views(state: &AppState, user_sub: &str) -> Result<(), ApiError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "DELETE FROM user_decision_views WHERE user_sub = $1",
        &[&user_sub],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("clear_views: {e}")))?;
    Ok(())
}
