//! Réparation rétroactive des faux merges **intra-source** judilibre (ADR 0104).
//!
//! La dédup historique (axe `canonical_ref`, ADR 0100) a fusionné des décisions
//! **distinctes** d'une même source judilibre partageant `numbers`+cour+date (lot
//! d'audience, RG réutilisé, arrêt sur déférée). Le `full_text` du **perdant** a
//! été détruit (la ligne `decisions` n'en garde qu'un, l'autorité) → pour le
//! re-créer il faut son corps de texte. On le relit du **cache local** judilibre
//! (miroir complet, rangé `<jur>/<AAAAMM-du-decision_date>.jsonl.gz`) — **aucun
//! appel API** (les anciens object_id supersédés n'y manquent pas ; le cache
//! `compact` retire en revanche les tombstonés → une provenance supprimée n'y est
//! plus, donc jamais re-matérialisée : RGPD respecté gratuitement).
//!
//! Passe (per perdante, ADR 0104 / inverse d'une fusion) :
//! 1. `create_split_decision([loser_uid])` : détache la provenance perdante vers
//!    une **nouvelle** décision squelette (l'autorité `rn=1` reste sur l'origine,
//!    son texte y est déjà — pas touchée) ;
//! 2. relit le payload du perdant dans son fichier cache et `drain_batch_in_txn`
//!    (`IngestMode::All`, re-chunk + re-embed **vLLM strict**) le matérialise sur
//!    la décision scindée. Split + matérialisation **dans une seule transaction**.
//!
//! `--dry-run` (défaut CLI) : compte seulement (cache-hit/miss), **aucune
//! écriture, aucun GPU**. `--limit N` : rollout borné.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use lj_store::repository::DecisionRepository;

use super::batch::drain_batch_in_txn;
use super::embed::build_vllm_strict;
use super::files::read_jsonl_gz_lines;
use super::prepare::classify_judilibre;
use super::resplit::{provenance_canonical_ref, provenance_jurisdiction_type};
use super::{generate_public_id, IngestCounts, IngestMode};
use crate::config::Settings;

/// `YYYYMM` d'une `decision_date` ISO (`YYYY-MM-DD`) ; `None` si malformée
/// (même découpage que `yyyymm_of` du downloader — bucket d'archivage cache).
fn yyyymm(decision_date: &str) -> Option<String> {
    let b = decision_date.as_bytes();
    if b.len() >= 7 && b[4] == b'-' {
        Some(format!("{}{}", &decision_date[0..4], &decision_date[5..7]))
    } else {
        None
    }
}

/// Une perdante planifiée : provenance à détacher + sa localisation cache.
struct Plan {
    decision_id: i64,
    source_uid: String,
    object_id: String,
    canonical_ref: String,
    jur_type: String,
}

#[derive(Default)]
struct Stats {
    losers_total: usize,
    skipped_no_jur_date: usize,
    skipped_no_canonical: usize,
    cache_miss: usize,
    ready: usize,
    split: usize,
    failed: usize,
}

/// Relit un fichier cache `.jsonl.gz` une fois et renvoie les payloads des
/// `needed` (par object_id top-level). Cache append-only → **last-in-file wins**
/// (version la plus récente). Fichier absent ⇒ map vide.
fn load_payloads(path: &Path, needed: &HashSet<&str>) -> Result<HashMap<String, Vec<u8>>> {
    let mut out: HashMap<String, Vec<u8>> = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let lines = read_jsonl_gz_lines(path).with_context(|| format!("lecture cache {path:?}"))?;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
            if needed.contains(id) {
                out.insert(id.to_string(), line);
            }
        }
    }
    Ok(out)
}

/// Défait les faux merges intra-source judilibre (ADR 0104). `execute = false`
/// (dry-run) : plan + comptage, aucune écriture. `limit` : borne le nombre de
/// splits écrits (rollout prudent).
pub async fn unmerge_same_source(execute: bool, limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let cache_root = settings.cache_dir().join("judilibre");

    // Re-embed vLLM **strict** (jamais Cloudflare, coût) — seulement en execute.
    let embedder = if execute {
        Some(build_vllm_strict(&settings).await?)
    } else {
        None
    };

    let losers = repo.fetch_same_source_judilibre_losers().await?;
    let mut stats = Stats {
        losers_total: losers.len(),
        ..Default::default()
    };
    tracing::info!(
        losers = losers.len(),
        execute,
        "unmerge-same-source : perdantes judilibre à re-créer"
    );

    // Regroupe par fichier cache (jur, yyyymm) → lecture une seule fois par fichier.
    let mut by_file: BTreeMap<(String, String), Vec<Plan>> = BTreeMap::new();
    for (decision_id, source_uid, sf) in losers {
        let Some(object_id) = source_uid.strip_prefix("judilibre/").map(str::to_string) else {
            continue;
        };
        let jur = sf
            .get("jurisdiction")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let date = sf
            .get("decision_date")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ym = match (jur.is_empty(), yyyymm(date)) {
            (false, Some(ym)) => ym,
            _ => {
                stats.skipped_no_jur_date += 1;
                continue;
            }
        };
        let Some(canonical_ref) = provenance_canonical_ref(&sf, &source_uid) else {
            stats.skipped_no_canonical += 1;
            continue;
        };
        let jur_type =
            provenance_jurisdiction_type(&sf, &source_uid).unwrap_or_else(|| "tj".to_string());
        by_file
            .entry((jur.to_string(), ym))
            .or_default()
            .push(Plan {
                decision_id,
                source_uid,
                object_id,
                canonical_ref,
                jur_type,
            });
    }

    let mut processed = 0usize;
    'outer: for ((jur, ym), plans) in &by_file {
        let path = cache_root.join(jur).join(format!("{ym}.jsonl.gz"));
        let needed: HashSet<&str> = plans.iter().map(|p| p.object_id.as_str()).collect();
        let payloads = load_payloads(&path, &needed)?;

        for p in plans {
            let Some(line) = payloads.get(&p.object_id) else {
                stats.cache_miss += 1;
                tracing::warn!(object_id = %p.object_id, path = ?path, "unmerge: payload absent du cache, laissé fusionné");
                continue;
            };
            stats.ready += 1;
            if !execute {
                continue;
            }
            let embedder = embedder.as_ref().expect("embedder présent en execute");

            conn.batch_execute("BEGIN").await?;
            let res: Result<()> = async {
                let public_id = generate_public_id();
                let new_id = repo
                    .create_split_decision(
                        p.decision_id,
                        std::slice::from_ref(&p.source_uid),
                        &p.jur_type,
                        &public_id,
                        &p.canonical_ref,
                    )
                    .await?;
                let cand = classify_judilibre(line.clone())?
                    .ok_or_else(|| anyhow!("payload cache non reconnu: {}", p.object_id))?;
                let mut counts = IngestCounts::default();
                drain_batch_in_txn(
                    &conn,
                    Some(embedder),
                    vec![cand],
                    true,
                    IngestMode::All,
                    &mut counts,
                )
                .await?;
                tracing::debug!(new_id, object_id = %p.object_id, "unmerge: décision scindée matérialisée");
                Ok(())
            }
            .await;

            match res {
                Ok(()) => {
                    conn.batch_execute("COMMIT").await?;
                    stats.split += 1;
                }
                Err(e) => {
                    let _ = conn.batch_execute("ROLLBACK").await;
                    stats.failed += 1;
                    tracing::warn!(object_id = %p.object_id, error = %e, "unmerge: split échoué, laissé fusionné");
                }
            }

            processed += 1;
            if processed.is_multiple_of(500) {
                tracing::info!(
                    processed,
                    split = stats.split,
                    failed = stats.failed,
                    cache_miss = stats.cache_miss,
                    "unmerge-same-source progress"
                );
            }
            if limit.is_some_and(|n| processed >= n) {
                tracing::info!(limit = ?limit, "unmerge: borne --limit atteinte, arrêt");
                break 'outer;
            }
        }
    }

    let mode = if execute {
        "EXECUTE (write)"
    } else {
        "DRY-RUN (read-only)"
    };
    println!("\n=== unmerge-same-source [{mode}] (ADR 0104) ===");
    println!(
        "perdantes judilibre (rn>1)            : {}",
        stats.losers_total
    );
    println!(
        "  sans jur/date (skip)                : {}",
        stats.skipped_no_jur_date
    );
    println!(
        "  sans canonical_ref (skip)           : {}",
        stats.skipped_no_canonical
    );
    println!(
        "  absentes du cache (laissées)        : {}",
        stats.cache_miss
    );
    println!("  re-matérialisables (cache-hit)      : {}", stats.ready);
    if execute {
        println!("  décisions scindées créées           : {}", stats.split);
        println!("  échecs (laissées fusionnées)        : {}", stats.failed);
    }
    Ok(())
}
