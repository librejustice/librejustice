//! Pondération RRF adaptative (signals) : embeddings représentatifs des têtes
//! BM25/ANN, puis calcul des poids.

use std::collections::HashMap;

use deadpool_postgres::Client;
use lj_store::error::StoreError;
use pgvector::Vector;

use crate::error::{ApiError, Result};
use crate::signals;
use crate::state::AppState;

use super::client;
use super::legs::LegHit;

async fn fetch_chunk_embeddings(
    conn: &Client,
    chunk_ids: &[i64],
) -> std::result::Result<HashMap<i64, Vec<f32>>, StoreError> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT id, dequantize_to_vector(embedding) FROM decision_chunks \
             WHERE id = ANY($1) AND embedding IS NOT NULL",
            &[&chunk_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, Vector>(1).to_vec()))
        .collect())
}

/// Embedding représentatif (chunk d'index minimal) de chaque décision, par
/// `decision_id`. La jambe BM25 est désormais au grain décision (ADR 0084) sans
/// chunk associé : on prend le 1er chunk comme proxy du document pour la
/// géométrie des signaux (93,5 % des décisions sont mono-chunk → proxy exact).
async fn fetch_decision_repr_embeddings(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, Vec<f32>>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT DISTINCT ON (decision_id) decision_id, dequantize_to_vector(embedding) \
             FROM decision_chunks WHERE decision_id = ANY($1) AND embedding IS NOT NULL \
             ORDER BY decision_id, chunk_index",
            &[&decision_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, Vector>(1).to_vec()))
        .collect())
}

pub(crate) async fn compute_adaptive_weights(
    state: &AppState,
    query: &str,
    bm25_scores: &HashMap<i64, f64>,
    ann_hits: &[LegHit],
) -> Result<signals::Weights> {
    if signals::has_article_reference(query) {
        return Ok(signals::compute_weights(query, &[], &[]));
    }
    // Top-K décisions BM25 (grain décision) — embedding représentatif par
    // décision ; top-K chunks ANN — embedding du chunk gagnant.
    let mut top_bm25: Vec<(i64, f64)> = bm25_scores.iter().map(|(d, s)| (*d, *s)).collect();
    top_bm25.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    top_bm25.truncate(signals::TOP_K);
    let bm25_decision_ids: Vec<i64> = top_bm25.iter().map(|(d, _)| *d).collect();

    let mut top_ann = ann_hits.to_vec();
    top_ann.sort_by(|a, b| {
        b.raw_score
            .partial_cmp(&a.raw_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_ann.truncate(signals::TOP_K);
    let ann_chunk_ids: Vec<i64> = top_ann.iter().map(|h| h.chunk_id).collect();

    // Deux lectures d'embeddings indépendantes (chunks ANN vs repr décision BM25) :
    // une connexion chacune en parallèle plutôt qu'en série sur une seule (une
    // connexion tokio-postgres ne pipeline pas deux requêtes). Les jambes ayant
    // relâché leurs conns en amont, le pic reste à 2 ici (≤ PEAK_CONNS_PER_SEARCH).
    let conn_ann = client(state).await?;
    let conn_bm25 = client(state).await?;
    let (ann_emb_map, bm25_emb_map) = tokio::try_join!(
        async {
            fetch_chunk_embeddings(&conn_ann, &ann_chunk_ids)
                .await
                .map_err(ApiError::Store)
        },
        async {
            fetch_decision_repr_embeddings(&conn_bm25, &bm25_decision_ids)
                .await
                .map_err(ApiError::Store)
        },
    )?;
    let ann_emb: Vec<Vec<f32>> = top_ann
        .iter()
        .filter_map(|h| ann_emb_map.get(&h.chunk_id).cloned())
        .collect();
    let bm25_emb: Vec<Vec<f32>> = bm25_decision_ids
        .iter()
        .filter_map(|d| bm25_emb_map.get(d).cloned())
        .collect();
    Ok(signals::compute_weights(query, &ann_emb, &bm25_emb))
}
