//! Reranker LLM en production — score-SC listwise sur résumés (`scs3_lws10x5_sum`).
//!
//! Port de `rerank.py`. Cf. ADR 0041 (mode IA) + ADR 0050 (bascule de méthode).
//! On garde le prompt listwise-scoring, le parser et la fusion par moyenne de
//! scores, on jette le scaffolding bench.
//!
//! Flux pour une query (méthode `scs3_small_lws10x5_sum`) :
//!
//! 1. On prend les top-50 docs du retrieval, body-source = résumé v4.
//! 2. 3 shuffles indépendants du top-50 (seeds 0/1/2) — réduction de variance.
//! 3. Pour chaque shuffle : 5 batches stratifiés (slice `[j::5]` → ~10 docs),
//!    1 appel listwise-scoring chacun → score 0-100 par doc = 5 appels/shuffle.
//! 4. Fusion par moyenne des scores sur les 3 shuffles → tri décroissant.
//!
//! Total = 15 appels Mistral parallèles.

use std::collections::HashMap;
use std::sync::Arc;

use lj_llm::mistral::{MistralClient, MistralError};
use regex::Regex;
use tracing::instrument;

use crate::error::ApiError;

/// Profondeur de rerank : nb de docs re-ordonnés (bench winner `scs3_lws10x5_sum`,
/// K=50). Le caller slice `ranked[:RERANK_K]`.
pub const RERANK_K: usize = 50;
/// Nb de shuffles fusionnés par moyenne de scores (`scs3`).
const N_SHUFFLES: u64 = 3;
/// Nb de batches listwise par shuffle (divisor de stratification `x5`).
const N_BATCHES: usize = 5;

/// Prompt système listwise-scoring (copié de `_LISTWISE_SCORE_SYS`, barème std).
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
///
/// - `decision_id` : id interne (utilisé pour le résultat).
/// - `title` : titre lisible court (juridiction + date + numéro).
/// - `summary` : résumé v4 (`decisions.summary`) — body-source unique, garanti
///   non vide par `_ensure_summaries` (ADR 0051).
#[derive(Debug, Clone)]
pub struct RerankItem {
    pub decision_id: i64,
    pub title: String,
    pub summary: String,
}

/// Texte injecté dans le prompt : résumé v4 (parité `_body_text`).
fn body_text(item: &RerankItem) -> &str {
    &item.summary
}

/// Parse `1: <score>\n2: <score>...` → liste de `k` scores (`None` si ≠ k).
///
/// Parité avec `_parse_listwise_scores` : regex `\s*(\d+)\s*[:.\-]\s*(\d+(?:\.\d+)?)`
/// appliquée à chaque ligne `strip()`-ée ; on ne garde que les index `1..=k` ;
/// échec si le nombre de scores distincts ≠ k.
fn parse_listwise_scores(content: &str, k: usize) -> Option<Vec<f64>> {
    // `re.match` ancre en début de chaîne (équivalent `^…`).
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

/// Un appel Mistral chat-completions via le client partagé ([`lj_llm::mistral`]),
/// température 0, `prompt_cache_key` stable (cache du prompt système côté Mistral).
///
/// Politique de retry **propre au rerank** : un seul retry sur 429 avec backoff
/// fixe 1 s, échec franc sinon (parité `_mistral_chat`). Les 15 appels parallèles
/// veulent fail-fast — pas le back-off exponentiel long des helpers d'ingest.
async fn mistral_chat(
    client: &MistralClient,
    system: &str,
    user: &str,
) -> Result<String, ApiError> {
    for attempt in 0..2 {
        match client.chat(system, user, None, Some("rr-lws-v1")).await {
            Ok(content) => return Ok(content),
            // 429 au 1ᵉʳ essai : un seul retry après 1 s.
            Err(MistralError::Status(429)) if attempt == 0 => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => return Err(ApiError::Internal(format!("mistral: {e}"))),
        }
    }
    // attempt 1 a aussi reçu 429 : échec franc.
    Err(ApiError::Internal("mistral: 429 after retry".into()))
}

/// Construit le prompt listwise utilisateur (parité `_build_listwise_prompt`).
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
///
/// Sur parse fail : score de repli = rang inversé dans le batch (parité
/// `_listwise_score_batch`).
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

/// Permute `items` (seed déterministe), stratifie en `N_BATCHES` slices
/// verticales `[j::N_BATCHES]`, note chaque batch en parallèle → scores fusionnés
/// (parité `_score_shuffle`).
#[instrument(skip(client, query, items), fields(n = items.len()))]
async fn score_shuffle(
    client: Arc<MistralClient>,
    query: String,
    items: Arc<Vec<RerankItem>>,
    seed: u64,
) -> Result<HashMap<i64, f64>, ApiError> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    shuffle_deterministic(&mut order, seed);
    let shuffled: Vec<RerankItem> = order.iter().map(|&i| items[i].clone()).collect();

    // Stratification verticale `shuffled[j::N_BATCHES]`.
    let mut batches: Vec<Vec<RerankItem>> = Vec::with_capacity(N_BATCHES);
    for j in 0..N_BATCHES {
        let b: Vec<RerankItem> = shuffled
            .iter()
            .skip(j)
            .step_by(N_BATCHES)
            .cloned()
            .collect();
        batches.push(b);
    }

    let mut handles = Vec::new();
    for b in batches.into_iter().filter(|b| !b.is_empty()) {
        let client = Arc::clone(&client);
        let query = query.clone();
        handles.push(async move { listwise_score_batch(&client, &query, &b).await });
    }
    let results = futures::future::join_all(handles).await;

    let mut merged: HashMap<i64, f64> = HashMap::new();
    for r in results {
        merged.extend(r?);
    }
    Ok(merged)
}

/// Reranke `items` (max 50) via `scs3_small_lws10x5_sum`.
///
/// Retourne les `decision_id` dans le nouvel ordre, taille ≤ [`RERANK_K`].
/// Sur shortlist < 3 docs : retourne tel quel (no-op silencieux).
///
/// `api_keys` : liste non vide. Round-robin sur les 15 appels parallèles
/// (3 shuffles × 5 batches) pour répartir la charge entre clés.
#[instrument(skip(items, api_keys), fields(n = items.len()))]
pub async fn rerank_shortlist(
    items: Vec<RerankItem>,
    query: &str,
    api_keys: Vec<String>,
    model: &str,
) -> Result<Vec<i64>, ApiError> {
    if items.len() < 3 {
        return Ok(items.into_iter().map(|it| it.decision_id).collect());
    }
    let items: Vec<RerankItem> = items.into_iter().take(RERANK_K).collect();
    // Client Mistral partagé : round-robin sur les clés, un span HTTP par appel
    // (TracingMiddleware) pour Tempo. Construit par requête (comme avant).
    let client = Arc::new(
        MistralClient::new(api_keys, model.to_string())
            .map_err(|e| ApiError::Internal(format!("rerank mistral client: {e}")))?,
    );
    let items = Arc::new(items);

    let mut shuffle_handles = Vec::new();
    for seed in 0..N_SHUFFLES {
        shuffle_handles.push(score_shuffle(
            Arc::clone(&client),
            query.to_string(),
            Arc::clone(&items),
            seed,
        ));
    }
    let shuffles = futures::future::join_all(shuffle_handles).await;

    // Fusion par moyenne des scores sur les shuffles (garde la magnitude).
    let mut sums: HashMap<i64, f64> = items.iter().map(|it| (it.decision_id, 0.0)).collect();
    let mut counts: HashMap<i64, u32> = items.iter().map(|it| (it.decision_id, 0u32)).collect();
    for scored in shuffles {
        for (did, s) in scored? {
            *sums.entry(did).or_insert(0.0) += s;
            *counts.entry(did).or_insert(0) += 1;
        }
    }
    let means: HashMap<i64, f64> = sums
        .keys()
        .map(|&did| {
            let c = counts[&did];
            (did, if c > 0 { sums[&did] / c as f64 } else { -1.0 })
        })
        .collect();

    // Tri stable : à moyenne égale, l'ordre retrieval (insertion = ordre `items`)
    // est préservé. On part de l'ordre d'entrée pour la stabilité.
    let mut ranked: Vec<i64> = items.iter().map(|it| it.decision_id).collect();
    ranked.sort_by(|a, b| {
        means[b]
            .partial_cmp(&means[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let query_head: String = query.chars().take(80).collect();
    tracing::info!(query = %query_head, n = items.len(), "rerank");
    Ok(ranked)
}

/// Mélange déterministe de `slice` à partir de `seed`.
///
/// NOTE de parité : Python utilise `random.Random(seed).shuffle`, un Fisher-Yates
/// backward piloté par le Mersenne Twister MT19937. On reproduit la *structure*
/// (Fisher-Yates backward, `j = randbelow(i+1)`) mais avec un PRNG SplitMix64 à
/// la place de MT19937 — l'ordre exact des permutations diverge donc de Python.
/// La fusion par moyenne sur 3 shuffles + 5 batches absorbe l'essentiel de cet
/// écart (l'objectif des shuffles est la réduction de variance, pas un ordre
/// canonique), mais ce point est listé comme `unresolved` pour la parité octet.
fn shuffle_deterministic(slice: &mut [usize], seed: u64) {
    let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    // Fisher-Yates backward (comme CPython `random.shuffle`).
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

    // Parité du parseur listwise ↔ oracle Python (apps/api rerank.py
    // `_parse_listwise_scores`). GT figée dans tests/fixtures/oracle/rerank_parse.json :
    // séparateurs `[:.\-]`, espaces parasites, lignes garbage ignorées, index hors
    // [1..=k] ignorés, doublon d'index (dernier gagne), count ≠ k → None. Aligne le
    // contrat exact de tolérance du parser, hors appel Mistral.
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
