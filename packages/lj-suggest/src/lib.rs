//! Autocomplete vocabulary: 1- to 5-grams counted in document frequency,
//! `legal_text` titles injected with a boost, a cut, then serialisation as an
//! `fst::Map` (key `folded[\x00display]`, value `u64 = df_juris << 32 |
//! df_textes`).
//!
//! Kept out of `lj-ingest`, which compiles at opt-0: the count walks tens of
//! millions of n-grams.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_textsearch::body_tok::{is_stopword, tokenize, tokenize_lower};
use lj_textsearch::suggest::{harvest_ngrams, DISPLAY_SEP, MAX_KEY_TOKENS};

/// Cap of the counting map, above which a lossy prune runs with a rising
/// threshold. Sized so that the threshold stays under the df floors, or the rare
/// tail of the vocabulary disappears.
const COUNT_CAP: usize = 24_000_000;
/// Case-law df floor to enter the vocabulary.
const MIN_DF_JURIS: u32 = 5;
const MIN_DF_TEXTES: u32 = 3;
/// Maximum size of the final vocabulary, cut by decreasing df.
const VOCAB_MAX: usize = 3_000_000;
/// Texts-side df boost of the `legal_text` titles, the ideal suggestion of that
/// mode: enough to surface in context without crushing the corpus's own words
/// in the unigram fallback.
const TITLE_BOOST: u32 = 200;
/// Key length guard, for endless titles.
const KEY_MAX_CHARS: usize = 64;

const DOMAIN_JURIS: usize = 0;
const DOMAIN_TEXTES: usize = 1;

/// Count of a folded n-gram: df per domain and the majority display form
/// (Boyer-Moore vote, O(1) memory, converging on the dominant spelling:
/// « congés payés » wins over a mistyped « conges payes »).
struct Entry {
    df: [u32; 2],
    display: String,
    display_votes: i32,
}

/// df count bounded in memory: at the cap, entries under a rising threshold are
/// purged, a simplified lossy counting, the suggest reading only the top.
struct Counter {
    map: HashMap<String, Entry>,
    prune_below: u32,
}

impl Counter {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            prune_below: 1,
        }
    }

    fn bump(&mut self, folded: &str, display: &str, domain: usize, by: u32) {
        if let Some(e) = self.map.get_mut(folded) {
            e.df[domain] += by;
            if e.display == display {
                e.display_votes += 1;
            } else {
                e.display_votes -= 1;
                if e.display_votes <= 0 {
                    e.display = display.to_string();
                    e.display_votes = 1;
                }
            }
            return;
        }
        let mut df = [0u32; 2];
        df[domain] = by;
        self.map.insert(
            folded.to_string(),
            Entry {
                df,
                display: display.to_string(),
                display_votes: 1,
            },
        );
        if self.map.len() > COUNT_CAP {
            self.prune_below += 1;
            let t = self.prune_below;
            self.map.retain(|_, e| e.df[0] + e.df[1] >= t);
            tracing::info!(
                retained = self.map.len(),
                prune_below = t,
                "n-gram counting: lossy prune"
            );
        }
    }
}

/// Distinct n-grams of a document, as (folded, displayed) pairs: df counts
/// presence, so boilerplate repeated within a document counts once.
fn doc_ngrams(text: &str) -> HashSet<(String, String)> {
    let folded = tokenize(text);
    let display = tokenize_lower(text);
    let mut set = HashSet::new();
    harvest_ngrams(&folded, |start, end| {
        let key = folded[start..end].join(" ");
        if key.len() <= KEY_MAX_CHARS {
            set.insert((key, display[start..end].join(" ")));
        }
    });
    set
}

/// Count in progress: sampled decisions on the case-law side, articles on the
/// texts side, titles injected whole.
pub struct SuggestBuilder {
    counter: Counter,
}

impl Default for SuggestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SuggestBuilder {
    pub fn new() -> Self {
        Self {
            counter: Counter::new(),
        }
    }

    /// Entries of the counting map.
    pub fn entries(&self) -> usize {
        self.counter.map.len()
    }

    /// Counts the distinct n-grams of each decision, case-law side.
    pub fn count_decisions(&mut self, texts: &[String]) {
        self.count(texts, DOMAIN_JURIS);
    }

    /// Counts the distinct n-grams of each article, texts side.
    pub fn count_articles(&mut self, texts: &[String]) {
        self.count(texts, DOMAIN_TEXTES);
    }

    fn count(&mut self, texts: &[String], domain: usize) {
        let sets: Vec<HashSet<(String, String)>> =
            texts.par_iter().map(|text| doc_ngrams(text)).collect();
        for set in sets {
            for (folded, display) in set {
                self.counter.bump(&folded, &display, domain, 1);
            }
        }
    }

    /// Injects a whole `legal_text` title, trimmed to non-stopword edges and at
    /// most `MAX_KEY_TOKENS` tokens, with the texts-side boost. `false`: title
    /// dropped.
    pub fn inject_title(&mut self, title: &str) -> bool {
        let folded = tokenize(title);
        let display = tokenize_lower(title);
        let bounds = {
            let start = folded.iter().position(|t| !is_stopword(t));
            let end = folded.iter().rposition(|t| !is_stopword(t));
            match (start, end) {
                (Some(s), Some(e)) if s <= e => Some((s, e + 1)),
                _ => None,
            }
        };
        let Some((s, e)) = bounds else { return false };
        let trimmed = &folded[s..e];
        if trimmed.len() > MAX_KEY_TOKENS {
            return false;
        }
        let key = trimmed.join(" ");
        if key.len() > KEY_MAX_CHARS {
            return false;
        }
        let shown = display[s..e].join(" ");
        self.counter.bump(&key, &shown, DOMAIN_TEXTES, TITLE_BOOST);
        // Decisions cite titles verbatim: a case-law df floor makes them suggestible
        // in case-law mode, below the corpus's own vocabulary.
        self.counter.bump(&key, &shown, DOMAIN_JURIS, MIN_DF_JURIS);
        true
    }

    /// Cuts (df floor per domain, then the top `VOCAB_MAX` by max df) and
    /// serialises the FST. Returns its bytes.
    pub fn build(self) -> Result<Vec<u8>> {
        let mut entries: Vec<(String, Entry)> = self
            .counter
            .map
            .into_iter()
            .filter(|(_, e)| {
                e.df[DOMAIN_JURIS] >= MIN_DF_JURIS || e.df[DOMAIN_TEXTES] >= MIN_DF_TEXTES
            })
            .collect();
        if entries.len() > VOCAB_MAX {
            entries.sort_unstable_by_key(|(_, e)| std::cmp::Reverse(e.df[0].max(e.df[1])));
            entries.truncate(VOCAB_MAX);
        }
        // FST key `folded[\x00display]`: the display suffix is stored only when it
        // differs from the folded form. Sorted on the bytes of the full key.
        let mut keyed: Vec<(Vec<u8>, u64)> = entries
            .into_iter()
            .map(|(folded, e)| {
                let mut key = folded.clone().into_bytes();
                if e.display != folded {
                    key.push(DISPLAY_SEP);
                    key.extend_from_slice(e.display.as_bytes());
                }
                let packed = (u64::from(e.df[DOMAIN_JURIS]) << 32) | u64::from(e.df[DOMAIN_TEXTES]);
                (key, packed)
            })
            .collect();
        keyed.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

        let mut builder = fst::MapBuilder::memory();
        for (key, packed) in &keyed {
            builder
                .insert(key, *packed)
                .map_err(|e| anyhow!("fst insert: {e}"))?;
        }
        let bytes = builder
            .into_inner()
            .map_err(|e| anyhow!("fst build: {e}"))?;
        tracing::info!(keys = keyed.len(), fst_bytes = bytes.len(), "FST built");
        Ok(bytes)
    }
}
