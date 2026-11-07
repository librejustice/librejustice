//! Jambes de récupération (BM25 body, BM25 titre, ANN) + pooling décision,
//! fusion RRF et troncature relative.

use std::collections::{HashMap, HashSet};

use deadpool_postgres::Client;
use lj_store::error::StoreError;
use pgvector::Vector;
use tokio_postgres::types::ToSql;

use lj_core::docket_key::QueryNumber;
use lj_dtos::SearchRequest;

use super::filters::{
    as_refs, build_array_filter_queries, build_facet_filter, compose_tantivy_query, parts_query,
    passage_tags, term_any, ArrayFilters, FilterTable, Params,
};
use super::query::{
    constrained_parse, is_boolean_query, phrase_combo_parse, quoted_constraint, translate_boolean,
};

const MAX_RESULTS: usize = 100;
const RELATIVE_SCORE_THRESHOLD: f64 = 0.3;
const RRF_RANK_CONSTANT: f64 = 60.0;
/// Sur-récupération chunk des jambes body/ANN : la LIMIT SQL est au grain
/// chunk, le classement de jambe au grain décision — ×1,1 suffit pour que
/// `leg_limit` chunks couvrent `leg_limit` décisions distinctes.
const POOL_OVERFETCH: f64 = 1.1;

/// LIMIT chunk d'une jambe body/ANN pour `leg_limit` décisions visées.
fn chunk_fetch_limit(leg_limit: i64) -> i64 {
    (leg_limit as f64 * POOL_OVERFETCH).ceil() as i64
}

// ── Hits internes ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct LegHit {
    pub(crate) decision_id: i64,
    pub(crate) chunk_id: i64,
    pub(crate) chunk_index: i32,
    pub(crate) raw_score: f64,
    pub(crate) snippet: Option<String>,
}

impl LegHit {
    pub(crate) fn synthetic_title_only(decision_id: i64) -> Self {
        LegHit {
            decision_id,
            chunk_id: -1,
            chunk_index: 0,
            raw_score: 0.0,
            snippet: None,
        }
    }
}

// ── Jambes (BM25 body, title, ANN) ───────────────────────────────────────────

/// A decision scores by its best text, never by the sum of its languages.
fn best_per_decision(hits: impl Iterator<Item = (i64, f64)>) -> HashMap<i64, f64> {
    let mut best: HashMap<i64, f64> = HashMap::new();
    for (id, score) in hits {
        best.entry(id)
            .and_modify(|s| *s = s.max(score))
            .or_insert(score);
    }
    best
}

#[tracing::instrument(skip(conn, req), fields(leg_limit))]
pub(crate) async fn bm25_leg(
    conn: &Client,
    req: &SearchRequest,
    leg_limit: i64,
) -> std::result::Result<HashMap<i64, f64>, StoreError> {
    let body_query = match quoted_constraint(&req.query) {
        Some(constraint) => constrained_parse(&req.query, &constraint),
        None if is_boolean_query(&req.query) => translate_boolean(&req.query),
        None => phrase_combo_parse(&req.query),
    };
    bm25_parse_leg(conn, req, &body_query, leg_limit).await
}

/// BM25 body leg on `decision_text_bm25`, in the parts the search asks: the SQL
/// `LIMIT` counts texts, a decision keeping its best one. Returns
/// `{decision_id: score}`, like [`title_leg`].
#[tracing::instrument(skip(conn, req, translated_query), fields(leg_limit))]
pub(crate) async fn bm25_parse_leg(
    conn: &Client,
    req: &SearchRequest,
    translated_query: &str,
    leg_limit: i64,
) -> std::result::Result<HashMap<i64, f64>, StoreError> {
    let mut params: Params = Vec::new();
    params.push(Box::new(translated_query.to_string()));
    let mut idx = 2usize;
    let array_filters = build_array_filter_queries(req, &mut idx, &mut params);
    let query_expr = compose_tantivy_query(&parts_query(req, "$1"), array_filters);
    let facet_filter = build_facet_filter(
        req,
        &mut idx,
        &mut params,
        ArrayFilters::Tantivy,
        FilterTable::Decisions,
    );
    let sql = format!(
        "SELECT d.decision_id, paradedb.score(d.id) AS score \
         FROM decision_text d \
         WHERE d.id @@@ {query_expr}{ff} \
         ORDER BY score DESC LIMIT {limit}",
        ff = facet_filter,
        limit = leg_limit,
    );
    let rows = conn.query(sql.as_str(), &as_refs(&params)).await?;
    Ok(best_per_decision(
        rows.iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1) as f64)),
    ))
}

/// BM25 sur le champ `search_title` de `decision_text_bm25`, au grain décision.
/// Renvoie `{decision_id: score}`. Le titre porte le nom d'usage, absent du
/// texte de sa propre décision et sans autre jambe pour le servir.
///
/// Mêmes filtres que la jambe body : TOUS les filtres (article composite et
/// instrument compris) s'appliquent ici aussi — la jambe titre ne peut pas
/// réinjecter de décisions hors-filtre dans la fusion RRF.
#[tracing::instrument(skip(conn, req), fields(leg_limit))]
pub(crate) async fn title_leg(
    conn: &Client,
    req: &SearchRequest,
    leg_limit: i64,
) -> std::result::Result<HashMap<i64, f64>, StoreError> {
    let mut params: Params = Vec::new();
    let title_query = if is_boolean_query(&req.query) {
        params.push(Box::new(translate_boolean(&req.query)) as Box<dyn ToSql + Sync + Send>);
        "paradedb.parse_with_field('search_title', $1)"
    } else {
        params.push(Box::new(req.query.clone()) as Box<dyn ToSql + Sync + Send>);
        "paradedb.match('search_title', $1)"
    };
    let mut idx = 2usize;
    let array_filters = build_array_filter_queries(req, &mut idx, &mut params);
    let query_expr = compose_tantivy_query(title_query, array_filters);
    let facet_filter = build_facet_filter(
        req,
        &mut idx,
        &mut params,
        ArrayFilters::Tantivy,
        FilterTable::Decisions,
    );
    let sql = format!(
        "SELECT d.decision_id, paradedb.score(d.id) AS score \
         FROM decision_text d \
         WHERE d.id @@@ {query_expr}{ff} \
         ORDER BY score DESC LIMIT {limit}",
        ff = facet_filter,
        limit = leg_limit,
    );
    let rows = conn.query(sql.as_str(), &as_refs(&params)).await?;
    Ok(best_per_decision(
        rows.iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1) as f64)),
    ))
}

/// What the query writes beside its number, read against the referential.
#[derive(Debug, Default)]
pub(crate) struct Written {
    pub(crate) kind: Option<&'static str>,
    pub(crate) code: Option<String>,
    pub(crate) day: Option<String>,
}

fn written_clauses(written: &Written, idx: &mut usize, params: &mut Params) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut push = |field: &str, cast: &str, value: String, idx: &mut usize| {
        params.push(Box::new(value) as Box<dyn ToSql + Sync + Send>);
        clauses.push(format!("paradedb.term('{field}', ${idx}::text{cast})"));
        *idx += 1;
    };
    if let Some(kind) = written.kind {
        push("jurisdiction_type", "", kind.to_string(), idx);
    }
    if let Some(code) = &written.code {
        push("jurisdiction_code", "", code.clone(), idx);
    }
    if let Some(day) = &written.day {
        push("date_lecture", "::date", day.clone(), idx);
    }
    clauses
}

/// The one query of the reference retrieval. `docket_keys` and `ecli` carry the
/// canonical form, insensitive to the graphy, so they are what ADMITS, with
/// `docket_search`, the same keys plus the digits of a lettered number: an
/// exact bearer matches both key fields and ranks above one reached by its
/// digits alone;
/// `search_title` carries the graphy as written, so it only RANKS, hence its
/// place in `should`, beside the court and the day the query writes. Left a
/// peer clause it admits on one token: `ECLI:FR:CCASS:2007:C200527` matches
/// every title carrying `2007`.
///
/// No boost on the keys. Every hit bears one, so weighting them only flattens
/// the gap the title opens between bearers, and `truncate_fused` cuts on that
/// gap.
/// What the reference retrieval looks for: numbers, ECLIs, or the decisions a
/// name designates.
#[derive(Debug, Default)]
pub(crate) struct ReferenceKeys {
    pub(crate) numbers: Vec<QueryNumber>,
    pub(crate) eclis: Vec<String>,
    pub(crate) bearers: Vec<i64>,
}

/// A number or an ECLI the query writes admits its bearers, and so does a
/// name; the title, which carries the name, ranks them.
fn reference_query(
    req: &SearchRequest,
    reference: &ReferenceKeys,
    written: &Written,
    idx: &mut usize,
    params: &mut Params,
) -> String {
    let title = if is_boolean_query(&req.query) {
        params.push(Box::new(translate_boolean(&req.query)) as Box<dyn ToSql + Sync + Send>);
        format!("paradedb.parse_with_field('search_title', ${idx})")
    } else {
        params.push(Box::new(req.query.clone()) as Box<dyn ToSql + Sync + Send>);
        format!("paradedb.match('search_title', ${idx})")
    };
    *idx += 1;
    let mut keys: Vec<String> = Vec::new();
    for n in &reference.numbers {
        params.push(Box::new(n.key.clone()) as Box<dyn ToSql + Sync + Send>);
        let key = format!(
            "paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', ${idx}::text), \
             paradedb.term('docket_search', ${idx}::text)])"
        );
        *idx += 1;
        keys.push(match n.courts.is_empty() {
            true => key,
            false => {
                let courts: Vec<String> = n.courts.iter().map(|c| (*c).to_string()).collect();
                let route = term_any("jurisdiction_type", &courts, idx, params);
                format!("paradedb.boolean(must => ARRAY[{key}, {route}])")
            }
        });
    }
    for e in &reference.eclis {
        params.push(Box::new(e.to_lowercase()) as Box<dyn ToSql + Sync + Send>);
        keys.push(format!("paradedb.term('ecli', ${idx}::text)"));
        *idx += 1;
    }
    if !reference.bearers.is_empty() {
        let ids: Vec<String> = reference
            .bearers
            .iter()
            .map(|id| format!("paradedb.term('decision_id', {id}::bigint)"))
            .collect();
        return format!(
            "paradedb.boolean(must => ARRAY[paradedb.term_set(terms => ARRAY[{}])], should => ARRAY[{title}])",
            ids.join(", ")
        );
    }
    let admits = match keys.len() {
        0 => return title,
        1 => keys.remove(0),
        _ => format!("paradedb.boolean(should => ARRAY[{}])", keys.join(", ")),
    };
    let ranks = std::iter::once(title)
        .chain(written_clauses(written, idx, params))
        .collect::<Vec<_>>()
        .join(", ");
    format!("paradedb.boolean(must => ARRAY[{admits}], should => ARRAY[{ranks}])")
}

/// Single leg of the reference retrieval: the title, the canonical docket keys
/// and the ECLIs are clauses of one BM25 query, so a bearer that satisfies two
/// of them ranks above one that satisfies a single one, and that one is still
/// served.
///
/// Rows come back in final order, the citation count breaking a score tie
/// (spans of `case_citation` and renvois of the Conseil d'Etat). The
/// `MATERIALIZED` CTE keeps those two correlated subqueries on the rows the
/// score already selected instead of the whole table.
#[tracing::instrument(skip(conn, req, reference, written), fields(leg_limit))]
pub(crate) async fn reference_leg(
    conn: &Client,
    req: &SearchRequest,
    reference: &ReferenceKeys,
    written: &Written,
    leg_limit: i64,
) -> std::result::Result<Vec<(i64, f64)>, StoreError> {
    let mut params: Params = Vec::new();
    let mut idx = 1usize;
    let primary = reference_query(req, reference, written, &mut idx, &mut params);
    let array_filters = build_array_filter_queries(req, &mut idx, &mut params);
    let query_expr = compose_tantivy_query(&primary, array_filters);
    let facet_filter = build_facet_filter(
        req,
        &mut idx,
        &mut params,
        ArrayFilters::Tantivy,
        FilterTable::Decisions,
    );
    let _ = idx;
    let citations = "(SELECT count(*) FROM case_citation cc WHERE cc.target_decision_id = h.id) \
                     + (SELECT count(*) FROM decision_renvoi r WHERE r.target_decision_id = h.id) DESC";
    // Every bearer of a name matches the title, whose score then measures only its length.
    let order = match reference.bearers.is_empty() {
        true => format!("h.score DESC, {citations}"),
        false => format!("{citations}, h.score DESC"),
    };
    let sql = format!(
        "WITH hits AS MATERIALIZED ( \
           SELECT d.decision_id AS id, d.date_lecture, paradedb.score(d.id) AS score \
           FROM decision_text d \
           WHERE d.id @@@ {query_expr} AND d.deleted_at IS NULL{ff} \
           ORDER BY score DESC LIMIT {limit} \
         ) \
         SELECT h.id, h.score FROM hits h \
         ORDER BY {order}, h.date_lecture DESC NULLS LAST, h.id DESC",
        ff = facet_filter,
        limit = leg_limit,
    );
    let rows = conn.query(sql.as_str(), &as_refs(&params)).await?;
    let mut seen = std::collections::HashSet::new();
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1) as f64))
        .filter(|(id, _)| seen.insert(*id))
        .collect())
}

/// ANN VectorChord sur `decision_chunks.embedding` (pre-filter conditionnel).
#[tracing::instrument(skip(conn, req, query_vec), fields(leg_limit, probes))]
pub(crate) async fn ann_leg(
    conn: &mut Client,
    req: &SearchRequest,
    query_vec: &[f32],
    leg_limit: i64,
    probes: u32,
) -> std::result::Result<Vec<LegHit>, StoreError> {
    let mut params: Params = Vec::new();
    params.push(Box::new(Vector::from(query_vec.to_vec())));
    let mut idx = 2usize;
    let chunk_filter = build_facet_filter(
        req,
        &mut idx,
        &mut params,
        ArrayFilters::AsSql,
        FilterTable::Chunks,
    );
    let has_filter = params.len() > 1;
    let limit_idx = idx;
    params.push(Box::new(chunk_fetch_limit(leg_limit)));
    let sql = format!(
        "SELECT c.id, c.decision_id, c.chunk_index, \
         c.embedding <=> quantize_to_rabitq8($1::vector)::rabitq8(1024) AS distance \
         FROM decision_chunks c \
         WHERE c.embedding IS NOT NULL{cf} \
         ORDER BY distance LIMIT ${limit_idx}",
        cf = chunk_filter,
        limit_idx = limit_idx,
    );
    // SET LOCAL dans une transaction (probes + prefilter).
    let tx = conn.transaction().await?;
    tx.batch_execute(&format!(
        "SET LOCAL vchordrq.probes = {probes}; SET LOCAL vchordrq.prefilter = {};",
        if has_filter { "on" } else { "off" }
    ))
    .await?;
    let rows = tx.query(sql.as_str(), &as_refs(&params)).await?;
    tx.commit().await?;
    Ok(rows
        .iter()
        .map(|r| LegHit {
            chunk_id: r.get::<_, i64>(0),
            decision_id: r.get::<_, i64>(1),
            chunk_index: r.get::<_, i32>(2),
            raw_score: -(r.get::<_, f32>(3) as f64),
            snippet: None,
        })
        .collect())
}

/// Up to this many texts holding the constraint, the leg ranks all of them
/// exactly.
const EXACT_MAX: usize = 20_000;
/// Up to this many, the set prefilters the index scan, where the plain scan
/// keeps too few of its 220.
const PREFILTER_MAX: usize = 100_000;
/// Beyond, the constraint holds one decision in thirty or more: the plain scan,
/// ten times deeper.
const POSTFILTER_DEPTH: i64 = 10;

/// The ANN leg inside the decisions holding `constraint`, under the filters.
#[tracing::instrument(skip(conn, req, query_vec), fields(held = tracing::field::Empty))]
pub(crate) async fn ann_within(
    conn: &mut Client,
    req: &SearchRequest,
    constraint: &str,
    query_vec: &[f32],
    leg_limit: i64,
    probes: u32,
) -> std::result::Result<Vec<LegHit>, StoreError> {
    let held_req = SearchRequest {
        query: constraint.to_string(),
        mode: lj_dtos::SearchMode::Lexical,
        ..req.clone()
    };
    let mut params: Params = Vec::new();
    let predicate = super::aggregate::build_predicate(&held_req, &mut 1, &mut params);
    let texts = conn
        .query(
            &format!(
                "SELECT t.decision_id FROM decision_text t WHERE t.id @@@ {predicate} LIMIT {}",
                PREFILTER_MAX + 1
            ),
            &as_refs(&params),
        )
        .await?;
    tracing::Span::current().record("held", texts.len());
    let mut held: Vec<i64> = texts.iter().map(|r| r.get(0)).collect();
    held.sort_unstable();
    held.dedup();
    let vector = Vector::from(query_vec.to_vec());
    let distance = "c.embedding <=> quantize_to_rabitq8($1::vector)::rabitq8(1024)";
    let tags = passage_tags(req);
    let tagged = |idx: usize| match tags {
        Some(_) => format!(" AND c.zones && ${idx}::text[]"),
        None => String::new(),
    };
    let limit = chunk_fetch_limit(leg_limit);
    let tx = conn.transaction().await?;
    let rows = if texts.len() <= EXACT_MAX {
        tx.query(
            &format!(
                "SELECT c.id, c.decision_id, c.chunk_index, {distance} AS distance FROM ( \
                   SELECT id, decision_id, chunk_index, embedding FROM decision_chunks c \
                   WHERE decision_id = ANY($2) AND embedding IS NOT NULL{} OFFSET 0 \
                 ) c ORDER BY distance LIMIT $3",
                tagged(4)
            ),
            &with_tags(vec![&vector, &held, &limit], &tags),
        )
        .await?
    } else if texts.len() <= PREFILTER_MAX {
        tx.batch_execute(&format!(
            "SET LOCAL vchordrq.probes = {probes}; SET LOCAL vchordrq.prefilter = on;"
        ))
        .await?;
        tx.query(
            &format!(
                "SELECT c.id, c.decision_id, c.chunk_index, {distance} AS distance \
                 FROM decision_chunks c \
                 WHERE c.embedding IS NOT NULL AND c.decision_id = ANY($2){} \
                 ORDER BY distance LIMIT $3",
                tagged(4)
            ),
            &with_tags(vec![&vector, &held, &limit], &tags),
        )
        .await?
    } else {
        tx.batch_execute(&format!(
            "SET LOCAL vchordrq.probes = {probes}; SET LOCAL vchordrq.prefilter = {};",
            if tags.is_some() { "on" } else { "off" }
        ))
        .await?;
        let depth = chunk_fetch_limit(leg_limit * POSTFILTER_DEPTH);
        let candidates = tx
            .query(
                &format!(
                    "SELECT c.id, c.decision_id, c.chunk_index, {distance} AS distance \
                     FROM decision_chunks c WHERE c.embedding IS NOT NULL{} \
                     ORDER BY distance LIMIT $2",
                    tagged(3)
                ),
                &with_tags(vec![&vector, &depth], &tags),
            )
            .await?;
        let ids: Vec<i64> = candidates.iter().map(|r| r.get(1)).collect();
        let mut params: Params = Vec::new();
        let predicate = super::aggregate::build_predicate(&held_req, &mut 1, &mut params);
        let terms: Vec<String> = ids
            .iter()
            .map(|id| format!("paradedb.term('decision_id', {id}::bigint)"))
            .collect();
        let kept: HashSet<i64> = tx
            .query(
                &format!(
                    "SELECT DISTINCT t.decision_id FROM decision_text t WHERE t.id @@@ \
                     paradedb.boolean(must => ARRAY[{predicate}, \
                     paradedb.term_set(terms => ARRAY[{}])])",
                    terms.join(", ")
                ),
                &as_refs(&params),
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        candidates
            .into_iter()
            .filter(|r| kept.contains(&r.get::<_, i64>(1)))
            .collect()
    };
    tx.commit().await?;
    Ok(rows
        .iter()
        .map(|r| LegHit {
            chunk_id: r.get::<_, i64>(0),
            decision_id: r.get::<_, i64>(1),
            chunk_index: r.get::<_, i32>(2),
            raw_score: -(r.get::<_, f32>(3) as f64),
            snippet: None,
        })
        .collect())
}

fn with_tags<'a>(
    mut params: Vec<&'a (dyn ToSql + Sync)>,
    tags: &'a Option<Vec<String>>,
) -> Vec<&'a (dyn ToSql + Sync)> {
    if let Some(tags) = tags {
        params.push(tags);
    }
    params
}

// ── Pooling décision + fusion RRF + truncate ─────────────────────────────────

/// Max-pool chunk→décision puis troncature aux `leg_limit` meilleures
/// décisions (score desc, tie-break id) : la jambe a sur-récupéré
/// [`POOL_OVERFETCH`] chunks, le classement entrant en fusion RRF est au grain
/// décision et de profondeur `leg_limit`, comme la jambe titre. Le `LegHit`
/// gagnant est conservé (chunk du snippet).
pub(crate) fn pool_max_per_decision(hits: &[LegHit], leg_limit: usize) -> HashMap<i64, LegHit> {
    let mut out: HashMap<i64, LegHit> = HashMap::new();
    for h in hits {
        match out.get(&h.decision_id) {
            Some(prev) if prev.raw_score >= h.raw_score => {}
            _ => {
                out.insert(h.decision_id, h.clone());
            }
        }
    }
    let mut ranked: Vec<LegHit> = out.into_values().collect();
    ranked.sort_by(|a, b| {
        b.raw_score
            .partial_cmp(&a.raw_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.decision_id.cmp(&b.decision_id))
    });
    ranked.truncate(leg_limit);
    ranked.into_iter().map(|h| (h.decision_id, h)).collect()
}

/// Rangs 1-based décroissants par score (tie-break déterministe sur l'id).
fn ranks_by_score<F: Fn(i64) -> f64>(ids: &[i64], score: F) -> HashMap<i64, usize> {
    let mut sorted: Vec<i64> = ids.to_vec();
    sorted.sort_by(|a, b| {
        score(*b)
            .partial_cmp(&score(*a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    sorted
        .into_iter()
        .enumerate()
        .map(|(r, d)| (d, r + 1))
        .collect()
}

/// The three rankings entering the fusion. `bm25` and `title` are scores per
/// decision; `ann` carries the `LegHit` of the winning chunk, the only leg at
/// chunk grain.
pub(crate) struct Legs<'a> {
    pub(crate) bm25: &'a HashMap<i64, f64>,
    pub(crate) ann: &'a HashMap<i64, LegHit>,
    pub(crate) title: &'a HashMap<i64, f64>,
}

/// Fusion RRF au grain décision, aux poids de [`crate::signals::Weights`]. Le
/// chunk d'affichage du hit fusionné vient de la jambe ANN — il ancre la fenêtre
/// du snippet sémantique ([`super::hydrate::assemble_page`]) — sinon un chunk
/// synthétique (snippet depuis `full_text`).
pub(crate) fn fuse_ranks(legs: Legs<'_>, w: crate::signals::Weights) -> Vec<(i64, f64, LegHit)> {
    let Legs { bm25, ann, title } = legs;
    let bm25_ids: Vec<i64> = bm25.keys().copied().collect();
    let ann_ids: Vec<i64> = ann.keys().copied().collect();
    let title_ids: Vec<i64> = title.keys().copied().collect();
    let bm25_rank = ranks_by_score(&bm25_ids, |d| bm25[&d]);
    let ann_rank = ranks_by_score(&ann_ids, |d| ann[&d].raw_score);
    let title_rank = ranks_by_score(&title_ids, |d| title[&d]);

    let mut all: HashSet<i64> = HashSet::new();
    all.extend(bm25.keys());
    all.extend(ann.keys());
    all.extend(title.keys());

    let mut fused: Vec<(i64, f64, LegHit)> = all
        .into_iter()
        .map(|d| {
            let mut score = 0.0;
            if let Some(r) = bm25_rank.get(&d) {
                score += w.bm25 / (*r as f64 + RRF_RANK_CONSTANT);
            }
            if let Some(r) = ann_rank.get(&d) {
                score += w.ann / (*r as f64 + RRF_RANK_CONSTANT);
            }
            if let Some(r) = title_rank.get(&d) {
                score += w.title / (*r as f64 + RRF_RANK_CONSTANT);
            }
            let chunk = ann
                .get(&d)
                .cloned()
                .unwrap_or_else(|| LegHit::synthetic_title_only(d));
            (d, score, chunk)
        })
        .collect();
    // Tri par score décroissant, tie-break déterministe par id (tri stable).
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    fused
}

/// Coupe la queue faible, relativement au meilleur score.
pub(crate) fn truncate_fused(fused: Vec<(i64, f64, LegHit)>) -> Vec<(i64, f64, LegHit)> {
    let Some(best) = fused.first().map(|r| r.1) else {
        return fused;
    };
    let threshold = best * RELATIVE_SCORE_THRESHOLD;
    fused
        .into_iter()
        .filter(|r| r.1 >= threshold)
        .take(MAX_RESULTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fusion_tiebreak_parity_oracle() {
        let mk = |did: i64, raw_score: f64| LegHit {
            decision_id: did,
            chunk_id: did * 10,
            chunk_index: 0,
            raw_score,
            snippet: None,
        };
        let bm25: HashMap<i64, f64> = HashMap::from([(1, 5.0), (2, 5.0), (3, 1.0)]);
        let ann = HashMap::from([(2, mk(2, 9.0)), (4, mk(4, 2.0))]);
        let title = HashMap::from([(1, 3.0), (4, 3.0)]);
        let fused = fuse_ranks(
            Legs {
                bm25: &bm25,
                ann: &ann,
                title: &title,
            },
            crate::signals::BASE_WEIGHTS,
        );
        let order: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
        assert_eq!(order, vec![1, 2, 4, 3], "ordre fusionné + tie-break");
        let scores: Vec<f64> = fused
            .iter()
            .map(|(_, s, _)| (s * 1e12).round() / 1e12)
            .collect();
        assert_eq!(
            scores,
            vec![
                0.032786885246,
                0.032522474881,
                0.032258064516,
                0.015873015873
            ],
            "scores RRF (rc=60, poids 1/1/1)"
        );
    }

    #[test]
    fn rrf_fusion_title_only_synthetic_chunk_parity_oracle() {
        let mk = |did: i64, raw_score: f64| LegHit {
            decision_id: did,
            chunk_id: did * 10,
            chunk_index: 0,
            raw_score,
            snippet: None,
        };
        let bm25: HashMap<i64, f64> = HashMap::from([(1, 4.0)]);
        let ann = HashMap::from([(1, mk(1, 2.0)), (2, mk(2, 7.0))]);
        let title = HashMap::from([(5, 9.0)]);
        let fused = fuse_ranks(
            Legs {
                bm25: &bm25,
                ann: &ann,
                title: &title,
            },
            crate::signals::BASE_WEIGHTS,
        );
        let order: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
        assert_eq!(order, vec![1, 2, 5], "ordre + tie-break (2 avant 5)");
        let scores: Vec<f64> = fused
            .iter()
            .map(|(_, s, _)| (s * 1e12).round() / 1e12)
            .collect();
        assert_eq!(scores, vec![0.032522474881, 0.016393442623, 0.016393442623]);
        let (_, _, chunk5) = fused.iter().find(|(d, _, _)| *d == 5).unwrap();
        assert_eq!(chunk5.chunk_id, -1, "chunk synthétique title-only");
        assert_eq!(chunk5.chunk_index, 0);
    }

    fn leg_hit(decision_id: i64, chunk_id: i64, raw_score: f64) -> LegHit {
        LegHit {
            decision_id,
            chunk_id,
            chunk_index: 0,
            raw_score,
            snippet: None,
        }
    }

    #[test]
    fn pool_keeps_winning_chunk_of_decision() {
        let hits = vec![
            leg_hit(1, 10, -0.4),
            leg_hit(1, 11, -0.2),
            leg_hit(1, 12, -0.9),
            leg_hit(2, 20, -0.5),
        ];
        let pooled = pool_max_per_decision(&hits, 10);
        assert_eq!(pooled.len(), 2);
        assert_eq!(pooled[&1].chunk_id, 11);
        assert_eq!(pooled[&1].raw_score, -0.2);
        assert_eq!(pooled[&2].chunk_id, 20);
    }

    #[test]
    fn pool_truncates_to_leg_limit_decisions() {
        let hits = vec![
            leg_hit(1, 10, 1.0),
            leg_hit(2, 20, 5.0),
            leg_hit(4, 40, 3.0),
            leg_hit(3, 30, 3.0),
        ];
        let pooled = pool_max_per_decision(&hits, 2);
        assert_eq!(pooled.len(), 2);
        assert!(pooled.contains_key(&2));
        assert!(pooled.contains_key(&3));
    }

    #[test]
    fn pool_recovers_decision_below_chunk_cutoff() {
        let overfetched = vec![
            leg_hit(1, 10, 9.0),
            leg_hit(1, 11, 8.0),
            leg_hit(2, 20, 7.0),
            leg_hit(3, 30, 6.0),
        ];
        let pooled = pool_max_per_decision(&overfetched, 2);
        assert_eq!(pooled.len(), 2);
        assert!(pooled.contains_key(&1));
        assert!(pooled.contains_key(&2));
    }

    #[test]
    fn rrf_pure_three_legs() {
        let mut bm25: HashMap<i64, f64> = HashMap::new();
        bm25.insert(1i64, 5.0);
        let ann: HashMap<i64, LegHit> = HashMap::new();
        let mut title = HashMap::new();
        title.insert(1i64, 3.0);
        title.insert(2i64, 1.0);
        let fused = fuse_ranks(
            Legs {
                bm25: &bm25,
                ann: &ann,
                title: &title,
            },
            crate::signals::BASE_WEIGHTS,
        );
        assert_eq!(fused[0].0, 1);
    }

    fn reference_expr(query: &str, numbers: &[QueryNumber], eclis: &[String]) -> String {
        let req: SearchRequest =
            serde_json::from_value(serde_json::json!({ "query": query })).unwrap();
        let reference = ReferenceKeys {
            numbers: numbers.to_vec(),
            eclis: eclis.to_vec(),
            bearers: Vec::new(),
        };
        let mut idx = 1usize;
        let mut params: Params = Vec::new();
        reference_query(&req, &reference, &Written::default(), &mut idx, &mut params)
    }

    fn nombre(key: &str, courts: &'static [&'static str]) -> QueryNumber {
        QueryNumber {
            key: key.to_string(),
            courts,
            weak: false,
        }
    }

    /// A query with no number is the title clause alone, unwrapped: the `should`
    /// of one is the one.
    #[test]
    fn sans_numero_la_reference_est_la_seule_clause_titre() {
        assert_eq!(
            reference_expr("Epoux Lemonnier", &[], &[]),
            "paradedb.match('search_title', $1)"
        );
    }

    /// The key admits and the title ranks: a title matching one token of the
    /// query brings nothing in on its own.
    #[test]
    fn la_cle_admet_et_le_titre_ne_fait_que_classer() {
        assert_eq!(
            reference_expr("24-10.102", &[nombre("2410102", &[])], &[]),
            "paradedb.boolean(must => ARRAY[paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', $2::text), \
             paradedb.term('docket_search', $2::text)])], \
             should => ARRAY[paradedb.match('search_title', $1)])"
        );
    }

    /// Two keys admit by their union, so a query naming two numbers serves the
    /// bearers of either.
    #[test]
    fn deux_cles_admettent_par_leur_union() {
        let expr = reference_expr(
            "24-10.101 24-10.102",
            &[nombre("2410101", &[]), nombre("2410102", &[])],
            &[],
        );
        assert!(
            expr.starts_with(
                "paradedb.boolean(must => ARRAY[paradedb.boolean(should => ARRAY[\
                 paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', $2::text), \
             paradedb.term('docket_search', $2::text)]), \
                 paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', $3::text), \
             paradedb.term('docket_search', $3::text)])])]"
            ),
            "{expr}"
        );
    }

    /// A graphy that names its courts routes inside the key clause, never on the
    /// whole query: the title of another court is still ranked.
    #[test]
    fn le_routage_des_cours_reste_dans_la_clause_de_la_cle() {
        let expr = reference_expr("24-10.102", &[nombre("2410102", &["CC"])], &[]);
        assert!(
            expr.contains(
                "paradedb.boolean(must => ARRAY[\
                 paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', $2::text), \
             paradedb.term('docket_search', $2::text)]), \
                 paradedb.term('jurisdiction_type', $3::text)])"
            ),
            "{expr}"
        );
    }

    /// The court and the day the query writes rank beside the title, weighed by
    /// BM25 like it: they admit nothing, so a bearer of another court is still
    /// served.
    #[test]
    fn la_cour_et_le_jour_ecrits_classent_sans_admettre() {
        let req: SearchRequest =
            serde_json::from_value(serde_json::json!({ "query": "CA Versailles 21/01234" }))
                .unwrap();
        let written = Written {
            kind: Some("CA"),
            code: Some("ca_versailles".to_string()),
            day: Some("2023-03-15".to_string()),
        };
        let mut idx = 1usize;
        let mut params: Params = Vec::new();
        let reference = ReferenceKeys {
            numbers: vec![nombre("2101234", &[])],
            ..Default::default()
        };
        let expr = reference_query(&req, &reference, &written, &mut idx, &mut params);
        assert_eq!(
            expr,
            "paradedb.boolean(must => ARRAY[paradedb.boolean(should => ARRAY[paradedb.term('docket_keys', $2::text), \
             paradedb.term('docket_search', $2::text)])], \
             should => ARRAY[paradedb.match('search_title', $1), \
             paradedb.term('jurisdiction_type', $3::text), \
             paradedb.term('jurisdiction_code', $4::text), \
             paradedb.term('date_lecture', $5::text::date)])"
        );
    }

    /// A name admits the decisions it designates, never another case under the
    /// same number: Blanco's 1873 number is also a 1977 Conseil d'État decision's.
    #[test]
    fn a_name_admits_its_bearers_and_the_title_ranks_them() {
        let req: SearchRequest =
            serde_json::from_value(serde_json::json!({ "query": "arrêt Blanco" })).unwrap();
        let reference = ReferenceKeys {
            bearers: vec![17, 42],
            ..Default::default()
        };
        let mut idx = 1usize;
        let mut params: Params = Vec::new();
        assert_eq!(
            reference_query(&req, &reference, &Written::default(), &mut idx, &mut params),
            "paradedb.boolean(must => ARRAY[paradedb.term_set(terms => ARRAY[\
             paradedb.term('decision_id', 17::bigint), paradedb.term('decision_id', 42::bigint)])], \
             should => ARRAY[paradedb.match('search_title', $1)])"
        );
    }

    /// A key asks both key fields, `docket_search` holding the exact keys with
    /// the digits of a lettered number: `106/77` reaches `C-106/77` by its
    /// digits, and a bearer of the exact key, which matches both, ranks above.
    #[test]
    fn une_cle_interroge_la_cle_exacte_et_ses_chiffres() {
        let expr = reference_expr("106/77", &[nombre("10677", &[])], &[]);
        assert!(
            expr.contains("paradedb.term('docket_keys', $2::text)")
                && expr.contains("paradedb.term('docket_search', $2::text)"),
            "{expr}"
        );
    }

    /// The operator governs the title clause, the keys sitting beside it.
    #[test]
    fn un_operateur_booleen_passe_par_le_parse_du_titre() {
        let expr = reference_expr(
            "\"24-10.101\" OU \"24-10.102\"",
            &[nombre("2410101", &[]), nombre("2410102", &[])],
            &[],
        );
        assert!(
            expr.contains("paradedb.parse_with_field('search_title', $1)"),
            "{expr}"
        );
        assert!(
            expr.contains("$2::text") && expr.contains("$3::text"),
            "{expr}"
        );
    }

    #[test]
    fn truncate_relative_threshold() {
        let fused = vec![
            (1i64, 1.0, LegHit::synthetic_title_only(1)),
            (2i64, 0.4, LegHit::synthetic_title_only(2)),
            (3i64, 0.2, LegHit::synthetic_title_only(3)),
        ];
        let kept = truncate_fused(fused);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].0, 2);
    }
}
