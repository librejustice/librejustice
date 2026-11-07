//! Reranker LLM : score-SC listwise sur résumés (`scs3_lws10x5_sum`).
//!
//! Pour une query : top-50 du retrieval, 3 passes indépendantes (seeds 0/1/2),
//! chacune découpée en batches stratifiés de ~10 documents notés 0-100 en un appel
//! listwise, puis fusion par moyenne des notes d'un document.
//!
//! La note est portée par le couple (query, document) : l'appelant passe les notes
//! qu'il a déjà, et une passe ne bat que les documents sous leur compte. Les
//! batches incomplets sont remplis d'ancres — des documents déjà notés de la même
//! shortlist, pris par ancienneté.
//!
//! Les appels d'une passe, et les passes entre elles, sont joints : la latence est
//! celle du plus lent, pas de la somme.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;
use tokio::time::Instant;

use lj_llm::mistral::{is_retryable_status, MistralClient, MistralError};
use regex::Regex;
use tracing::instrument;

use crate::error::ApiError;

/// Profondeur de rerank : nombre de documents re-ordonnés. Le caller slice
/// `ranked[:RERANK_K]`.
pub const RERANK_K: usize = 50;
/// Notes visées par document avant fusion — une par passe (`scs3`).
///
/// Without a deadline, a second pass buys nothing over one and a third lifts the
/// ranking significantly.
pub const TARGET_SCORES: usize = 3;
/// Documents par appel listwise.
const BATCH_SIZE: usize = 10;
/// Hedged request: past this, a second call leaves without cancelling the first,
/// and the first answer back wins. Set only when `deadline` is.
///
/// Above the p90 of the call latency: lower, the single hedge of
/// [`HEDGES_PER_PASS`] would leave on most calls and none on the real straggler.
const HEDGE_AFTER: Duration = Duration::from_millis(1500);

/// Relances autorisées par passe.
const HEDGES_PER_PASS: usize = 1;

/// End-to-end latency deadline of the rerank on the live path, passed as
/// `deadline` by [`crate::search`]. Past it, the calls not back are dropped and
/// the fusion runs on what arrived ([`fuse_scores`]).
///
/// Covers the model's whole latency distribution: a shorter cut falls in its
/// normal regime and degrades the ranking.
pub const RERANK_BUDGET: Duration = Duration::from_secs(3);

/// Same deadline for an MCP caller, whose assistant turn already costs seconds:
/// a burst of searches slows the model, and the live deadline would then serve
/// the retrieval order rather than the ranking.
pub const RERANK_BUDGET_MCP: Duration = Duration::from_secs(5);

/// Model the prompt below was written and measured against. Moving one without
/// the other measures something else, so the pair is held here.
pub const RERANK_MODEL: &str = "mistral-small-2603";

/// Prompt système listwise-scoring (barème std).
const LISTWISE_SCORE_SYS: &str = concat!(
    "Tu es juriste. Pour chaque décision listée, donne un score de ",
    "pertinence 0-100 par rapport à la requête. ",
    "Barème :\n",
    "- 90 : pile-sujet, exactement ce que cherche le juriste\n",
    "- 70 : voisinage strict (même question, variante mineure)\n",
    "- 50 : adjacent, utile mais à côté\n",
    "- 30 : tangentiellement lié\n",
    "- 10 : hors sujet / faux ami (homonymie, autre branche du droit)\n\n",
    "Réponds STRICTEMENT au format :\n1: <score>\n2: <score>\n...\n",
    "Pas d'explication, pas de markdown."
);

/// Candidat passé au reranker.
#[derive(Debug, Clone)]
pub struct RerankItem {
    pub decision_id: i64,
    pub title: String,
    pub summary: String,
}

fn body_text(item: &RerankItem) -> &str {
    &item.summary
}

/// Parse `1: <score>\n2: <score>...` → liste de `k` scores (`None` si ≠ k).
fn parse_listwise_scores(content: &str, k: usize) -> Option<Vec<f64>> {
    let re = Regex::new(r"^\s*(\d+)\s*[:.\-]\s*(\d+(?:\.\d+)?)").expect("regex statique valide");
    let mut scores: HashMap<usize, f64> = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some(m) = re.captures(line) {
            let idx: usize = m[1].parse().ok()?;
            let val: f64 = m[2].parse().ok()?;
            if (1..=k).contains(&idx) {
                scores.insert(idx, val);
            }
        }
    }
    if scores.len() != k {
        return None;
    }
    Some((1..=k).map(|i| scores[&i]).collect())
}

/// Un appel Mistral via le client partagé ([`lj_llm::mistral`]), température 0.
///
/// Retry propre au rerank, sans sommeil : 401 (clé morte) et 429 / 5xx / erreur
/// transport sont retentés immédiatement, l'admission par quota faisant l'attente
/// et la rotation changeant de clé ; un 4xx non-retryable échoue franchement. La
/// borne est le pool — une tentative par clé vivante — et le plafond de temps le
/// `timeout_at` de [`score_pass`].
async fn mistral_chat(
    client: &MistralClient,
    system: &str,
    user: &str,
) -> Result<String, ApiError> {
    let mut last_err = String::new();
    let attempts = client.live_key_count();
    for attempt in 0..attempts {
        match client.chat(system, user, None).await {
            Ok(content) => return Ok(content),
            Err(MistralError::Status(401)) => last_err = "401".into(),
            Err(MistralError::Status(code)) if !is_retryable_status(code) => {
                return Err(ApiError::Internal(format!("mistral: status {code}")));
            }
            Err(e) => {
                last_err = e.to_string();
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    "rerank mistral retry"
                );
            }
        }
    }
    Err(ApiError::Internal(format!(
        "mistral: échec sur les {attempts} clés vivantes ({last_err})"
    )))
}

fn build_listwise_prompt(query: &str, items: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = vec![
        format!("Requête : {query}"),
        String::new(),
        "Décisions :".to_string(),
    ];
    for (i, (title, body)) in items.iter().enumerate() {
        lines.push(format!("\n[{}] {title}", i + 1));
        lines.push((*body).to_string());
    }
    lines.join("\n")
}

/// 1 appel listwise-scoring sur `batch` (≤ ~10 docs) → `{decision_id: score}`.
/// Sur parse fail : score de repli = rang inversé dans le batch.
#[instrument(skip(client, batch), fields(n = batch.len()))]
async fn listwise_score_batch(
    client: &MistralClient,
    query: &str,
    batch: &[RerankItem],
) -> Result<HashMap<i64, f64>, ApiError> {
    let pairs: Vec<(&str, &str)> = batch
        .iter()
        .map(|it| (it.title.as_str(), body_text(it)))
        .collect();
    let prompt = build_listwise_prompt(query, &pairs);
    let content = mistral_chat(client, LISTWISE_SCORE_SYS, &prompt).await?;
    match parse_listwise_scores(&content, batch.len()) {
        None => {
            tracing::warn!(
                n = batch.len(),
                "listwise scoring parse fail — fallback to input order"
            );
            Ok(batch
                .iter()
                .enumerate()
                .map(|(i, it)| (it.decision_id, (batch.len() - i) as f64))
                .collect())
        }
        Some(scores) => Ok(batch
            .iter()
            .zip(scores)
            .map(|(it, s)| (it.decision_id, s))
            .collect()),
    }
}

/// Scores d'une passe, avec la couverture qui les a produits.
struct PassScores {
    scores: HashMap<i64, f64>,
    batches_ok: usize,
    batches_total: usize,
    /// Relances parties sur cette passe (au plus [`HEDGES_PER_PASS`]).
    hedges_fired: usize,
    /// Relances qui ont devancé l'appel qu'elles doublaient.
    hedges_won: usize,
}

/// Notes d'un document pour une query donnée, de la plus ancienne à la plus
/// récente.
#[derive(Clone, Debug)]
pub struct DocScores {
    /// Au plus [`TARGET_SCORES`] notes — au-delà, la plus ancienne sort.
    pub scores: Vec<f64>,
    /// Tour où la dernière note a été posée. Ordonne le choix des ancres.
    pub stamped: Instant,
}

impl DocScores {
    /// Verse `fresh` dans les notes existantes et ne garde que les
    /// [`TARGET_SCORES`] dernières. Point unique de la règle de fenêtre.
    pub fn merged(existing: Option<DocScores>, fresh: &[f64], at: Instant) -> DocScores {
        let mut scores = existing.map(|d| d.scores).unwrap_or_default();
        scores.extend_from_slice(fresh);
        if scores.len() > TARGET_SCORES {
            scores.drain(..scores.len() - TARGET_SCORES);
        }
        DocScores {
            scores,
            stamped: at,
        }
    }
}

/// Documents à noter à la passe `pass` : ceux dont le compte de notes en cache
/// est encore sous [`TARGET_SCORES`] à ce rang.
fn pass_pool<'a>(
    items: &'a [RerankItem],
    cached: &HashMap<i64, DocScores>,
    pass: usize,
) -> Vec<&'a RerankItem> {
    items
        .iter()
        .filter(|it| {
            let have = cached.get(&it.decision_id).map_or(0, |d| d.scores.len());
            have + pass < TARGET_SCORES
        })
        .collect()
}

/// Permute `pool` (seed déterministe), stratifie en slices verticales de
/// ~[`BATCH_SIZE`] documents, et complète chaque batch court avec des `anchors`,
/// données du plus ancien au plus récent.
fn build_batches(pool: &[&RerankItem], anchors: &[&RerankItem], seed: u64) -> Vec<Vec<RerankItem>> {
    if pool.is_empty() {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..pool.len()).collect();
    shuffle_deterministic(&mut order, seed);
    let shuffled: Vec<&RerankItem> = order.iter().map(|&i| pool[i]).collect();

    let n_batches = shuffled.len().div_ceil(BATCH_SIZE);
    let mut batches = Vec::with_capacity(n_batches);
    let mut cursor = 0usize;
    for j in 0..n_batches {
        let mut members: Vec<&RerankItem> = shuffled
            .iter()
            .skip(j)
            .step_by(n_batches)
            .copied()
            .collect();
        let missing = BATCH_SIZE.saturating_sub(members.len()).min(anchors.len());
        for _ in 0..missing {
            members.push(anchors[cursor % anchors.len()]);
            cursor += 1;
        }
        if missing > 0 {
            let mut positions: Vec<usize> = (0..members.len()).collect();
            shuffle_deterministic(&mut positions, seed.wrapping_add(j as u64 + 1));
            members = positions.into_iter().map(|i| members[i]).collect();
        }
        batches.push(members.into_iter().cloned().collect());
    }
    batches
}

/// Pourquoi un batch n'a pas rendu ses notes. `Echeance` est le régime prévu
/// sous charge, `Amont` un échec du reranker : deux lignes de log distinctes.
enum BatchPerdu {
    Echeance,
    Amont(ApiError),
}

/// Note en parallèle les batches d'une passe → une note par document envoyé,
/// ancres comprises.
///
/// Ne rend pas d'erreur : un batch perdu (échec Mistral ou `deadline` atteint) est
/// retiré du décompte et les autres sont conservés.
#[instrument(skip(client, query, pool, anchors, sink), fields(n = pool.len()))]
async fn score_pass(
    client: Arc<MistralClient>,
    query: &str,
    pool: &[&RerankItem],
    anchors: &[&RerankItem],
    seed: u64,
    deadline: Option<Instant>,
    sink: &ScoreSink,
) -> PassScores {
    let mut handles = Vec::new();
    let hedges = Arc::new(std::sync::atomic::AtomicUsize::new(HEDGES_PER_PASS));
    let hedges_won = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for b in build_batches(pool, anchors, seed) {
        let client = Arc::clone(&client);
        let query = query.to_string();
        let sink = Arc::clone(sink);
        let hedges = Arc::clone(&hedges);
        let hedges_won = Arc::clone(&hedges_won);
        handles.push(async move {
            let scored = async {
                let primary = listwise_score_batch(&client, &query, &b);
                let Some(hedge_after) = deadline.map(|_| HEDGE_AFTER) else {
                    return primary.await;
                };
                tokio::pin!(primary);
                tokio::select! {
                    r = &mut primary => r,
                    _ = tokio::time::sleep(hedge_after) => {
                        use std::sync::atomic::Ordering::Relaxed;
                        if hedges.fetch_update(Relaxed, Relaxed, |n| n.checked_sub(1)).is_err() {
                            return (&mut primary).await;
                        }
                        tracing::debug!(seed, n = b.len(), "rerank batch retried");
                        let hedge = listwise_score_batch(&client, &query, &b);
                        tokio::pin!(hedge);
                        tokio::select! {
                            r = &mut primary => r,
                            r = &mut hedge => {
                                hedges_won.fetch_add(1, Relaxed);
                                r
                            }
                        }
                    }
                }
            };
            match deadline {
                Some(t) => match tokio::time::timeout_at(t, scored).await {
                    Ok(r) => r.map_err(BatchPerdu::Amont),
                    Err(_) => Err(BatchPerdu::Echeance),
                },
                None => scored.await.map_err(BatchPerdu::Amont),
            }
            .inspect(|m: &HashMap<i64, f64>| sink(m.clone()))
        });
    }
    let results = futures::future::join_all(handles).await;

    let batches_total = results.len();
    let mut batches_ok = 0usize;
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for r in results {
        match r {
            Ok(m) => {
                batches_ok += 1;
                scores.extend(m);
            }
            Err(BatchPerdu::Echeance) => tracing::warn!(
                seed,
                "rerank batch cut on the latency deadline, merging on the remaining scores"
            ),
            Err(BatchPerdu::Amont(err)) => tracing::warn!(
                error = %err,
                seed,
                "rerank batch lost on a reranker error, merging on the remaining scores"
            ),
        }
    }
    PassScores {
        scores,
        batches_ok,
        batches_total,
        hedges_fired: HEDGES_PER_PASS - hedges.load(std::sync::atomic::Ordering::Relaxed),
        hedges_won: hedges_won.load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// Fusionne les notes par document en un ordre de pertinence décroissante.
///
/// Moyenne par document sur les seules notes qu'il porte : le barème étant absolu
/// (0-100), un document noté 2 fois sur 3 reste comparable à un document noté 3
/// fois, et des notes de tours différents se mélangent.
///
/// Un document sans aucune note garde son rang de récupération ; les notés se
/// reclassent autour de lui. `Err` seulement si aucun document n'a de note.
///
/// Tri stable depuis l'ordre d'entrée : à moyenne égale, l'ordre RRF tranche.
pub fn fuse_scores(ids: &[i64], scores: &HashMap<i64, Vec<f64>>) -> Result<Vec<i64>, ApiError> {
    let noted = |d: &i64| scores.get(d).is_some_and(|v| !v.is_empty());

    let means: HashMap<i64, f64> = ids
        .iter()
        .filter(|d| noted(d))
        .map(|d| {
            let v = &scores[d];
            (*d, v.iter().sum::<f64>() / v.len() as f64)
        })
        .collect();
    if means.is_empty() {
        return Err(ApiError::Internal(format!(
            "rerank: aucun des {} documents n'a de note",
            ids.len()
        )));
    }

    let mut ranked: Vec<i64> = ids.iter().copied().filter(noted).collect();
    ranked.sort_by(|a, b| {
        means[b]
            .partial_cmp(&means[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut next = ranked.into_iter();
    Ok(ids
        .iter()
        .map(|d| {
            if noted(d) {
                next.next().expect("un noté par place notée")
            } else {
                *d
            }
        })
        .collect())
}

/// Ordre fusionné sur les seules notes déjà en cache, si **chaque** document de
/// `items` a son compte de [`TARGET_SCORES`]. `None` dès qu'un appel reste dû.
pub fn cached_order(items: &[RerankItem], cached: &HashMap<i64, DocScores>) -> Option<Vec<i64>> {
    if items.len() < 3 || !pass_pool(items, cached, 0).is_empty() {
        return None;
    }
    let scores = cached
        .iter()
        .map(|(did, d)| (*did, d.scores.clone()))
        .collect();
    let ids: Vec<i64> = items.iter().map(|it| it.decision_id).collect();
    fuse_scores(&ids, &scores).ok()
}

/// Puits de notes : reçoit les notes d'un batch dès son retour, sans attendre ses
/// sœurs ni la fin du rerank. Appelé depuis les futures de batch, donc
/// `Send + Sync`, et non-bloquant.
pub type ScoreSink = Arc<dyn Fn(HashMap<i64, f64>) + Send + Sync>;

/// Puits qui jette tout.
pub fn discard_scores() -> ScoreSink {
    Arc::new(|_| {})
}

/// Reranke `items` (max 50) via `scs3_small_lws10x5_sum`.
///
/// Retourne les `decision_id` dans le nouvel ordre, taille ≤ [`RERANK_K`].
/// Sur shortlist < 3 docs : retourne tel quel.
///
/// `cached` : notes déjà connues par `decision_id`, pour cette query. Seuls les
/// documents sous [`TARGET_SCORES`] sont battus.
///
/// `sink` : reçoit les notes de chaque batch dès son retour (cf. [`ScoreSink`]).
/// C'est lui, pas la valeur de retour, qui alimente le cache de l'appelant.
///
/// `deadline` : échéance de bout en bout. `Some` sur le chemin live
/// ([`RERANK_BUDGET`]) — au-delà, les appels en vol sont abandonnés et la fusion
/// se fait sur les notes arrivées. `None` au banc.
#[instrument(skip(items, client, cached, sink), fields(
    n = items.len(),
    librejustice.rerank.outcome = tracing::field::Empty,
    librejustice.rerank.batches_ok = tracing::field::Empty,
    librejustice.rerank.batches_total = tracing::field::Empty,
    librejustice.rerank.docs_cached = tracing::field::Empty,
    librejustice.rerank.docs_unscored = tracing::field::Empty,
    librejustice.rerank.hedges_fired = tracing::field::Empty,
    librejustice.rerank.hedges_won = tracing::field::Empty,
))]
pub async fn rerank_shortlist(
    items: Vec<RerankItem>,
    query: &str,
    client: Arc<MistralClient>,
    deadline: Option<Instant>,
    cached: &HashMap<i64, DocScores>,
    sink: ScoreSink,
) -> Result<Vec<i64>, ApiError> {
    if items.len() < 3 {
        return Ok(items.into_iter().map(|it| it.decision_id).collect());
    }
    let items: Vec<RerankItem> = items.into_iter().take(RERANK_K).collect();

    let mut pass_handles = Vec::new();
    for pass in 0..TARGET_SCORES {
        let pool = pass_pool(&items, cached, pass);
        if pool.is_empty() {
            continue;
        }
        let anchors: Vec<&RerankItem> = {
            let needy: std::collections::HashSet<i64> =
                pool.iter().map(|it| it.decision_id).collect();
            let mut rest: Vec<&RerankItem> = items
                .iter()
                .filter(|it| !needy.contains(&it.decision_id))
                .collect();
            rest.sort_by_key(|it| cached.get(&it.decision_id).map(|d| d.stamped));
            rest
        };
        let client = Arc::clone(&client);
        let sink = Arc::clone(&sink);
        pass_handles.push(async move {
            score_pass(client, query, &pool, &anchors, pass as u64, deadline, &sink).await
        });
    }
    let passes = futures::future::join_all(pass_handles).await;

    let batches_ok: usize = passes.iter().map(|s| s.batches_ok).sum();
    let batches_total: usize = passes.iter().map(|s| s.batches_total).sum();
    let hedges_fired: usize = passes.iter().map(|s| s.hedges_fired).sum();
    let hedges_won: usize = passes.iter().map(|s| s.hedges_won).sum();
    let mut fresh: HashMap<i64, Vec<f64>> = HashMap::new();
    for p in passes {
        for (did, s) in p.scores {
            fresh.entry(did).or_default().push(s);
        }
    }

    let mut all: HashMap<i64, Vec<f64>> = cached
        .iter()
        .map(|(did, d)| (*did, d.scores.clone()))
        .collect();
    for (did, s) in &fresh {
        all.entry(*did).or_default().extend(s);
    }
    let docs_cached = items
        .iter()
        .filter(|it| cached.contains_key(&it.decision_id))
        .count();
    let ids: Vec<i64> = items.iter().map(|it| it.decision_id).collect();
    let fused = fuse_scores(&ids, &all);

    let docs_unscored = ids
        .iter()
        .filter(|d| all.get(d).is_none_or(Vec::is_empty))
        .count();
    let outcome = match (&fused, batches_ok == batches_total) {
        (Err(_), _) => "abandon",
        (Ok(_), true) => "complet",
        (Ok(_), false) => "degrade",
    };
    let span = tracing::Span::current();
    span.record("librejustice.rerank.outcome", outcome);
    lj_telemetry::meter()
        .u64_counter("search.rerank.outcome")
        .with_description("Reranks de recherche, par qualité de sortie.")
        .build()
        .add(1, &[KeyValue::new("outcome", outcome)]);
    span.record("librejustice.rerank.batches_ok", batches_ok);
    span.record("librejustice.rerank.batches_total", batches_total);
    span.record("librejustice.rerank.docs_cached", docs_cached);
    span.record("librejustice.rerank.docs_unscored", docs_unscored);
    span.record("librejustice.rerank.hedges_fired", hedges_fired);
    span.record("librejustice.rerank.hedges_won", hedges_won);

    let order = fused?;
    let query_head: String = query.chars().take(80).collect();
    tracing::info!(query = %query_head, n = items.len(), docs_cached, "rerank");
    Ok(order)
}

/// Mélange déterministe de `slice` à partir de `seed` : Fisher-Yates backward
/// piloté par un PRNG SplitMix64.
fn shuffle_deterministic(slice: &mut [usize], seed: u64) {
    let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let n = slice.len();
    if n <= 1 {
        return;
    }
    for i in (1..n).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        slice.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(ids: &[i64]) -> Vec<RerankItem> {
        ids.iter()
            .map(|&decision_id| RerankItem {
                decision_id,
                title: String::new(),
                summary: String::new(),
            })
            .collect()
    }

    fn notes(pairs: &[(i64, &[f64])]) -> HashMap<i64, Vec<f64>> {
        pairs.iter().map(|(id, v)| (*id, v.to_vec())).collect()
    }

    #[test]
    fn fuse_orders_by_mean_across_passes() {
        let s = notes(&[
            (1, &[30.0, 30.0, 30.0]),
            (2, &[90.0, 90.0, 90.0]),
            (3, &[50.0, 50.0, 50.0]),
        ]);
        assert_eq!(fuse_scores(&[1, 2, 3], &s).unwrap(), vec![2, 3, 1]);
    }

    #[test]
    fn fuse_averages_partial_coverage_instead_of_penalising_it() {
        let s = notes(&[(1, &[90.0]), (2, &[50.0, 50.0, 50.0])]);
        assert_eq!(fuse_scores(&[1, 2], &s).unwrap(), vec![1, 2]);
    }

    #[test]
    fn fuse_freezes_an_unscored_document_at_its_retrieval_rank() {
        let s = notes(&[(1, &[50.0]), (2, &[]), (4, &[90.0])]);
        assert_eq!(fuse_scores(&[1, 2, 4], &s).unwrap(), vec![4, 2, 1]);
    }

    #[test]
    fn fuse_keeps_ranking_the_scored_when_most_documents_are_unscored() {
        let s = notes(&[(1, &[10.0]), (2, &[]), (3, &[]), (4, &[90.0])]);
        assert_eq!(fuse_scores(&[1, 2, 3, 4], &s).unwrap(), vec![4, 2, 3, 1]);
    }

    #[test]
    fn fuse_with_every_batch_lost_is_an_error_not_an_empty_ranking() {
        assert!(fuse_scores(&[1, 2], &HashMap::new()).is_err());
        assert!(fuse_scores(&[1, 2], &notes(&[(1, &[]), (2, &[])])).is_err());
    }

    #[test]
    fn fuse_keeps_retrieval_order_on_ties() {
        let s = notes(&[(7, &[50.0]), (3, &[50.0]), (5, &[50.0])]);
        assert_eq!(fuse_scores(&[7, 3, 5], &s).unwrap(), vec![7, 3, 5]);
    }

    /// Notes en cache, toutes estampillées au même tour.
    fn cache(pairs: &[(i64, &[f64])]) -> HashMap<i64, DocScores> {
        let now = Instant::now();
        pairs
            .iter()
            .map(|(id, v)| {
                (
                    *id,
                    DocScores {
                        scores: v.to_vec(),
                        stamped: now,
                    },
                )
            })
            .collect()
    }

    fn ids(batch: &[RerankItem]) -> std::collections::HashSet<i64> {
        batch.iter().map(|it| it.decision_id).collect()
    }

    fn pool_ids(items: &[RerankItem], cached: &HashMap<i64, DocScores>, pass: usize) -> Vec<i64> {
        pass_pool(items, cached, pass)
            .iter()
            .map(|it| it.decision_id)
            .collect()
    }

    #[test]
    fn cold_shortlist_is_scored_by_every_pass() {
        let it = items(&(1..=50).collect::<Vec<_>>());
        let empty = HashMap::new();
        for pass in 0..TARGET_SCORES {
            assert_eq!(pool_ids(&it, &empty, pass).len(), 50);
        }
        let batches = build_batches(&pass_pool(&it, &empty, 0), &[], 0);
        assert_eq!(batches.len(), 5);
        assert!(batches.iter().all(|b| b.len() == BATCH_SIZE));
    }

    #[test]
    fn a_document_is_in_exactly_as_many_passes_as_notes_it_lacks() {
        let it = items(&[1, 2, 3, 4]);
        let cached = cache(&[(2, &[50.0]), (3, &[50.0, 60.0]), (4, &[50.0, 60.0, 70.0])]);
        for pass in 0..TARGET_SCORES {
            let expected: Vec<i64> = (1..=(TARGET_SCORES - pass) as i64).collect();
            assert_eq!(pool_ids(&it, &cached, pass), expected);
        }
    }

    #[test]
    fn short_batches_are_padded_to_full_size_with_anchors() {
        let all = items(&(1..=23).collect::<Vec<_>>());
        let pool: Vec<&RerankItem> = all[..3].iter().collect();
        let anchors: Vec<&RerankItem> = all[3..].iter().collect();
        let batches = build_batches(&pool, &anchors, 0);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), BATCH_SIZE);
        assert!(ids(&batches[0]).is_superset(&[1, 2, 3].into_iter().collect()));
    }

    #[test]
    fn anchors_are_taken_oldest_first_and_never_twice_in_a_pass() {
        // L'appelant trie les ancres par ancienneté ; `build_batches` les
        // consomme dans l'ordre reçu, sans revenir en arrière tant qu'il en
        // reste. Deux batches courts ne voient donc pas les mêmes voisins.
        let all = items(&(1..=40).collect::<Vec<_>>());
        let pool: Vec<&RerankItem> = all[..12].iter().collect();
        let anchors: Vec<&RerankItem> = all[12..].iter().collect();
        let batches = build_batches(&pool, &anchors, 0);
        assert_eq!(batches.len(), 2);
        let needy: std::collections::HashSet<i64> = (1..=12).collect();
        let padding: Vec<std::collections::HashSet<i64>> = batches
            .iter()
            .map(|b| ids(b).difference(&needy).copied().collect())
            .collect();
        assert!(padding[0].is_disjoint(&padding[1]));
        // Les 8 ancres servies sont les 8 premières fournies (les plus vieilles).
        let served: std::collections::HashSet<i64> =
            padding[0].union(&padding[1]).copied().collect();
        assert_eq!(served, (13..=20).collect());
    }

    #[test]
    fn a_fully_cached_shortlist_needs_no_call() {
        let it = items(&[1, 2, 3]);
        let full = |v: f64| vec![v; TARGET_SCORES];
        let cached = cache(&[(1, &full(10.0)), (2, &full(90.0)), (3, &full(50.0))]);
        assert_eq!(cached_order(&it, &cached), Some(vec![2, 3, 1]));
        // Une seule note manquante suffit à redemander un tour.
        let short = full(10.0)[1..].to_vec();
        let partial = cache(&[(1, &short), (2, &full(90.0)), (3, &full(50.0))]);
        assert_eq!(cached_order(&it, &partial), None);
    }

    #[test]
    fn merged_keeps_the_most_recent_notes() {
        let t0 = Instant::now();
        let first: Vec<f64> = (1..=TARGET_SCORES).map(|i| 10.0 * i as f64).collect();
        let d = DocScores::merged(None, &first, t0);
        assert_eq!(d.scores, first);
        // The extra note evicts the oldest, so a document often taken as an
        // anchor does not weigh more than another.
        let d = DocScores::merged(Some(d), &[99.0], t0);
        assert_eq!(d.scores, [&first[1..], &[99.0][..]].concat());
    }

    #[test]
    fn parse_ok_colon() {
        let content = "1: 90\n2: 30\n3: 70";
        let scores = parse_listwise_scores(content, 3).unwrap();
        assert_eq!(scores, vec![90.0, 30.0, 70.0]);
    }

    #[test]
    fn parse_ok_dot_and_dash_separators() {
        // Le séparateur accepté est `[:.\-]`.
        let content = "1. 50\n2- 10\n3: 80.5";
        let scores = parse_listwise_scores(content, 3).unwrap();
        assert_eq!(scores, vec![50.0, 10.0, 80.5]);
    }

    #[test]
    fn parse_fail_wrong_count() {
        // Manque l'index 3 → None.
        assert!(parse_listwise_scores("1: 90\n2: 30", 3).is_none());
    }

    #[test]
    fn parse_ignores_out_of_range_index() {
        // Index 5 hors [1..=3] ignoré ; il manque donc l'index 3 → None.
        assert!(parse_listwise_scores("1: 90\n2: 30\n5: 70", 3).is_none());
    }

    #[test]
    fn parse_ignores_garbage_lines() {
        let content = "blah\n1: 90\n  markdown ?\n2: 30\n3 : 70";
        let scores = parse_listwise_scores(content, 3).unwrap();
        assert_eq!(scores, vec![90.0, 30.0, 70.0]);
    }

    #[test]
    fn build_prompt_numbers_from_one() {
        let prompt = build_listwise_prompt(
            "expulsion",
            &[("CE 2024", "corps A"), ("CA 2023", "corps B")],
        );
        assert!(prompt.starts_with("Requête : expulsion\n\nDécisions :"));
        assert!(prompt.contains("[1] CE 2024"));
        assert!(prompt.contains("[2] CA 2023"));
        assert!(prompt.contains("corps A"));
    }

    #[test]
    fn shuffle_is_deterministic_and_a_permutation() {
        let mut a: Vec<usize> = (0..50).collect();
        let mut b: Vec<usize> = (0..50).collect();
        shuffle_deterministic(&mut a, 0);
        shuffle_deterministic(&mut b, 0);
        assert_eq!(a, b, "même seed → même ordre");
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>(), "permutation valide");
        // seeds différents → ordres différents (avec très haute probabilité).
        let mut c: Vec<usize> = (0..50).collect();
        shuffle_deterministic(&mut c, 1);
        assert_ne!(a, c);
    }

    // The listwise parser's tolerance contract, held in tests/fixtures/oracle/rerank_parse.json.
    #[derive(serde::Deserialize)]
    struct RerankCase {
        content: String,
        k: usize,
        scores: Option<Vec<f64>>,
    }
    #[derive(serde::Deserialize)]
    struct RerankFixture {
        cases: Vec<RerankCase>,
    }

    #[test]
    fn listwise_parse_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/rerank_parse.json");
        let fix: RerankFixture = serde_json::from_str(raw).expect("fixture rerank_parse");
        for c in &fix.cases {
            assert_eq!(
                parse_listwise_scores(&c.content, c.k),
                c.scores,
                "parse {:?} k={}",
                c.content,
                c.k
            );
        }
    }
}
