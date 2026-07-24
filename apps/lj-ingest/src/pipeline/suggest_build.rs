//! Build du vocabulaire d'autocomplétion (ADR 0216) : n-grammes 1-5 comptés en
//! doc-frequency sur un échantillon des décisions (`id % 10`), full scan des
//! articles, titres `legal_text` injectés avec boost — sérialisés en
//! `fst::Map` (clé `folded[\x00display]`, valeur `u64 = df_juris << 32 |
//! df_textes`) et déposés en blob dans `suggest_index`.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_core::body_tok::{is_stopword, tokenize, tokenize_lower};
use lj_core::suggest::{harvest_ngrams, DISPLAY_SEP, MAX_KEY_TOKENS};
use lj_store::repository::{DecisionRepository, SUGGEST_FST_KEY};

use crate::config::Settings;

/// Échantillon décisions : une sur `SAMPLE_MODULO` (fréquences relatives —
/// scanner 100 % n'apporte rien au top du vocabulaire).
const SAMPLE_MODULO: i64 = 10;
const BATCH: i64 = 500;
/// Plafond de la map de comptage ; au-delà, prune lossy (seuil croissant).
/// Dimensionné pour que le seuil de prune reste sous les planchers de df :
/// à 6 M, l'espace des 4-5-grammes le faisait grimper à ~50 et mangeait
/// toute la queue rare du vocabulaire (~5 Go de comptage à 24 M, la VM en a
/// 125).
const COUNT_CAP: usize = 24_000_000;
/// df plancher (mesuré sur l'échantillon côté juris) pour entrer au vocabulaire.
const MIN_DF_JURIS: u32 = 5;
const MIN_DF_TEXTES: u32 = 3;
/// Taille max du vocabulaire final (coupe par df décroissant).
const VOCAB_MAX: usize = 3_000_000;
/// Boost df côté textes des titres `legal_text` (la suggestion idéale du
/// mode) — assez pour émerger en contexte, sans écraser les vrais mots du
/// corpus dans le repli unigramme.
const TITLE_BOOST: u32 = 2_000;
/// Garde-fou de longueur de clé (titres interminables).
const KEY_MAX_CHARS: usize = 64;

const DOMAIN_JURIS: usize = 0;
const DOMAIN_TEXTES: usize = 1;

/// Compte d'un n-gramme plié : df par domaine + forme d'affichage majoritaire
/// (vote Boyer-Moore : O(1) mémoire, converge vers la graphie dominante —
/// « congés payés » l'emporte sur un « conges payes » mal saisi).
struct Entry {
    df: [u32; 2],
    display: String,
    display_votes: i32,
}

/// Comptage df borné en RAM : au plafond, purge des entrées sous un seuil
/// croissant (lossy counting simplifié — le suggest ne consomme que le top).
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
                "comptage n-grammes : prune lossy"
            );
        }
    }
}

/// N-grammes distincts d'un document, en paire (plié, affiché) — df =
/// présence, pas occurrences (le boilerplate répété dans un même doc ne
/// compte qu'une fois).
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

/// Commande `build-suggest` : comptage → coupe → FST → blob `suggest_index`.
pub async fn build_suggest() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut counter = Counter::new();

    // 1. Décisions échantillonnées (df côté jurisprudence).
    let mut last_id = 0i64;
    let mut docs = 0u64;
    loop {
        let batch = repo
            .suggest_decision_texts_batch(last_id, SAMPLE_MODULO, BATCH)
            .await?;
        let Some(&(max_id, _)) = batch.last() else {
            break;
        };
        last_id = max_id;
        docs += batch.len() as u64;
        let sets: Vec<HashSet<(String, String)>> =
            batch.par_iter().map(|(_, text)| doc_ngrams(text)).collect();
        for set in sets {
            for (folded, display) in set {
                counter.bump(&folded, &display, DOMAIN_JURIS, 1);
            }
        }
        if docs.is_multiple_of(20_000) {
            tracing::info!(
                docs,
                entries = counter.map.len(),
                "build-suggest : décisions"
            );
        }
    }
    tracing::info!(docs, entries = counter.map.len(), "décisions comptées");

    // 2. Articles, full scan (df côté textes).
    let mut last_id = 0i64;
    let mut arts = 0u64;
    loop {
        let batch = repo.suggest_article_texts_batch(last_id, BATCH).await?;
        let Some(&(max_id, _)) = batch.last() else {
            break;
        };
        last_id = max_id;
        arts += batch.len() as u64;
        let sets: Vec<HashSet<(String, String)>> =
            batch.par_iter().map(|(_, text)| doc_ngrams(text)).collect();
        for set in sets {
            for (folded, display) in set {
                counter.bump(&folded, &display, DOMAIN_TEXTES, 1);
            }
        }
        if arts.is_multiple_of(100_000) {
            tracing::info!(
                arts,
                entries = counter.map.len(),
                "build-suggest : articles"
            );
        }
    }
    tracing::info!(arts, entries = counter.map.len(), "articles comptés");

    // 3. Titres legal_text injectés entiers (bords non-stopword,
    //    ≤ MAX_KEY_TOKENS) avec boost côté textes.
    let mut titles_kept = 0u64;
    for title in repo.suggest_text_titles().await? {
        let folded = tokenize(&title);
        let display = tokenize_lower(&title);
        let bounds = {
            let start = folded.iter().position(|t| !is_stopword(t));
            let end = folded.iter().rposition(|t| !is_stopword(t));
            match (start, end) {
                (Some(s), Some(e)) if s <= e => Some((s, e + 1)),
                _ => None,
            }
        };
        let Some((s, e)) = bounds else { continue };
        let trimmed = &folded[s..e];
        if trimmed.len() > MAX_KEY_TOKENS {
            continue;
        }
        let key = trimmed.join(" ");
        if key.len() > KEY_MAX_CHARS {
            continue;
        }
        let shown = display[s..e].join(" ");
        counter.bump(&key, &shown, DOMAIN_TEXTES, TITLE_BOOST);
        // Cités tels quels dans les décisions : un df plancher côté
        // jurisprudence les rend suggérables en mode juris, sous le
        // vocabulaire réel du corpus (qui garde le dessus au ranking).
        counter.bump(&key, &shown, DOMAIN_JURIS, MIN_DF_JURIS);
        titles_kept += 1;
    }
    tracing::info!(titles_kept, "titres injectés");

    // 4. Coupe : df plancher par domaine, puis top VOCAB_MAX par df max.
    let mut entries: Vec<(String, Entry)> = counter
        .map
        .into_iter()
        .filter(|(_, e)| e.df[DOMAIN_JURIS] >= MIN_DF_JURIS || e.df[DOMAIN_TEXTES] >= MIN_DF_TEXTES)
        .collect();
    if entries.len() > VOCAB_MAX {
        entries.sort_unstable_by_key(|(_, e)| std::cmp::Reverse(e.df[0].max(e.df[1])));
        entries.truncate(VOCAB_MAX);
    }
    // Clé FST : `folded[\x00display]` — le suffixe d'affichage n'est stocké que
    // s'il diffère de la forme pliée. Tri sur les octets de la clé complète.
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

    // 5. FST (clés triées, valeur packée) → blob.
    let mut builder = fst::MapBuilder::memory();
    for (key, packed) in &keyed {
        builder
            .insert(key, *packed)
            .map_err(|e| anyhow!("fst insert: {e}"))?;
    }
    let bytes = builder
        .into_inner()
        .map_err(|e| anyhow!("fst build: {e}"))?;
    tracing::info!(keys = keyed.len(), fst_bytes = bytes.len(), "FST construit");
    repo.upsert_suggest_fst(SUGGEST_FST_KEY, &bytes).await?;
    tracing::info!("build-suggest terminé");
    Ok(())
}
