//! Listing by date, the index sorting and cutting what the counts cover whole: the
//! filtered stock without a question, and the question of a date-sorted search.
//!
//! « Textes cités » is served here like the other axes, the index rendering it in
//! the same pass.

use std::sync::Arc;

use deadpool_postgres::Client;
use lj_dtos::{QueryMode, SearchFacets, SearchHit, SearchRequest, SearchResponse, SortOrder};

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

use super::aggregate::{aggregate_facets, Aggs, Scope};
use super::filters::{as_refs, Params};
use super::hydrate::{facets_from_counters, FacetCounters};
use super::query::query_lacks_searchable_terms;

/// Does the request carry at least one filter? With neither question nor filter,
/// the page is the landing on `/decision-search`, not a search.
pub(crate) fn has_filters(req: &SearchRequest) -> bool {
    fn some<T>(v: &Option<Vec<T>>) -> bool {
        v.as_deref().is_some_and(|s| !s.is_empty())
    }
    some(&req.jurisdiction_type)
        || some(&req.solution)
        || some(&req.formation)
        || some(&req.legal_domain)
        || some(&req.jurisdiction_code)
        || some(&req.seat)
        || some(&req.legal_instrument)
        || some(&req.legal_article)
        || some(&req.publication)
        || req.date_from.is_some()
        || req.date_to.is_some()
        || super::filters::conclusions(req)
}

/// The reader's jurisdiction a browse holds to. France's reads the whole fund.
pub(crate) fn browsed_home(req: &SearchRequest) -> Option<&str> {
    req.home.as_deref().filter(|home| *home != "fr")
}

/// Sans requête, la pertinence vaut la date décroissante.
fn order_by(sort: SortOrder, date: &str, id: &str) -> String {
    match sort {
        SortOrder::DateAsc => format!("{date} ASC NULLS LAST, {id}"),
        _ => format!("{date} DESC NULLS LAST, {id}"),
    }
}

/// Largeur de la liste que la navigation latérale parcourt, ici comme en
/// recherche : les flèches traversent la page au lieu de mourir à son bord.
const NAV_WINDOW: i64 = 100;

/// Fenêtre `[start, start + len)` ancrée sur un multiple de [`NAV_WINDOW`], et
/// élargie quand la page demandée déborderait de son ancrage.
fn nav_window(offset: i64, limit: i64) -> (i64, i64) {
    let start = offset - offset % NAV_WINDOW;
    (start, NAV_WINDOW.max(offset + limit - start))
}

/// Ids and `public_id` of the window `[start, start + len)`, which covers the
/// requested page. Under a filter the index sorts and truncates, through the
/// predicate that already serves the counts: the heap would walk its date index
/// discarding at the filter, slow on a rare selection, where the index costs the
/// same whatever the selectivity. Without a filter the heap stays the shortest,
/// index only.
async fn window_ids(
    conn: &Client,
    req: &SearchRequest,
    start: i64,
    len: i64,
) -> Result<Vec<(i64, String)>> {
    let mut params: Params = Vec::new();
    let question = !query_lacks_searchable_terms(&req.query);
    let filtered = (question || has_filters(req) || browsed_home(req).is_some()).then(|| {
        let mut idx = 1usize;
        super::aggregate::build_predicate(req, &mut idx, &mut params)
    });
    let (li, oi, fi) = (params.len() + 1, params.len() + 2, params.len() + 3);
    // A question matches every text of a decision, and few carry more than three.
    let fetch = (start + len) * if question { 3 } else { 1 };
    let mut refs = as_refs(&params);
    refs.push(&len);
    refs.push(&start);
    let sql = match filtered {
        Some(predicate) => {
            refs.push(&fetch);
            format!(
                "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code FROM ( \
                   SELECT DISTINCT ON (w.date_lecture, w.decision_id) w.decision_id, w.date_lecture \
                   FROM ( \
                     SELECT t.decision_id, t.date_lecture FROM decision_text t \
                     WHERE t.id @@@ {predicate} ORDER BY {} LIMIT ${fi} \
                   ) w ORDER BY {} LIMIT ${li} OFFSET ${oi} \
                 ) p JOIN decisions d ON d.id = p.decision_id \
                 ORDER BY {}",
                order_by(req.sort, "t.date_lecture", "t.decision_id"),
                order_by(req.sort, "w.date_lecture", "w.decision_id"),
                order_by(req.sort, "p.date_lecture", "p.decision_id"),
            )
        }
        None => format!(
            "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code \
             FROM decisions d WHERE d.deleted_at IS NULL \
             ORDER BY {} LIMIT ${li} OFFSET ${oi}",
            order_by(req.sort, "d.date_lecture", "d.id"),
        ),
    };
    let rows = conn.query(&sql, &refs).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let jt = lj_dtos::JurisdictionType::from_code(r.get(2))
                .expect("a stored jurisdiction_type is a known variant");
            (r.get(0), lj_dtos::decision_path(jt, r.get(3), r.get(1)))
        })
        .collect())
}

/// Total and facets of the filtered fund, counted in the index by the same
/// aggregate as the text search, several times faster than a full-table
/// `GROUPING SETS`.
async fn live_totals_and_facets(
    state: &AppState,
    req: &SearchRequest,
    refs: &Referential,
) -> Result<(i64, SearchFacets)> {
    let conn = super::client(state).await?;
    let (total, counters) = aggregate_facets(&conn, req, Scope::Question(&[]), Aggs::All).await?;
    Ok((total, facets_from_counters(&counters, refs)))
}

/// Without any filter, the state of the fonds changes once a night: it is read
/// from the snapshot the chain recounts, without scanning the table.
async fn snapshot_totals_and_facets(
    state: &AppState,
    refs: &Referential,
) -> Result<(i64, SearchFacets)> {
    let conn = super::client(state).await?;
    let rows = conn
        .query(
            "SELECT c.dimension, c.value, c.parent, c.n, lt.title, lt.slug \
             FROM corpus_counts c \
             LEFT JOIN legal_text lt \
               ON c.dimension = 'legal_instrument' AND lt.text_uid = c.value",
            &[],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("browse snapshot: {e}")))?;
    let mut total = 0i64;
    let mut c = FacetCounters::default();
    for row in &rows {
        let dimension: String = row.get(0);
        let value: String = row.get(1);
        let parent: Option<String> = row.get(2);
        let n: i64 = row.get(3);
        match (dimension.as_str(), parent) {
            ("total", _) => total = n,
            ("seat", _) => drop(c.seats.insert(value, n)),
            ("formation", _) => drop(c.formation.insert(value, n)),
            ("legal_domain", _) => drop(c.domain.insert(value, n)),
            ("solution", _) => drop(c.solution.insert(value, n)),
            ("year", _) => drop(c.year.insert(value, n)),
            ("publication_codes", _) => {
                let codes: Vec<String> = value.split(',').map(str::to_string).collect();
                for key in lj_core::publication::publication_keys(&codes) {
                    *c.publication
                        .entry(format!("publication:{key}"))
                        .or_insert(0) += n;
                }
            }
            ("legal_instrument", _) => {
                if let Some(title) = row.get::<_, Option<String>>(4) {
                    c.instrument.labels.insert(value.clone(), title);
                }
                c.instrument.slugs.insert(value.clone(), row.get(5));
                c.instrument.counts.insert(value, n);
            }
            ("legal_article", Some(uid)) => drop(
                c.instrument
                    .articles
                    .entry(uid)
                    .or_default()
                    .insert(value, n),
            ),
            _ => {}
        }
    }
    Ok((total, facets_from_counters(&c, refs)))
}

/// The browse cache key, the filters alone: the total and the facets depend on
/// neither the page, the order, the mode nor the parts, only on whether
/// conclusions are asked for.
fn cache_key(req: &SearchRequest) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = SortOrder::Relevance;
    norm.mode = lj_dtos::SearchMode::Auto;
    norm.ai_mode = false;
    norm.search_in = None;
    format!(
        "{}{}",
        serde_json::to_string(&norm).unwrap_or_default(),
        super::filters::conclusions(req)
    )
}

/// Total et facettes, du cache ou calculés. Single-flight : `optionally_get_with`
/// ne laisse qu'un appelant payer le calcul.
pub(crate) async fn totals_and_facets(
    state: &AppState,
    req: &SearchRequest,
    refs: &Referential,
) -> Result<Arc<(i64, SearchFacets)>> {
    let unfiltered = !has_filters(req) && query_lacks_searchable_terms(&req.query);
    if let Some(home) = browsed_home(req).filter(|_| unfiltered) {
        if let Some(warm) = state.home_browse_cache.get(home).await {
            return Ok(warm);
        }
    }
    let mut own_err: Option<ApiError> = None;
    let mut paye = false;
    let hit = state
        .browse_cache
        .optionally_get_with(cache_key(req), async {
            paye = true;
            let computed = if !unfiltered || browsed_home(req).is_some() {
                live_totals_and_facets(state, req, refs).await
            } else {
                snapshot_totals_and_facets(state, refs).await
            };
            match computed {
                Ok(v) => Some(Arc::new(v)),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("parcours", !paye);
    match (hit, own_err) {
        (Some(v), _) => Ok(v),
        (None, Some(err)) => Err(err),
        (None, None) => Err(ApiError::Internal("browse: cache sans valeur".into())),
    }
}

pub(crate) async fn browse(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    let refs = referential(state).await?;

    let (offset, limit) = (i64::from(req.offset), i64::from(req.limit));
    let (window_start, window_len) = nav_window(offset, limit);

    let question = !query_lacks_searchable_terms(&req.query);
    let list_conn = super::client(state).await?;
    // Under a question, the aggregate outlasts the page: it waits for the count
    // call, as relevance does.
    let (window, counts) = tokio::try_join!(
        window_ids(&list_conn, req, window_start, window_len),
        async {
            match question {
                true => Ok(None),
                false => totals_and_facets(state, req, &refs).await.map(Some),
            }
        },
    )?;

    let page: Vec<i64> = window
        .iter()
        .skip((offset - window_start) as usize)
        .take(limit as usize)
        .map(|(id, _)| *id)
        .collect();

    let hits_conn = super::client(state).await?;
    let hits: Vec<SearchHit> =
        super::hits_for_decision_ids(&hits_conn, &page, &req.query, &refs, super::reader(req))
            .await?
            .into_iter()
            .map(|(_, hit)| hit)
            .collect();
    let all_hit_paths: Vec<String> = window.into_iter().map(|(_, path)| path).collect();

    Ok(SearchResponse {
        query: req.query.clone(),
        total: counts.as_ref().map(|c| c.0),
        hits,
        query_mode: match question {
            true => QueryMode::Lexical,
            false => QueryMode::Browse,
        },
        facets: counts.map(|c| c.1.clone()),
        all_hit_paths,
        reranked: false,
        pinned: Vec::new(),
        pinned_more: false,
    })
}

/// When the homes are counted each day, after the night chain.
const WARM_AT: (u32, u32) = (7, 30);

/// The night chain's window, 02:45 to 07:00Z, in which nothing is counted here.
fn in_night(time: chrono::NaiveTime) -> bool {
    let at = |h, m| chrono::NaiveTime::from_hms_opt(h, m, 0).expect("a valid time");
    (at(2, 45)..at(7, 0)).contains(&time)
}

/// Counts every reader jurisdiction's browse without a question, at start outside the night
/// chain's window and then each day at [`WARM_AT`], so no reader waits on it.
pub async fn keep_homes_warm(state: AppState) {
    if !in_night(chrono::Utc::now().time()) {
        warm_homes(&state).await;
    }
    loop {
        tokio::time::sleep(until_next_warm(chrono::Utc::now())).await;
        warm_homes(&state).await;
    }
}

fn until_next_warm(now: chrono::DateTime<chrono::Utc>) -> std::time::Duration {
    let at = chrono::NaiveTime::from_hms_opt(WARM_AT.0, WARM_AT.1, 0).expect("a valid time");
    let today = now.date_naive().and_time(at).and_utc();
    let next = match today > now {
        true => today,
        false => today + chrono::Duration::days(1),
    };
    (next - now).to_std().expect("the next warm is ahead")
}

/// One home at a time: Switzerland's scope alone took 12 s counted live.
async fn warm_homes(state: &AppState) {
    let started = std::time::Instant::now();
    let refs = match referential(state).await {
        Ok(refs) => refs,
        Err(e) => {
            tracing::warn!(error = %e, "home browse warm-up: no referential");
            return;
        }
    };
    let mut warmed = 0usize;
    for jurisdiction in crate::jurisdictions::known(&refs) {
        let req: SearchRequest =
            serde_json::from_value(serde_json::json!({ "query": "", "home": jurisdiction.code }))
                .expect("a request naming its home alone");
        let Some(home) = browsed_home(&req) else {
            continue;
        };
        match live_totals_and_facets(state, &req, &refs).await {
            Ok(counts) => {
                state
                    .home_browse_cache
                    .insert(home.to_string(), Arc::new(counts))
                    .await;
                warmed += 1;
            }
            Err(e) => tracing::warn!(home, error = %e, "home browse warm-up failed"),
        }
    }
    tracing::info!(
        warmed,
        elapsed_s = started.elapsed().as_secs(),
        "home browses warmed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_window_couvre_toujours_la_page_demandee() {
        for (offset, limit) in [(0, 10), (40, 10), (140, 10), (60, 60), (0, 100), (250, 25)] {
            let (start, len) = nav_window(offset, limit);
            assert!(start <= offset, "fenetre {start} apres la page {offset}");
            assert!(
                offset + limit <= start + len,
                "page {offset}+{limit} hors de la fenetre {start}+{len}"
            );
            assert!(len >= NAV_WINDOW);
        }
        assert_eq!(nav_window(0, 10), (0, NAV_WINDOW));
        assert_eq!(nav_window(140, 10), (100, NAV_WINDOW));
    }

    #[test]
    fn the_homes_are_counted_after_the_night_chain_and_never_inside_it() {
        let at = |h, m| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, 20)
                .and_then(|d| d.and_hms_opt(h, m, 0))
                .expect("a valid instant")
                .and_utc()
        };
        let hours = |h: u64, m: u64| std::time::Duration::from_secs(h * 3600 + m * 60);
        assert_eq!(until_next_warm(at(6, 0)), hours(1, 30));
        assert_eq!(until_next_warm(at(7, 30)), hours(24, 0));
        assert_eq!(until_next_warm(at(21, 0)), hours(10, 30));
        assert!(in_night(at(2, 45).time()) && in_night(at(6, 59).time()));
        assert!(!in_night(at(2, 44).time()) && !in_night(at(7, 0).time()));
    }
}
