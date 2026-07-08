//! Jambes de récupération (BM25 body, BM25 titre, ANN) + pooling décision,
//! fusion RRF et troncature relative.

use std::collections::{HashMap, HashSet};

use deadpool_postgres::Client;
use lj_store::error::StoreError;
use pgvector::Vector;
use tokio_postgres::types::ToSql;

use lj_dtos::SearchRequest;

use super::filters::{
    as_refs, build_array_filter_queries, build_facet_filter, compose_tantivy_query, ArrayFilters,
    FilterTable, Params,
};
use super::query::{is_boolean_query, phrase_combo_parse, translate_boolean};

const MAX_RESULTS: usize = 100;
const RELATIVE_SCORE_THRESHOLD: f64 = 0.3;
const RRF_RANK_CONSTANT: f64 = 60.0;
/// Sur-récupération chunk des jambes body/ANN (ADR 0080) : la LIMIT SQL est au
/// grain chunk, le classement de jambe au grain décision — ×1,1 suffit pour que
/// `leg_limit` chunks couvrent `leg_limit` décisions distinctes (1,08
/// chunk/décision en moyenne sur le corpus, p90 = 1).
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

#[tracing::instrument(skip(conn, req), fields(leg_limit))]
pub(crate) async fn bm25_leg(
    conn: &Client,
    req: &SearchRequest,
    leg_limit: i64,
) -> std::result::Result<HashMap<i64, f64>, StoreError> {
    let body_query = if is_boolean_query(&req.query) {
        translate_boolean(&req.query)
    } else {
        phrase_combo_parse(&req.query)
    };
    bm25_parse_leg(conn, req, &body_query, leg_limit).await
}

/// BM25 body au grain décision sur `decisions_bm25` (`full_text`). Plus de
/// sur-récupération ni de pooling : la `LIMIT` SQL est directement en décisions
/// (ADR 0084, supersede la jambe chunk + ×1,1 de l'ADR 0080). Renvoie
/// `{decision_id: score}`, symétrique de [`title_leg`].
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
    // `parse_with_field` : `decisions_bm25` porte DEUX champs texte (`full_text`
    // + `search_title`) — un parse non qualifié chercherait dans les deux. Cette
    // jambe ne score que le texte intégral.
    let array_filters = build_array_filter_queries(req, &mut idx, &mut params);
    let query_expr =
        compose_tantivy_query("paradedb.parse_with_field('full_text', $1)", array_filters);
    let facet_filter = build_facet_filter(
        req,
        &mut idx,
        &mut params,
        ArrayFilters::Tantivy,
        FilterTable::Decisions,
    );
    let sql = format!(
        "SELECT d.id, paradedb.score(d.id) AS score \
         FROM decisions d \
         WHERE d.id @@@ {query_expr}{ff} \
         ORDER BY score DESC LIMIT {limit}",
        ff = facet_filter,
        limit = leg_limit,
    );
    let rows = conn.query(sql.as_str(), &as_refs(&params)).await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1) as f64))
        .collect())
}

/// BM25 sur le champ `search_title` de `decisions_bm25`, au grain décision
/// (ADR 0084). Renvoie `{decision_id: score}`.
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
        "SELECT d.id, paradedb.score(d.id) AS score \
         FROM decisions d \
         WHERE d.id @@@ {query_expr}{ff} \
         ORDER BY score DESC LIMIT {limit}",
        ff = facet_filter,
        limit = leg_limit,
    );
    let rows = conn.query(sql.as_str(), &as_refs(&params)).await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1) as f64))
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
    // SET LOCAL dans une transaction (probes + prefilter), comme search.py.
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

// ── Pooling décision + fusion RRF + truncate ─────────────────────────────────

/// Max-pool chunk→décision (ADR 0080) puis troncature aux `leg_limit`
/// meilleures décisions (score desc, tie-break id) : la jambe a sur-récupéré
/// [`POOL_OVERFETCH`] chunks, le classement entrant en fusion RRF est au
/// grain décision et de profondeur `leg_limit`, comme la jambe titre. Le
/// `LegHit` gagnant est conservé (chunk du snippet).
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

/// Fusion RRF au grain décision (ADR 0080/0084). `bm25` et `title` sont des
/// scores par décision (jambes `decisions_bm25`) ; `ann` porte le `LegHit` du
/// chunk gagnant (seule jambe chunk-grain). Le chunk d'affichage du hit fusionné
/// vient de la jambe ANN — il ancre la fenêtre du snippet sémantique
/// ([`super::hydrate::assemble_page`]) — sinon un chunk synthétique (snippet depuis
/// `full_text`).
pub(crate) fn fuse_ranks(
    bm25: &HashMap<i64, f64>,
    ann: &HashMap<i64, LegHit>,
    title: &HashMap<i64, f64>,
    w_bm25: f64,
    w_ann: f64,
    w_title: f64,
) -> Vec<(i64, f64, LegHit)> {
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
                score += w_bm25 / (*r as f64 + RRF_RANK_CONSTANT);
            }
            if let Some(r) = ann_rank.get(&d) {
                score += w_ann / (*r as f64 + RRF_RANK_CONSTANT);
            }
            if let Some(r) = title_rank.get(&d) {
                score += w_title / (*r as f64 + RRF_RANK_CONSTANT);
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

pub(crate) fn truncate_fused(fused: Vec<(i64, f64, LegHit)>) -> Vec<(i64, f64, LegHit)> {
    if fused.is_empty() {
        return fused;
    }
    let threshold = fused[0].1 * RELATIVE_SCORE_THRESHOLD;
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
        // `_fuse_ranks` (Python) sur 3 jambes avec ex aequo, poids défaut (1,1,1),
        // rc=60. Ordre + scores figés depuis l'oracle : verrouille la formule RRF
        // ET le tie-break déterministe `(-score, decision_id)` (gap #2, HashMap Rust
        // non ordonné). bm25 {1:5,2:5,3:1} (1,2 ex aequo) ; ann {2:9,4:2} ;
        // title {1:3,4:3} (1,4 ex aequo).
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
        let fused = fuse_ranks(&bm25, &ann, &title, 1.0, 1.0, 1.0);
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
        // Doc présent UNIQUEMENT dans la jambe title → chunk synthétique
        // (`chunk_id=-1`, `chunk_index=0`), comme `_LegHit` côté Python.
        // bm25 {1:4} ; ann {1:2, 2:7} ; title {5:9}. Ordre/scores figés depuis
        // l'oracle ; doc 2 et doc 5 ex aequo → tie-break par id.
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
        let fused = fuse_ranks(&bm25, &ann, &title, 1.0, 1.0, 1.0);
        let order: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
        assert_eq!(order, vec![1, 2, 5], "ordre + tie-break (2 avant 5)");
        let scores: Vec<f64> = fused
            .iter()
            .map(|(_, s, _)| (s * 1e12).round() / 1e12)
            .collect();
        assert_eq!(scores, vec![0.032522474881, 0.016393442623, 0.016393442623]);
        // Le chunk de doc 5 (title-only) est synthétique.
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
        // Trois chunks de la décision 1 (scores ANN négatifs = -distance) :
        // un seul hit en sortie, celui du meilleur chunk.
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
        // 4 décisions, leg_limit 2 → top-2 par score poolé ; tie-break par id
        // (3 et 4 à égalité : 3 gagne).
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
        // Le scénario d'ADR 0080 : leg_limit = 2 mais la tête du classement
        // CHUNK est saturée par les chunks de la décision 1 ; la décision 2,
        // au-delà du rang 2 en chunks (sur-récupération), entre quand même
        // dans le top-2 décisions après pooling.
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
        let fused = fuse_ranks(&bm25, &ann, &title, 1.0, 1.0, 1.0);
        // doc 1 présent dans bm25 (rang 1) + title (rang 1) ⇒ score plus haut.
        assert_eq!(fused[0].0, 1);
    }

    #[test]
    fn truncate_relative_threshold() {
        let fused = vec![
            (1i64, 1.0, LegHit::synthetic_title_only(1)),
            (2i64, 0.4, LegHit::synthetic_title_only(2)),
            (3i64, 0.2, LegHit::synthetic_title_only(3)), // < 0.3 du top → coupé
        ];
        let kept = truncate_fused(fused);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].0, 2);
    }
}
