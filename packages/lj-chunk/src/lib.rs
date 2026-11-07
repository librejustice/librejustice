//! Chunker: a dynamic programme over the cut points of a text.
//!
//! Two entries: [`chunk_char`] (character mode, the nominal fast path) and
//! [`chunk_bpe`] (exact BPE mode through the HF tokenizer, the fallback when the
//! embedder overflows), and [`chunk_aligned`], which cuts the translations of
//! one decision in the same number of chunks. The layer-by-layer DP and the
//! assembly are shared; only the candidates differ.
//!
//! Everything is indexed in **char units**, never bytes. The BPE offsets come
//! from `encode_char_offsets`.
//!
//! Kept out of `lj-ingest`, its only consumer, which compiles at opt-0: the
//! dynamic programme is the ingest's CPU hot path.

use anyhow::{anyhow, Result};
use lj_vocab::passage_zones::{ANNEXES, SUMMARY, TEXT};
use lj_vocab::tokens::chars_per_token_safe;
use std::collections::BTreeMap;

pub const DEFAULT_CHUNK_TOKENS: usize = 8192;

/// The embedding host's context length: a passage at or above it is an overflow.
pub const EMBED_CONTEXT_TOKENS: usize = 10_240;

/// Chunk and embed generation, persisted in `decisions.embed_version` when
/// embeddings are written: `embed-missing --stock` rebuilds the passages of
/// every decision below it. Bumped on any change of the chunker, the prefix or
/// the model.
pub const EMBED_VERSION: i16 = 3;

/// Separator penalties ρ (lower is better).
pub const DEFAULT_SEP_PENALTIES: &[(&str, f64)] = &[
    ("\n\n", 0.0),
    ("\n", 1.0),
    (". ", 3.0),
    (".", 5.0),
    (";", 7.0),
    (",", 9.0),
];
pub const DEFAULT_LAMBDA_SIZE: f64 = 1.0;

/// HF tokenizer of the exact BPE mode. `lj-ingest` loads it.
pub type Tokenizer = tokenizers::Tokenizer;

/// The cost of a cut: `cost_i = λ_size·((own_i − T/n)/K)² + ρ(q_{i+1})`.
#[derive(Debug, Clone)]
pub struct ChunkerParams {
    pub lambda_size: f64,
    pub sep_penalties: Vec<(String, f64)>,
}

impl Default for ChunkerParams {
    fn default() -> Self {
        Self {
            lambda_size: DEFAULT_LAMBDA_SIZE,
            sep_penalties: DEFAULT_SEP_PENALTIES
                .iter()
                .map(|(s, p)| (s.to_string(), *p))
                .collect(),
        }
    }
}

/// What every chunk of a decision is embedded behind: its metadata header,
/// then its summary.
pub fn embedding_prefix(metadata_header: &str, summary: Option<&str>) -> String {
    let mut prefix = String::new();
    for part in [Some(metadata_header), summary].into_iter().flatten() {
        let part = part.trim();
        if !part.is_empty() {
            prefix.push_str(part);
            prefix.push_str("\n\n");
        }
    }
    prefix
}

/// A chunk ready to embed. Its embedding text is assembled on demand, never
/// stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub chunk_index: usize,
    pub char_start: usize,
    pub char_end: usize,
    /// `x[char_start:char_end]`.
    pub body: String,
    /// The [`embedding_prefix`] of its decision.
    pub prefix: String,
}

impl Chunk {
    /// What the embedder reads: the prefix, then the body.
    pub fn embedding_text(&self) -> String {
        format!("{}{}", self.prefix, self.body)
    }

    /// The tags of what the chunk holds, see [`zones`].
    pub fn zones(&self, summary: bool, annex_start: Option<usize>) -> Vec<&'static str> {
        zones(self.char_start, self.char_end, summary, annex_start)
    }
}

/// The tags of a passage `[char_start, char_end)`: `summary` when its prefix
/// carries one, `text` when it crosses the text before `annex_start`,
/// `annexes` when it crosses the annexes.
pub fn zones(
    char_start: usize,
    char_end: usize,
    summary: bool,
    annex_start: Option<usize>,
) -> Vec<&'static str> {
    let boundary = annex_start.unwrap_or(usize::MAX);
    let mut zones = Vec::with_capacity(3);
    if summary {
        zones.push(SUMMARY);
    }
    if char_start < boundary {
        zones.push(TEXT);
    }
    if char_end > boundary {
        zones.push(ANNEXES);
    }
    zones
}

// ---------------------------------------------------------------------------
// Candidate cut points S
// ---------------------------------------------------------------------------

/// Char mode: the position right after each separator, plus 0 and len(text).
///
/// `chars` is `text` split into chars (positions are char indices).
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
        // `char_starts` n'est pas monotone sur ce token : `a >= b` ⇒ span vide,
        // aucun candidat.
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
// Layer-by-layer DP
// ---------------------------------------------------------------------------

/// `bisect_right`: the first index `i` with `arr[i] > x`.
fn bisect_right(arr: &[usize], x: usize) -> usize {
    arr.partition_point(|&v| v <= x)
}

/// The cuts `q_0 = 0 < … < q_n = t` among the candidates `s` that minimise the
/// summed cost, each chunk holding at most `room` units; `None` when `n` chunks
/// cannot hold the text.
fn dp_for_n(
    s: &[usize],
    rho: &[f64],
    n: usize,
    t: usize,
    k: usize,
    room: usize,
    params: &ChunkerParams,
) -> Option<Vec<usize>> {
    let m = s.len();
    if m < 2 || s[0] != 0 || s[m - 1] != t {
        return None;
    }
    let last = m - 1;
    let tau = t as f64 / n as f64;
    let k_inv = 1.0 / k as f64;
    let mut f = vec![vec![f64::INFINITY; m]; n + 1];
    let mut parent = vec![vec![usize::MAX; m]; n + 1];
    f[0][0] = 0.0;

    for i in 0..n {
        let closing = i + 1 == n;
        for j in 0..m {
            let base = f[i][j];
            if !base.is_finite() {
                continue;
            }
            let q = s[j];
            let reach = bisect_right(s, q + room);
            let targets = if closing {
                if reach <= last {
                    continue;
                }
                last..last + 1
            } else {
                j + 1..reach.min(last)
            };
            for jt in targets {
                let z = ((s[jt] - q) as f64 - tau) * k_inv;
                let cost = base + params.lambda_size * z * z + rho[jt];
                if cost < f[i + 1][jt] {
                    f[i + 1][jt] = cost;
                    parent[i + 1][jt] = j;
                }
            }
        }
    }
    if !f[n][last].is_finite() {
        return None;
    }

    let mut cuts = vec![0usize; n + 1];
    let mut j = last;
    for layer in (1..=n).rev() {
        cuts[layer] = s[j];
        j = parent[layer][j];
    }
    Some(cuts)
}

// ---------------------------------------------------------------------------
// Chunker
// ---------------------------------------------------------------------------

/// A text made ready for the shared DP, in chars or in tokens.
struct Prepared {
    chars: Vec<char>,
    t_total: usize,
    k: usize,
    /// The prefix, in units.
    pi: usize,
    cands: Vec<usize>,
    rhos: Vec<f64>,
    /// BPE mode: maps a token boundary to its char index. `None` in char mode.
    char_starts_bpe: Option<Vec<usize>>,
}

/// Cuts `text` in **char mode** (the nominal fast path), each chunk budgeted
/// with `prefix` through the chars per token of `lang`, without a tokenizer.
pub fn chunk_char(
    text: &str,
    lang: &str,
    prefix: &str,
    chunk_tokens: usize,
    params: Option<&ChunkerParams>,
) -> Result<Vec<Chunk>> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    let default_params = ChunkerParams::default();
    let params = params.unwrap_or(&default_params);
    let prep = prepare_char(text, lang, prefix, chunk_tokens, params);
    let cuts = least_cuts(&prep, params)?;
    Ok(assemble(prefix, &prep, &cuts))
}

/// Cuts `text` in **exact BPE mode**: one unit is one Qwen token, so no
/// `embedding_text` exceeds `chunk_tokens`. The fallback when the char
/// estimate falls short and a chunk overflows the embedder's context.
pub fn chunk_bpe(
    text: &str,
    prefix: &str,
    chunk_tokens: usize,
    tokenizer: &Tokenizer,
    params: Option<&ChunkerParams>,
) -> Result<Vec<Chunk>> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    let default_params = ChunkerParams::default();
    let params = params.unwrap_or(&default_params);
    let prep = prepare_bpe(text, prefix, chunk_tokens, tokenizer, params)?;
    let cuts = least_cuts(&prep, params)?;
    Ok(assemble(prefix, &prep, &cuts))
}

/// Cuts each of `texts`, translations of one decision each with its language,
/// in the same number of chunks: the fewest every text fits in, each text cut
/// by the DP for that number, so that chunk `i` covers about the same ground in
/// every language. In exact BPE mode with `tokenizer`, in char mode without.
pub fn chunk_aligned(
    texts: &[(&str, &str)],
    prefix: &str,
    chunk_tokens: usize,
    tokenizer: Option<&Tokenizer>,
    params: Option<&ChunkerParams>,
) -> Result<Vec<Vec<Chunk>>> {
    let default_params = ChunkerParams::default();
    let params = params.unwrap_or(&default_params);
    let preps = texts
        .iter()
        .map(|(text, lang)| match tokenizer {
            Some(tokenizer) => prepare_bpe(text, prefix, chunk_tokens, tokenizer, params),
            None => Ok(prepare_char(text, lang, prefix, chunk_tokens, params)),
        })
        .collect::<Result<Vec<_>>>()?;
    let mut cuts = preps
        .iter()
        .map(|prep| least_cuts(prep, params))
        .collect::<Result<Vec<_>>>()?;
    let n = cuts.iter().map(|c| c.len() - 1).max().unwrap_or(0);
    for (c, prep) in cuts.iter_mut().zip(&preps) {
        if c.len() - 1 < n {
            *c = dp_for_n(
                &prep.cands,
                &prep.rhos,
                n,
                prep.t_total,
                prep.k,
                room(prep)?,
                params,
            )
            .ok_or_else(|| {
                anyhow!(
                    "chunker: a text of {} units does not cut in {n}",
                    prep.t_total
                )
            })?;
        }
    }
    Ok(preps
        .iter()
        .zip(&cuts)
        .map(|(prep, cuts)| assemble(prefix, prep, cuts))
        .collect())
}

/// Whether a passage of `chunks` counts `EMBED_CONTEXT_TOKENS` or more by `tokenizer`.
pub fn reaches_context(chunks: &[Vec<Chunk>], tokenizer: &Tokenizer) -> Result<bool> {
    for chunk in chunks.iter().flatten() {
        let encoding = tokenizer
            .encode(chunk.embedding_text(), true)
            .map_err(|e| anyhow!("tokenizer encode: {e}"))?;
        if encoding.get_ids().len() >= EMBED_CONTEXT_TOKENS {
            return Ok(true);
        }
    }
    Ok(false)
}

fn prepare_char(
    text: &str,
    lang: &str,
    prefix: &str,
    chunk_tokens: usize,
    params: &ChunkerParams,
) -> Prepared {
    let chars: Vec<char> = text.chars().collect();
    let (cands, rhos) = build_candidates_chars(&chars, &params.sep_penalties);
    Prepared {
        t_total: chars.len(),
        chars,
        k: (chunk_tokens as f64 * chars_per_token_safe(lang)) as usize,
        pi: prefix.chars().count(),
        cands,
        rhos,
        char_starts_bpe: None,
    }
}

fn prepare_bpe(
    text: &str,
    prefix: &str,
    chunk_tokens: usize,
    tokenizer: &Tokenizer,
    params: &ChunkerParams,
) -> Result<Prepared> {
    let chars: Vec<char> = text.chars().collect();
    let encoding = tokenizer
        .encode_char_offsets(text, true)
        .map_err(|e| anyhow!("tokenizer encode: {e}"))?;
    let t_tok = encoding.get_ids().len();
    let mut starts: Vec<usize> = encoding.get_offsets().iter().map(|o| o.0).collect();
    starts.push(chars.len());
    let pi = if prefix.is_empty() {
        0
    } else {
        tokenizer
            .encode_char_offsets(prefix, true)
            .map_err(|e| anyhow!("tokenizer encode: {e}"))?
            .get_ids()
            .len()
    };
    let (cands, rhos) = build_candidates_bpe(&chars, t_tok, &starts, &params.sep_penalties);
    Ok(Prepared {
        chars,
        t_total: t_tok,
        k: chunk_tokens,
        pi,
        cands,
        rhos,
        char_starts_bpe: Some(starts),
    })
}

/// The units a chunk holds behind the prefix.
fn room(prep: &Prepared) -> Result<usize> {
    prep.k
        .checked_sub(prep.pi)
        .filter(|&room| room > 0)
        .ok_or_else(|| {
            anyhow!(
                "chunker: a prefix of {} units leaves no room in K={}",
                prep.pi,
                prep.k
            )
        })
}

/// The cuts of the DP at the smallest feasible number of chunks.
fn least_cuts(prep: &Prepared, params: &ChunkerParams) -> Result<Vec<usize>> {
    let room = room(prep)?;
    if prep.t_total <= room {
        return Ok(vec![0, prep.t_total]);
    }
    let n_lo = prep.t_total.div_ceil(room).max(2);
    (n_lo..n_lo + 50)
        .find_map(|n| {
            dp_for_n(
                &prep.cands,
                &prep.rhos,
                n,
                prep.t_total,
                prep.k,
                room,
                params,
            )
        })
        .ok_or_else(|| {
            anyhow!(
                "chunker DP infeasible: T={} K={} prefix={}",
                prep.t_total,
                prep.k,
                prep.pi
            )
        })
}

/// The `Chunk`s of `cuts`, units turned into chars.
fn assemble(prefix: &str, prep: &Prepared, cuts: &[usize]) -> Vec<Chunk> {
    let to_char = |unit: usize| match &prep.char_starts_bpe {
        Some(starts) => starts[unit],
        None => unit,
    };
    cuts.windows(2)
        .enumerate()
        .map(|(i, w)| {
            let (char_start, char_end) = (to_char(w[0]), to_char(w[1]));
            Chunk {
                chunk_index: i,
                char_start,
                char_end,
                body: prep.chars[char_start..char_end].iter().collect(),
                prefix: prefix.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(chunk_char("", "fr", "", DEFAULT_CHUNK_TOKENS, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn short_text_single_chunk_char_mode() {
        let text = "Le tribunal administratif rejette la requête.\n\nFin.";
        let out = chunk_char(text, "fr", "HDR\n\n", DEFAULT_CHUNK_TOKENS, None).unwrap();
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!((c.chunk_index, c.char_start), (0, 0));
        assert_eq!(c.char_end, text.chars().count());
        assert_eq!(c.body, text);
        assert_eq!(c.embedding_text(), format!("HDR\n\n{text}"));
    }

    #[test]
    fn the_prefix_is_the_header_then_the_summary() {
        assert_eq!(
            embedding_prefix("CE | 2026-04-01", Some(" Le Conseil rejette. ")),
            "CE | 2026-04-01\n\nLe Conseil rejette.\n\n"
        );
        assert_eq!(embedding_prefix("", Some("Résumé")), "Résumé\n\n");
        assert_eq!(embedding_prefix("HDR", None), "HDR\n\n");
        assert_eq!(embedding_prefix("", None), "");
    }

    #[test]
    fn a_chunk_is_tagged_with_what_it_holds() {
        let chunk = |char_start, char_end| Chunk {
            chunk_index: 0,
            char_start,
            char_end,
            body: String::new(),
            prefix: String::new(),
        };
        assert_eq!(chunk(0, 10).zones(true, None), [SUMMARY, TEXT]);
        assert_eq!(chunk(0, 10).zones(false, Some(10)), [TEXT]);
        assert_eq!(chunk(0, 10).zones(true, Some(5)), [SUMMARY, TEXT, ANNEXES]);
        assert_eq!(chunk(5, 10).zones(false, Some(5)), [ANNEXES]);
    }

    #[test]
    fn candidates_chars_marks_after_each_separator() {
        let chars: Vec<char> = "a\n\nbb. c".chars().collect();
        let n = chars.len();
        let pen: Vec<(String, f64)> = DEFAULT_SEP_PENALTIES
            .iter()
            .map(|(s, p)| (s.to_string(), *p))
            .collect();
        let (pos, rho) = build_candidates_chars(&chars, &pen);
        assert_eq!(pos[0], 0);
        assert_eq!(*pos.last().unwrap(), n);
        assert!(pos.iter().all(|&p| p <= n));
        assert_eq!(pos.len(), rho.len());
    }

    #[test]
    fn multi_chunk_char_mode_covers_full_text_without_gaps() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("Phrase numero {i} du corps de la decision.\n\n"));
        }
        let prefix = embedding_prefix("HDR", Some("Le tribunal rejette la requête."));
        let chunk_tokens = 200;
        let out = chunk_char(&text, "fr", &prefix, chunk_tokens, None).unwrap();
        assert!(out.len() >= 2, "several chunks expected, got {}", out.len());
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(out[0].char_start, 0);
        assert_eq!(out.last().unwrap().char_end, chars.len());
        for w in out.windows(2) {
            assert_eq!(w[0].char_end, w[1].char_start);
        }
        let budget = (chunk_tokens as f64 * chars_per_token_safe("fr")) as usize;
        for (i, c) in out.iter().enumerate() {
            assert_eq!(c.chunk_index, i);
            assert_eq!(
                c.body,
                chars[c.char_start..c.char_end].iter().collect::<String>()
            );
            assert!(c.embedding_text().starts_with(&prefix));
            assert!(c.embedding_text().chars().count() <= budget);
        }
    }

    #[test]
    fn a_language_running_fewer_characters_a_token_is_cut_in_shorter_chunks() {
        let text: String = (0..400)
            .map(|i| format!("Phrase numero {i} du corps de la decision.\n\n"))
            .collect();
        let (prefix, chunk_tokens) = ("HDR\n\n", 200);
        let fr = chunk_char(&text, "fr", prefix, chunk_tokens, None).unwrap();
        let cs = chunk_char(&text, "cs", prefix, chunk_tokens, None).unwrap();
        assert!(cs.len() > fr.len());
        let budget = (chunk_tokens as f64 * chars_per_token_safe("cs")) as usize;
        assert!(cs
            .iter()
            .all(|c| c.embedding_text().chars().count() <= budget));
    }

    #[test]
    fn translations_are_cut_in_as_many_chunks_as_the_longest_needs_at_the_same_points() {
        let text = |sentence: &str| -> String {
            (0..150).map(|i| format!("{sentence} {i}.\n\n")).collect()
        };
        let short = text("Phrase numero du corps");
        let long = text("Ein viel laengerer Satz aus dem Hauptteil der Entscheidung");
        let (prefix, chunk_tokens) = ("HDR\n\n", 200);
        let alone = chunk_char(&short, "fr", prefix, chunk_tokens, None).unwrap();
        let out = chunk_aligned(
            &[(&short, "fr"), (&long, "fr")],
            prefix,
            chunk_tokens,
            None,
            None,
        )
        .unwrap();
        assert_eq!(out[0].len(), out[1].len());
        assert!(out[0].len() > alone.len());
        let budget = (chunk_tokens as f64 * chars_per_token_safe("fr")) as usize;
        for (text, chunks) in [&short, &long].into_iter().zip(&out) {
            assert_eq!(chunks[0].char_start, 0);
            assert_eq!(chunks.last().unwrap().char_end, text.chars().count());
            for w in chunks.windows(2) {
                assert_eq!(w[0].char_end, w[1].char_start);
            }
            assert!(chunks
                .iter()
                .all(|c| c.embedding_text().chars().count() <= budget));
        }
        let share = |chunks: &[Chunk], text: &str| -> Vec<f64> {
            let len = text.chars().count() as f64;
            chunks.iter().map(|c| c.char_end as f64 / len).collect()
        };
        for (a, b) in share(&out[0], &short).iter().zip(share(&out[1], &long)) {
            assert!(
                (a - b).abs() < 0.05,
                "cut at {a:.2} of one text, {b:.2} of the other"
            );
        }
    }

    #[test]
    fn candidates_bpe_tolerates_non_monotonic_char_starts() {
        let chars: Vec<char> = "a\n\nbb".chars().collect();
        let pen: Vec<(String, f64)> = DEFAULT_SEP_PENALTIES
            .iter()
            .map(|(s, p)| (s.to_string(), *p))
            .collect();
        let n_tok = 4;
        let char_starts = vec![0usize, 1, 3, 0, 5];
        let (pos, rho) = build_candidates_bpe(&chars, n_tok, &char_starts, &pen);
        assert_eq!(pos[0], 0);
        assert_eq!(*pos.last().unwrap(), n_tok);
        assert_eq!(pos.len(), rho.len());
    }

    // Frozen char-mode golden. A boundary drift in the DP is invisible to the
    // eye: its output is pinned on a deterministic input (varied separators,
    // accents for codepoint offsets, a non-trivial prefix). Not edited by hand;
    // regenerated from `chunk_char` when the behaviour changes on purpose.
    #[derive(Deserialize)]
    struct Fixture {
        input: GoldenInput,
        chunks: Vec<ExpectedChunk>,
    }

    #[derive(Deserialize)]
    struct GoldenInput {
        text: String,
        prefix: String,
        chunk_tokens: usize,
    }

    #[derive(Deserialize)]
    struct ExpectedChunk {
        chunk_index: usize,
        char_start: usize,
        char_end: usize,
        body: String,
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
            "fr",
            &fix.input.prefix,
            fix.input.chunk_tokens,
            None,
        )
        .expect("chunk_char on the golden input");
        assert_eq!(
            chunks.len(),
            fix.chunks.len(),
            "chunk count (the DP drifted)"
        );
        for (c, e) in chunks.iter().zip(&fix.chunks) {
            let idx = e.chunk_index;
            assert_eq!(c.chunk_index, e.chunk_index, "chunk[{idx}].chunk_index");
            assert_eq!(c.char_start, e.char_start, "chunk[{idx}].char_start");
            assert_eq!(c.char_end, e.char_end, "chunk[{idx}].char_end");
            assert_eq!(c.body, e.body, "chunk[{idx}].body");
        }
    }
}
