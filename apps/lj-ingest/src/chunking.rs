//! Chunker mathématique avec DP sur DAG (port de `chunking/chunker.py`).
//!
//! Deux entrées : [`chunk_char`] (mode caractères, chemin nominal rapide) et
//! [`chunk_bpe`] (mode BPE exact via tokenizer HF, repli déclenché sur overflow
//! de l'embedder). La DP layer-par-layer et l'assemblage sont communs
//! ([`solve_and_assemble`]) ; seule la préparation des candidats diffère.
//! Cf. `docs/working-notes/chunking_dag.md`.
//!
//! Parité octet : tout est indexé en **unités char** (positions de string Python),
//! jamais en octets. Les offsets BPE sont récupérés via `encode_char_offsets`
//! (le binding Python `Tokenizer.encode` renvoie des offsets char).
//!
//! Vit dans `lj-ingest` (seul consommateur) : le mode BPE charge le tokenizer
//! Qwen3 (~11 Mo, `include_bytes!`), une I/O qui n'a pas sa place dans `lj-core`
//! pur. Le calibrage chars↔tokens partagé reste, lui, dans `lj_core::tokens`
//! (ADR 0081).

use anyhow::{anyhow, Result};
use lj_core::tokens::{CHARS_PER_TOKEN_MEDIAN, CHARS_PER_TOKEN_SAFE};
use std::collections::BTreeMap;

pub const DEFAULT_CHUNK_TOKENS: usize = 8192;
pub const DEFAULT_OVERLAP_MIN: usize = 125;
pub const DEFAULT_OVERLAP_MAX: usize = 250;

/// Version de génération chunk + embed (ADR 0085), miroir d'`EXTRACT_VERSION`
/// (ADR 0083). Persistée dans `decisions.embed_version` quand des embeddings
/// sont écrits ; un futur `reembed` ne retouche que les décisions dont la
/// version diffère. À incrémenter à tout changement du chunker ou du modèle.
pub const EMBED_VERSION: i16 = 1;

/// Séparateur visuel inséré entre `visa_trim` et `ctx` dans `E_i` (i ≥ 1).
pub const TRUNCATION_MARKER: &str = "\n\n[…]\n\n";

/// Pénalités séparateur ρ par défaut (plus bas = meilleur).
pub const DEFAULT_SEP_PENALTIES: &[(&str, f64)] = &[
    ("\n\n", 0.0),
    ("\n", 1.0),
    (". ", 3.0),
    (".", 5.0),
    (";", 7.0),
    (",", 9.0),
];
pub const DEFAULT_LAMBDA_SIZE: f64 = 1.0;
pub const DEFAULT_LAMBDA_CTX: f64 = 1.0;

/// Tokenizer HF chargé (mode BPE exact). Le chargement effectif (I/O, artefact
/// ~11 Mo) vit dans `pipeline.rs` ; ici on ne reçoit qu'un `&Tokenizer`.
pub type Tokenizer = tokenizers::Tokenizer;

/// Paramètres de la fonction de coût :
/// `cost_i = λ_size·Φ(E_i − τ_n) + ρ(q_{i+1}) + λ_ctx·ρ(c_i)`.
#[derive(Debug, Clone)]
pub struct ChunkerParams {
    pub lambda_size: f64,
    pub lambda_ctx: f64,
    pub sep_penalties: Vec<(String, f64)>,
}

impl Default for ChunkerParams {
    fn default() -> Self {
        Self {
            lambda_size: DEFAULT_LAMBDA_SIZE,
            lambda_ctx: DEFAULT_LAMBDA_CTX,
            sep_penalties: DEFAULT_SEP_PENALTIES
                .iter()
                .map(|(s, p)| (s.to_string(), *p))
                .collect(),
        }
    }
}

/// Un chunk prêt à indexer/embeder. `embedding_text` est assemblé à la volée,
/// jamais stocké en DB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub chunk_index: usize,
    pub char_start: usize,
    pub char_end: usize,
    pub ctx_start: usize,
    /// `own_i = x[q_i:q_{i+1}]`, slice strict, BM25-indexable.
    pub body: String,
    /// Contexte gauche `x[ctx_start:char_start]` pour i ≥ 1.
    pub ctx: String,
    pub metadata_header: String,
    pub visa_trim: String,
}

impl Chunk {
    /// Texte envoyé à l'embedder : metadata + (visa + marker + ctx pour i ≥ 1) + body.
    pub fn embedding_text(&self) -> String {
        if self.chunk_index == 0 {
            if !self.metadata_header.is_empty() {
                return format!("{}\n\n{}", self.metadata_header, self.body);
            }
            return self.body.clone();
        }
        let mut parts = String::new();
        if !self.metadata_header.is_empty() {
            parts.push_str(&self.metadata_header);
            parts.push_str("\n\n");
        }
        if !self.visa_trim.is_empty() {
            parts.push_str(&self.visa_trim);
        }
        parts.push_str(TRUNCATION_MARKER);
        parts.push_str(&self.ctx);
        parts.push_str(&self.body);
        parts
    }
}

// ---------------------------------------------------------------------------
// Construction de S (positions candidates en unité)
// ---------------------------------------------------------------------------

/// Char-mode : positions juste après chaque séparateur, plus 0 et len(text).
///
/// `chars` est `text` décomposé en chars (positions = indices Python).
fn build_candidates_chars(
    chars: &[char],
    sep_penalties: &[(String, f64)],
) -> (Vec<usize>, Vec<f64>) {
    let n = chars.len();
    let mut cand: BTreeMap<usize, f64> = BTreeMap::new();
    cand.insert(0, 0.0);
    cand.insert(n, 0.0);

    for (sep, rho) in sep_penalties {
        let sep_chars: Vec<char> = sep.chars().collect();
        let sep_len = sep_chars.len();
        if sep_len == 0 {
            continue;
        }
        // Reproduit `text.find(sep, start)` avec start avançant de 1 à chaque hit
        // (boucle Python : start = i + 1).
        let mut start = 0usize;
        loop {
            let i = find_char(chars, &sep_chars, start);
            let Some(i) = i else { break };
            let after = i + sep_len;
            if after > 0 && after < n {
                cand.entry(after)
                    .and_modify(|cur| {
                        if *rho < *cur {
                            *cur = *rho;
                        }
                    })
                    .or_insert(*rho);
            }
            start = i + 1;
        }
    }

    let sorted_pos: Vec<usize> = cand.keys().copied().collect();
    let rhos: Vec<f64> = sorted_pos.iter().map(|p| cand[p]).collect();
    (sorted_pos, rhos)
}

/// Recherche du sous-slice `needle` dans `haystack` à partir de `start` (indices char).
/// Renvoie l'indice char du premier match, comme `str.find`.
fn find_char(haystack: &[char], needle: &[char], start: usize) -> Option<usize> {
    let n = haystack.len();
    let m = needle.len();
    if m == 0 {
        return Some(start.min(n));
    }
    if start + m > n {
        return None;
    }
    for i in start..=(n - m) {
        if haystack[i..i + m] == *needle {
            return Some(i);
        }
    }
    None
}

/// BPE-mode : pour chaque token, vérifie si son span char contient un séparateur.
///
/// La frontière `t+1` est marquée avec la pénalité du séparateur de plus haute
/// priorité trouvé dans le span char `chars[char_starts[t]..char_starts[t+1]]`.
fn build_candidates_bpe(
    chars: &[char],
    n_tok: usize,
    char_starts: &[usize],
    sep_penalties: &[(String, f64)],
) -> (Vec<usize>, Vec<f64>) {
    let mut cand: BTreeMap<usize, f64> = BTreeMap::new();
    cand.insert(0, 0.0);
    cand.insert(n_tok, 0.0);

    for t in 0..n_tok.saturating_sub(1) {
        // Le post-processor Qwen ajoute `<|endoftext|>` en fin (offset (0,0)) →
        // `char_starts` n'est pas monotone sur ce token. Python slice `text[a:b]`
        // renvoie `""` quand `a >= b` ; on réplique (span vide, aucun candidat).
        let (a, b) = (char_starts[t], char_starts[t + 1]);
        let span: &[char] = if a < b { &chars[a..b] } else { &[] };
        for (sep, rho) in sep_penalties {
            let sep_chars: Vec<char> = sep.chars().collect();
            if contains_subslice(span, &sep_chars) {
                let boundary = t + 1;
                cand.entry(boundary)
                    .and_modify(|cur| {
                        if *rho < *cur {
                            *cur = *rho;
                        }
                    })
                    .or_insert(*rho);
                break;
            }
        }
    }

    let sorted_pos: Vec<usize> = cand.keys().copied().collect();
    let rhos: Vec<f64> = sorted_pos.iter().map(|p| cand[p]).collect();
    (sorted_pos, rhos)
}

fn contains_subslice(haystack: &[char], needle: &[char]) -> bool {
    find_char(haystack, needle, 0).is_some()
}

// ---------------------------------------------------------------------------
// DP layer-par-layer
// ---------------------------------------------------------------------------

/// `bisect_left` : premier indice `i` tel que `arr[i] >= x`.
fn bisect_left(arr: &[usize], x: usize) -> usize {
    arr.partition_point(|&v| v < x)
}

/// `bisect_right` : premier indice `i` tel que `arr[i] > x`.
fn bisect_right(arr: &[usize], x: usize) -> usize {
    arr.partition_point(|&v| v <= x)
}

#[allow(clippy::too_many_arguments)]
fn dp_for_n(
    s_arr: &[usize],
    rho_arr: &[f64],
    n: usize,
    t_total: usize,
    k: usize,
    pi_0: usize,
    pi: usize,
    o_min: usize,
    o_max: usize,
    o_tgt: usize,
    params: &ChunkerParams,
) -> Option<(Vec<usize>, Vec<Option<usize>>)> {
    let m = s_arr.len();
    if m < 2 || s_arr[0] != 0 || s_arr[m - 1] != t_total {
        return None;
    }
    let idx_t = m - 1;

    let tau =
        (t_total as f64 + pi_0 as f64 + (n as f64 - 1.0) * (pi as f64 + o_tgt as f64)) / n as f64;
    let k_inv = 1.0 / k as f64;
    let inf = f64::INFINITY;
    let lambda_size = params.lambda_size;
    let lambda_ctx = params.lambda_ctx;

    // f[i][j], parent_prev[i][j], parent_c[i][j]
    let mut f = vec![vec![inf; m]; n + 1];
    let mut parent_prev = vec![vec![-1i64; m]; n + 1];
    let mut parent_c = vec![vec![-1i64; m]; n + 1];
    f[0][0] = 0.0;

    // Layer 0 → 1 : place q_1, sur tous les j ∈ (0, T) avec π_0 + S[j] ≤ K.
    let k_minus_pi0 = k as i64 - pi_0 as i64;
    let j_max1 = if k_minus_pi0 < 0 {
        0
    } else {
        bisect_right(s_arr, k_minus_pi0 as usize).min(idx_t)
    };
    if j_max1 > 1 {
        for j in 1..j_max1 {
            let q1 = s_arr[j] as f64;
            let z = (pi_0 as f64 + q1 - tau) * k_inv;
            let cost_0 = lambda_size * z * z + rho_arr[j];
            f[1][j] = cost_0;
            parent_prev[1][j] = 0;
        }
    }

    let max_own_rest = k as i64 - pi as i64 - o_min as i64;
    if max_own_rest <= 0 {
        return None;
    }
    let max_own_rest = max_own_rest as usize;

    for i in 1..n {
        let is_last = i + 1 == n;

        let active_from: Vec<usize> = (0..m).filter(|&j| f[i][j].is_finite()).collect();
        if active_from.is_empty() {
            return None;
        }

        for &j_from in &active_from {
            let q_from = s_arr[j_from];
            let f_i_jfrom = f[i][j_from];

            let c_lo_base = (q_from as i64 - o_max as i64).max(0) as usize;
            let c_hi = q_from as i64 - o_min as i64;
            if c_hi < 0 {
                continue;
            }
            let c_hi = c_hi as usize;
            if c_lo_base > c_hi {
                continue;
            }
            let jc_lo = bisect_left(s_arr, c_lo_base);
            let jc_hi = bisect_right(s_arr, c_hi);
            if jc_lo >= jc_hi {
                continue;
            }

            if is_last {
                let own_last = t_total as i64 - q_from as i64;
                if own_last <= 0 || own_last as usize > max_own_rest {
                    continue;
                }
                // Single q_to = T, on parcourt la fenêtre c.
                let mut best_c_local: Option<usize> = None;
                let mut best_edge_val = inf;
                for (offset, jc) in (jc_lo..jc_hi).enumerate() {
                    let c_val = s_arr[jc];
                    let e_val = pi as i64 + t_total as i64 - c_val as i64;
                    if e_val > k as i64 {
                        continue;
                    }
                    let z = (e_val as f64 - tau) * k_inv;
                    let edge = lambda_size * z * z + lambda_ctx * rho_arr[jc];
                    if edge < best_edge_val {
                        best_edge_val = edge;
                        best_c_local = Some(offset);
                    }
                }
                let Some(best_c_local) = best_c_local else {
                    continue;
                };
                if !best_edge_val.is_finite() {
                    continue;
                }
                let total = f_i_jfrom + best_edge_val + rho_arr[idx_t];
                if total < f[i + 1][idx_t] {
                    f[i + 1][idx_t] = total;
                    parent_prev[i + 1][idx_t] = j_from as i64;
                    parent_c[i + 1][idx_t] = (jc_lo + best_c_local) as i64;
                }
                continue;
            }

            let jt_lo = bisect_right(s_arr, q_from);
            let mut jt_hi = bisect_right(s_arr, q_from + max_own_rest);
            if jt_hi > idx_t {
                jt_hi = idx_t;
            }
            if jt_lo >= jt_hi {
                continue;
            }

            // Pour chaque q_to dans [jt_lo, jt_hi), trouve le meilleur c dans [jc_lo, jc_hi).
            for jt in jt_lo..jt_hi {
                let q_to = s_arr[jt];
                let mut best_c_idx: Option<usize> = None;
                let mut best_edge = inf;
                for jc in jc_lo..jc_hi {
                    let c_val = s_arr[jc];
                    let e = pi as i64 + q_to as i64 - c_val as i64;
                    let z = (e as f64 - tau) * k_inv;
                    let mut edge = lambda_size * z * z + lambda_ctx * rho_arr[jc];
                    if e > k as i64 {
                        edge = inf;
                    }
                    // argmin : premier minimum strict (np.argmin renvoie le 1er min).
                    if edge < best_edge {
                        best_edge = edge;
                        best_c_idx = Some(jc);
                    }
                }
                // np.argmin sur une ligne tout-INF renvoie 0 ; mais edge_grid[E>K]=INF
                // puis best_edge ajouté à f_i : reste INF, improved=False. On reproduit
                // en ne mettant à jour que si fini.
                let (best_edge, best_c_jc) = match best_c_idx {
                    Some(jc) => (best_edge, jc),
                    None => (inf, jc_lo),
                };
                let total = f_i_jfrom + best_edge + rho_arr[jt];
                if total < f[i + 1][jt] {
                    f[i + 1][jt] = total;
                    parent_prev[i + 1][jt] = j_from as i64;
                    parent_c[i + 1][jt] = best_c_jc as i64;
                }
            }
        }
    }

    if !f[n][idx_t].is_finite() {
        return None;
    }

    // Backtrace
    let mut qs_idx = vec![0usize; n + 1];
    let mut cs_idx: Vec<Option<usize>> = vec![None; n];
    qs_idx[n] = idx_t;
    let mut cur_j = idx_t;
    for layer in (1..=n).rev() {
        let prev_j = parent_prev[layer][cur_j];
        let c_idx = parent_c[layer][cur_j];
        if prev_j < 0 {
            return None;
        }
        if layer >= 2 {
            cs_idx[layer - 1] = if c_idx >= 0 {
                Some(c_idx as usize)
            } else {
                None
            };
        }
        qs_idx[layer - 1] = prev_j as usize;
        cur_j = prev_j as usize;
    }

    let qs: Vec<usize> = qs_idx.iter().map(|&j| s_arr[j]).collect();
    let cs: Vec<Option<usize>> = cs_idx.iter().map(|&c| c.map(|ci| s_arr[ci])).collect();
    Some((qs, cs))
}

/// Minimum n tel que `(K - π_0) + (n-1)(K - π - O_min) ≥ T`.
fn n_min(t_total: usize, k: usize, pi_0: usize, pi: usize, o_min: usize) -> usize {
    if k as i64 - pi_0 as i64 >= t_total as i64 {
        return 1;
    }
    let denom = k as i64 - pi as i64 - o_min as i64;
    if denom <= 0 {
        return 9999;
    }
    let numer = t_total as i64 - (k as i64 - pi_0 as i64);
    // math.ceil(numer / denom) + 1, numer > 0, denom > 0
    (((numer + denom - 1) / denom) + 1) as usize
}

// ---------------------------------------------------------------------------
// Chunker
// ---------------------------------------------------------------------------

/// Paramètres en unités (char ou token) préparés par `chunk_char`/`chunk_bpe`,
/// consommés par la DP commune.
struct Prepared {
    t_total: usize,
    k: usize,
    o_min_u: usize,
    o_max_u: usize,
    o_tgt_u: usize,
    pi_0: usize,
    pi: usize,
    cands: Vec<usize>,
    rhos: Vec<f64>,
    /// En mode BPE, mappe une frontière token → indice char. `None` en char-mode.
    char_starts_bpe: Option<Vec<usize>>,
}

/// Découpe `text` en chunks en **mode caractères** (chemin nominal rapide).
/// Budget conservateur via `CHARS_PER_TOKEN_SAFE` ; pas de tokenizer.
pub fn chunk_char(
    text: &str,
    metadata_header: &str,
    visa_trim: &str,
    chunk_tokens: usize,
    overlap_min: usize,
    overlap_max: usize,
    params: Option<&ChunkerParams>,
) -> Result<Vec<Chunk>> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    let default_params = ChunkerParams::default();
    let params = params.unwrap_or(&default_params);

    let chars: Vec<char> = text.chars().collect();
    let char_len = chars.len();

    // 1 unité = 1 char, budget conservateur.
    let k = (chunk_tokens as f64 * CHARS_PER_TOKEN_SAFE) as usize;
    let o_min_u = (overlap_min as f64 * CHARS_PER_TOKEN_MEDIAN) as usize;
    let o_max_u = (overlap_max as f64 * CHARS_PER_TOKEN_MEDIAN) as usize;

    // Longueurs exactes en chars des préfixes d'embedding_text (hors ctx+body).
    let pi_0 = if !metadata_header.is_empty() {
        metadata_header.chars().count() + 2
    } else {
        0
    };
    let pi = pi_0 + visa_trim.chars().count() + TRUNCATION_MARKER.chars().count();

    let (cands, rhos) = build_candidates_chars(&chars, &params.sep_penalties);

    solve_and_assemble(
        text,
        metadata_header,
        visa_trim,
        &chars,
        char_len,
        params,
        Prepared {
            t_total: char_len,
            k,
            o_min_u,
            o_max_u,
            o_tgt_u: o_min_u,
            pi_0,
            pi,
            cands,
            rhos,
            char_starts_bpe: None,
        },
    )
}

/// Découpe `text` en chunks en **mode BPE exact** : 1 unité = 1 token Qwen,
/// budget en tokens réels → `embedding_text` garanti ≤ `chunk_tokens`. Repli
/// déclenché quand l'heuristique char sous-estime et qu'un chunk dépasse le
/// contexte de l'embedder. Le tokenizer est fourni déjà construit (l'I/O de
/// chargement vit dans `pipeline.rs`).
#[allow(clippy::too_many_arguments)]
pub fn chunk_bpe(
    text: &str,
    metadata_header: &str,
    visa_trim: &str,
    chunk_tokens: usize,
    overlap_min: usize,
    overlap_max: usize,
    tokenizer: &Tokenizer,
    params: Option<&ChunkerParams>,
) -> Result<Vec<Chunk>> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    let default_params = ChunkerParams::default();
    let params = params.unwrap_or(&default_params);

    let chars: Vec<char> = text.chars().collect();
    let char_len = chars.len();

    // Le binding Python `encode` => offsets char + add_special_tokens=True.
    let encoding = tokenizer
        .encode_char_offsets(text, true)
        .map_err(|e| anyhow!("tokenizer encode: {e}"))?;
    let t_tok = encoding.get_ids().len();
    let mut starts: Vec<usize> = encoding.get_offsets().iter().map(|o| o.0).collect();
    starts.push(char_len);

    let prefix_0_str = if !metadata_header.is_empty() {
        format!("{metadata_header}\n\n")
    } else {
        String::new()
    };
    let mut prefix_rest = String::new();
    if !metadata_header.is_empty() {
        prefix_rest.push_str(metadata_header);
        prefix_rest.push_str("\n\n");
    }
    if !visa_trim.is_empty() {
        prefix_rest.push_str(visa_trim);
    }
    prefix_rest.push_str(TRUNCATION_MARKER);

    let pi_0 = if prefix_0_str.is_empty() {
        0
    } else {
        tokenizer
            .encode_char_offsets(prefix_0_str.as_str(), true)
            .map_err(|e| anyhow!("tokenizer encode: {e}"))?
            .get_ids()
            .len()
    };
    let pi = tokenizer
        .encode_char_offsets(prefix_rest.as_str(), true)
        .map_err(|e| anyhow!("tokenizer encode: {e}"))?
        .get_ids()
        .len();

    let (cands, rhos) = build_candidates_bpe(&chars, t_tok, &starts, &params.sep_penalties);

    solve_and_assemble(
        text,
        metadata_header,
        visa_trim,
        &chars,
        char_len,
        params,
        Prepared {
            t_total: t_tok,
            k: chunk_tokens,
            o_min_u: overlap_min,
            o_max_u: overlap_max,
            o_tgt_u: overlap_min,
            pi_0,
            pi,
            cands,
            rhos,
            char_starts_bpe: Some(starts),
        },
    )
}

/// DP (plus petit n faisable) + assemblage des `Chunk` (conversion unité → char),
/// commun aux deux modes.
fn solve_and_assemble(
    text: &str,
    metadata_header: &str,
    visa_trim: &str,
    chars: &[char],
    char_len: usize,
    params: &ChunkerParams,
    prep: Prepared,
) -> Result<Vec<Chunk>> {
    let Prepared {
        t_total,
        k,
        o_min_u,
        o_max_u,
        o_tgt_u,
        pi_0,
        pi,
        cands,
        rhos,
        char_starts_bpe,
    } = prep;

    // ── Cas trivial : un seul chunk ───────────────────────────────────────────
    if k as i64 - pi_0 as i64 >= t_total as i64 {
        return Ok(vec![Chunk {
            chunk_index: 0,
            char_start: 0,
            char_end: char_len,
            ctx_start: 0,
            body: text.to_string(),
            ctx: String::new(),
            metadata_header: metadata_header.to_string(),
            visa_trim: visa_trim.to_string(),
        }]);
    }

    // ── DP : cherche le plus petit n faisable ─────────────────────────────────
    let n_lo = n_min(t_total, k, pi_0, pi, o_min_u).max(2);
    let mut solution: Option<(Vec<usize>, Vec<Option<usize>>)> = None;
    for n in n_lo..(n_lo + 50) {
        if let Some(result) = dp_for_n(
            &cands, &rhos, n, t_total, k, pi_0, pi, o_min_u, o_max_u, o_tgt_u, params,
        ) {
            solution = Some(result);
            break;
        }
    }
    let (qs, cs) = solution.ok_or_else(|| anyhow!("chunker DP infeasible: T={t_total} K={k}"))?;

    // ── Assemblage des Chunk (conversion unité → char) ────────────────────────
    let n_chunks = qs.len() - 1;
    let mut chunks = Vec::with_capacity(n_chunks);
    for i in 0..n_chunks {
        let ctx_unit = cs[i];
        let (q_start, q_end, ctx_start) = if let Some(ref starts) = char_starts_bpe {
            let q_start = starts[qs[i]];
            let q_end = starts[qs[i + 1]];
            let ctx_start = match ctx_unit {
                None => q_start,
                Some(cu) => starts[cu],
            };
            (q_start, q_end, ctx_start)
        } else {
            let q_start = qs[i];
            let q_end = qs[i + 1];
            let ctx_start = ctx_unit.unwrap_or(q_start);
            (q_start, q_end, ctx_start)
        };
        let ctx = if i > 0 {
            chars[ctx_start..q_start].iter().collect::<String>()
        } else {
            String::new()
        };
        let body = chars[q_start..q_end].iter().collect::<String>();
        chunks.push(Chunk {
            chunk_index: i,
            char_start: q_start,
            char_end: q_end,
            ctx_start,
            body,
            ctx,
            metadata_header: metadata_header.to_string(),
            visa_trim: visa_trim.to_string(),
        });
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn empty_text_yields_no_chunks() {
        let out = chunk_char("", "", "", DEFAULT_CHUNK_TOKENS, 125, 250, None).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn short_text_single_chunk_char_mode() {
        // T tout petit << K = 8192*3.20 → un seul chunk, body == text intégral.
        let text = "Le tribunal administratif rejette la requête.\n\nFin.";
        let out = chunk_char(text, "", "", DEFAULT_CHUNK_TOKENS, 125, 250, None).unwrap();
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!(c.chunk_index, 0);
        assert_eq!(c.char_start, 0);
        assert_eq!(c.char_end, text.chars().count());
        assert_eq!(c.body, text);
        assert_eq!(c.ctx, "");
    }

    #[test]
    fn embedding_text_chunk0_with_header() {
        let c = Chunk {
            chunk_index: 0,
            char_start: 0,
            char_end: 4,
            ctx_start: 0,
            body: "body".to_string(),
            ctx: String::new(),
            metadata_header: "HDR".to_string(),
            visa_trim: "VISA".to_string(),
        };
        assert_eq!(c.embedding_text(), "HDR\n\nbody");
    }

    #[test]
    fn embedding_text_chunk0_no_header() {
        let c = Chunk {
            chunk_index: 0,
            char_start: 0,
            char_end: 4,
            ctx_start: 0,
            body: "body".to_string(),
            ctx: String::new(),
            metadata_header: String::new(),
            visa_trim: String::new(),
        };
        assert_eq!(c.embedding_text(), "body");
    }

    #[test]
    fn embedding_text_chunk_rest_assembles_all_parts() {
        let c = Chunk {
            chunk_index: 1,
            char_start: 10,
            char_end: 20,
            ctx_start: 5,
            body: "BODY".to_string(),
            ctx: "CTX".to_string(),
            metadata_header: "HDR".to_string(),
            visa_trim: "VISA".to_string(),
        };
        // HDR + "\n\n" + VISA + marker + CTX + BODY
        let expected = format!("HDR\n\nVISA{TRUNCATION_MARKER}CTXBODY");
        assert_eq!(c.embedding_text(), expected);
    }

    #[test]
    fn embedding_text_chunk_rest_no_header_no_visa() {
        let c = Chunk {
            chunk_index: 2,
            char_start: 10,
            char_end: 20,
            ctx_start: 5,
            body: "BODY".to_string(),
            ctx: "CTX".to_string(),
            metadata_header: String::new(),
            visa_trim: String::new(),
        };
        let expected = format!("{TRUNCATION_MARKER}CTXBODY");
        assert_eq!(c.embedding_text(), expected);
    }

    #[test]
    fn candidates_chars_marks_after_each_separator() {
        // "a\n\nbb. c" (8 codepoints) → séparateurs : "\n\n", ". ", ".", "\n"
        let chars: Vec<char> = "a\n\nbb. c".chars().collect();
        let n = chars.len(); // 8
        let pen: Vec<(String, f64)> = DEFAULT_SEP_PENALTIES
            .iter()
            .map(|(s, p)| (s.to_string(), *p))
            .collect();
        let (pos, rho) = build_candidates_chars(&chars, &pen);
        // 0 et n toujours présents.
        assert_eq!(pos[0], 0);
        assert_eq!(*pos.last().unwrap(), n);
        // toutes les positions entre 0 et n.
        assert!(pos.iter().all(|&p| p <= n));
        // rho aligné.
        assert_eq!(pos.len(), rho.len());
    }

    #[test]
    fn bisect_helpers_match_python() {
        let arr = [0usize, 2, 4, 4, 6];
        assert_eq!(bisect_left(&arr, 4), 2);
        assert_eq!(bisect_right(&arr, 4), 4);
        assert_eq!(bisect_left(&arr, 5), 4);
        assert_eq!(bisect_right(&arr, 6), 5);
        assert_eq!(bisect_left(&arr, 0), 0);
    }

    #[test]
    fn n_min_trivial_and_general() {
        // K - pi_0 >= T → 1
        assert_eq!(n_min(100, 1000, 10, 5, 5), 1);
        // général : T=1000, K=300, pi_0=10, pi=20, o_min=30
        // denom = 300-20-30 = 250 ; numer = 1000 - (300-10) = 710
        // ceil(710/250)+1 = 3+1 = 4
        assert_eq!(n_min(1000, 300, 10, 20, 30), 4);
    }

    #[test]
    fn multi_chunk_char_mode_covers_full_text_without_gaps() {
        // Texte long forçant plusieurs chunks avec un petit budget.
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("Phrase numero {i} du corps de la decision.\n\n"));
        }
        // budget volontairement petit : chunk_tokens tel que K ~ petit.
        // K = chunk_tokens * 3.20 ; on veut K << len(text).
        let chunk_tokens = 200; // K = 640
        let out = chunk_char(
            &text,
            "",
            "",
            chunk_tokens,
            DEFAULT_OVERLAP_MIN,
            DEFAULT_OVERLAP_MAX,
            None,
        )
        .unwrap();
        assert!(
            out.len() >= 2,
            "attendu plusieurs chunks, got {}",
            out.len()
        );
        let total_chars = text.chars().count();
        // Premier body commence à 0, dernier finit à T.
        assert_eq!(out[0].char_start, 0);
        assert_eq!(out.last().unwrap().char_end, total_chars);
        // Les bodies sont contigus (own_i = x[q_i:q_{i+1}], couverture sans trou).
        for w in out.windows(2) {
            assert_eq!(w[0].char_end, w[1].char_start);
        }
        // chunk_index monotone.
        for (i, c) in out.iter().enumerate() {
            assert_eq!(c.chunk_index, i);
        }
        // Chaque body est bien le slice char correspondant.
        let chars: Vec<char> = text.chars().collect();
        for c in &out {
            let slice: String = chars[c.char_start..c.char_end].iter().collect();
            assert_eq!(c.body, slice);
        }
        // ctx présent pour i >= 1 (overlap gauche), absent pour i == 0.
        assert_eq!(out[0].ctx, "");
        assert!(!out[1].ctx.is_empty());
    }

    #[test]
    fn candidates_bpe_tolerates_non_monotonic_char_starts() {
        // Le post-processor Qwen ajoute `<|endoftext|>` en fin, d'offset (0,0) :
        // `char_starts` n'est donc pas monotone (un 0 après le dernier token réel).
        // Le span de ce token doit être traité comme vide (slicing Python `a:b`,
        // a >= b → ""), sans paniquer. Régression du mode BPE.
        let chars: Vec<char> = "a\n\nbb".chars().collect(); // 5 chars
        let pen: Vec<(String, f64)> = DEFAULT_SEP_PENALTIES
            .iter()
            .map(|(s, p)| (s.to_string(), *p))
            .collect();
        // 4 tokens : [a][\n\n][bb][EOS], EOS d'offset 0 → starts non monotone.
        // char_starts a n_tok+1 entrées (offsets + len poussée).
        let n_tok = 4;
        let char_starts = vec![0usize, 1, 3, 0, 5];
        let (pos, rho) = build_candidates_bpe(&chars, n_tok, &char_starts, &pen);
        // Pas de panic ; 0 et n_tok toujours présents, alignés.
        assert_eq!(pos[0], 0);
        assert_eq!(*pos.last().unwrap(), n_tok);
        assert_eq!(pos.len(), rho.len());
    }

    // Golden char-mode gelé (ex-`lj-core/tests/chunking_regression.rs`).
    //
    // La DP de découpe (`chunk_char`) est de la logique profonde où une dérive de
    // frontière est invisible à l'œil. On fige sa sortie char-mode sur une entrée
    // déterministe (séparateurs variés, accents → offsets en codepoints, header +
    // visa → préfixe non trivial) : index, `char_start`/`char_end`/`ctx_start`
    // (codepoints), `body`, `ctx`. Golden non éditable à la main ; régénérer
    // depuis `chunk_char` si le comportement change volontairement.
    #[derive(Deserialize)]
    struct Fixture {
        input: GoldenInput,
        chunks: Vec<ExpectedChunk>,
    }

    #[derive(Deserialize)]
    struct GoldenInput {
        text: String,
        metadata_header: String,
        visa_trim: String,
        chunk_tokens: usize,
        overlap_min: usize,
        overlap_max: usize,
    }

    #[derive(Deserialize)]
    struct ExpectedChunk {
        chunk_index: usize,
        char_start: usize,
        char_end: usize,
        ctx_start: usize,
        body: String,
        ctx: String,
    }

    #[test]
    fn chunking_char_mode_golden() {
        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/golden/chunking_char_mode.json"
        ));
        let fix: Fixture = serde_json::from_str(raw).expect("fixture chunking_char_mode");

        let chunks = chunk_char(
            &fix.input.text,
            &fix.input.metadata_header,
            &fix.input.visa_trim,
            fix.input.chunk_tokens,
            fix.input.overlap_min,
            fix.input.overlap_max,
            None,
        )
        .expect("chunk_char ne doit pas échouer sur l'entrée golden");

        assert_eq!(
            chunks.len(),
            fix.chunks.len(),
            "nombre de chunks (DP a divergé)"
        );
        for (c, e) in chunks.iter().zip(&fix.chunks) {
            let idx = e.chunk_index;
            assert_eq!(c.chunk_index, e.chunk_index, "chunk[{idx}].chunk_index");
            assert_eq!(c.char_start, e.char_start, "chunk[{idx}].char_start");
            assert_eq!(c.char_end, e.char_end, "chunk[{idx}].char_end");
            assert_eq!(c.ctx_start, e.ctx_start, "chunk[{idx}].ctx_start");
            assert_eq!(c.body, e.body, "chunk[{idx}].body");
            assert_eq!(c.ctx, e.ctx, "chunk[{idx}].ctx");
        }
    }
}
