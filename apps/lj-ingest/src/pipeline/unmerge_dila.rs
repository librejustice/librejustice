//! Réparation rétroactive des faux merges **intra-source** `dila-jade` (#47,
//! ADR 0104) — analogue de [`super::unmerge`] (judilibre) pour le fond DILA JADE.
//!
//! La dédup historique (axe `canonical_ref`, ADR 0100) a fusionné des décisions
//! distinctes d'un même fond `dila-jade` partageant la clé → le texte du
//! **perdant** a été détruit (la ligne `decisions` ne garde que l'autorité). Pour
//! le re-créer il faut son XML source : on le relit du **cache local DILA**
//! (tarballs `<cache>/dila/jade/tarballs/*.tar.gz`) — aucun appel réseau.
//!
//! Différence clé avec judilibre : le cache judilibre est rangé par mois
//! (`<jur>/<AAAAMM>.jsonl.gz`, lookup direct) ; les tarballs DILA ne sont **pas
//! indexés par décision**. On fait donc **un seul passage streamé** sur les
//! tarballs (comme [`super::dila`]), en re-matérialisant les seuls membres dont
//! l'ID DILA est une perdante. Le jeu de perdantes est petit (~1 300) → on
//! collecte les `Candidate` matchés en mémoire (passe 1, CPU dans
//! `spawn_blocking`), puis on scinde + matérialise (passe 2, DB async). Un membre
//! peut apparaître dans plusieurs tarballs (stock + incrément) → `done` garantit
//! **une seule** scission par perdante (première occurrence ; évite les décisions
//! squelettes orphelines).
//!
//! Passe 2, per perdante (inverse d'une fusion) : `create_split_decision`
//! (détache la provenance perdante vers une nouvelle décision squelette ;
//! l'autorité reste sur l'origine, son texte y est déjà) puis `drain_batch_in_txn`
//! (`IngestMode::All`, re-chunk + re-embed **vLLM strict**, jamais Cloudflare) —
//! le tout dans une transaction par perdante (échec → rollback, laissée fusionnée).
//!
//! `--dry-run` (défaut CLI) : stream + comptage (cache-hit/miss), **aucune
//! écriture, aucun GPU** — vérifiable sans risque même pendant le cron.
//! `--limit N` : rollout borné (valider à `--limit 2` avant le run complet, comme
//! #46).

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};

use lj_store::repository::DecisionRepository;

use super::batch::drain_batch_in_txn;
use super::dila::{classify_dila, dila_member_id, ClassifiedDila, Fond};
use super::embed::build_vllm_strict;
use super::files::collect_tar_gz;
use super::{generate_public_id, Candidate, IngestCounts, IngestMode};
use crate::config::Settings;

/// Une perdante planifiée : provenance à détacher + amorce de squelette
/// (`canonical_ref`/`juridiction_type` de la décision, écrasés par la ré-ingestion).
struct Plan {
    decision_id: i64,
    source_uid: String,
    canonical_ref: String,
    jur_type: String,
}

#[derive(Default)]
struct Stats {
    losers_total: usize,
    ready: usize,
    cache_miss: usize,
    split: usize,
    failed: usize,
}

/// Défait les faux merges intra-source `dila-jade` (#47, ADR 0104). `execute =
/// false` (dry-run) : stream + comptage, aucune écriture. `limit` : borne le
/// nombre de splits écrits (rollout prudent).
pub async fn unmerge_same_source_dila(execute: bool, limit: Option<usize>) -> Result<()> {
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

    // Re-embed vLLM **strict** (jamais Cloudflare, coût) — seulement en execute.
    let embedder = if execute {
        Some(build_vllm_strict(&settings).await?)
    } else {
        None
    };

    let losers = repo.fetch_same_source_dila_jade_losers().await?;
    let mut stats = Stats {
        losers_total: losers.len(),
        ..Default::default()
    };
    tracing::info!(
        losers = losers.len(),
        execute,
        "unmerge-same-source-dila : perdantes dila-jade à re-créer"
    );

    // Index : ID DILA (sans préfixe `dila-jade/`) → Plan.
    let mut plans: HashMap<String, Plan> = HashMap::new();
    for (decision_id, source_uid, canonical_ref, jur_type) in losers {
        let Some(id) = source_uid.strip_prefix("dila-jade/").map(str::to_string) else {
            continue;
        };
        plans.insert(
            id,
            Plan {
                decision_id,
                source_uid,
                canonical_ref: canonical_ref.unwrap_or_default(),
                jur_type: jur_type.unwrap_or_else(|| "ta".to_string()),
            },
        );
    }

    let tarballs_dir =
        lj_sources::dila::tarballs_dir(&settings.cache_dir(), lj_sources::dila::DilaFond::Jade);
    let mut tarballs = collect_tar_gz(&tarballs_dir)?;
    tarballs.sort();
    if tarballs.is_empty() {
        tracing::warn!(dir = %tarballs_dir.display(), "unmerge-dila : aucun tarball jade — rien à re-matérialiser");
    }

    // Passe 1 (CPU, bloquant) : un seul passage streamé sur les tarballs jade ;
    // collecte les `Candidate` des perdantes (première occurrence par `done`).
    let needed: HashSet<String> = plans.keys().cloned().collect();
    let collected: Vec<(String, Candidate)> =
        tokio::task::spawn_blocking(move || -> Result<Vec<(String, Candidate)>> {
            let mut out: Vec<(String, Candidate)> = Vec::new();
            let mut done: HashSet<String> = HashSet::new();
            for path in &tarballs {
                lj_sources::tar_reader::for_each_member(path, |name, raw| {
                    if raw.is_empty() || !name.to_lowercase().ends_with(".xml") {
                        return Ok(());
                    }
                    let Some(id) = dila_member_id(&name).map(str::to_string) else {
                        return Ok(());
                    };
                    if !needed.contains(&id) || done.contains(&id) {
                        return Ok(());
                    }
                    let repaired = lj_sources::dila::repair_dila(&raw, Fond::Jade.source());
                    match classify_dila(repaired, &raw, Fond::Jade) {
                        // Full ou Analysis : on re-matérialise le `Candidate` inclus
                        // (re-split recovery — le texte de la perdante, intégral ou,
                        // à défaut, son analyse, #33).
                        Ok(Some(ClassifiedDila::Full(cand) | ClassifiedDila::Analysis(cand))) => {
                            done.insert(id.clone());
                            out.push((id, cand));
                        }
                        // Non routé / électoral : laissé fusionné (non fatal).
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(member = %name, error = %e, "unmerge-dila: parse échec");
                        }
                    }
                    Ok(())
                })?;
            }
            Ok(out)
        })
        .await
        .map_err(|e| anyhow!("stream tarballs jade: {e}"))??;

    stats.ready = collected.len();
    stats.cache_miss = plans.len().saturating_sub(collected.len());

    // Passe 2 (DB async) : scinde + matérialise chaque perdante trouvée.
    let mut processed = 0usize;
    for (id, cand) in collected {
        let plan = plans.get(&id).expect("plan présent (id ∈ needed)");
        if !execute {
            continue;
        }
        let embedder = embedder.as_ref().expect("embedder présent en execute");

        conn.batch_execute("BEGIN").await?;
        let res: Result<()> = async {
            let public_id = generate_public_id();
            let new_id = repo
                .create_split_decision(
                    plan.decision_id,
                    std::slice::from_ref(&plan.source_uid),
                    &plan.jur_type,
                    &public_id,
                    &plan.canonical_ref,
                )
                .await?;
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
            tracing::debug!(new_id, dila_id = %id, "unmerge-dila: décision scindée matérialisée");
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
                tracing::warn!(dila_id = %id, error = %e, "unmerge-dila: split échoué, laissé fusionné");
            }
        }

        processed += 1;
        if processed.is_multiple_of(200) {
            tracing::info!(
                processed,
                split = stats.split,
                failed = stats.failed,
                "unmerge-same-source-dila progress"
            );
        }
        if limit.is_some_and(|n| processed >= n) {
            tracing::info!(limit = ?limit, "unmerge-dila: borne --limit atteinte, arrêt");
            break;
        }
    }

    let mode = if execute {
        "EXECUTE (write)"
    } else {
        "DRY-RUN (read-only)"
    };
    println!("\n=== unmerge-same-source-dila [{mode}] (ADR 0104) ===");
    println!(
        "perdantes dila-jade (rn>1)            : {}",
        stats.losers_total
    );
    println!("  re-matérialisables (tarball-hit)    : {}", stats.ready);
    println!(
        "  absentes des tarballs (laissées)    : {}",
        stats.cache_miss
    );
    if execute {
        println!("  décisions scindées créées           : {}", stats.split);
        println!("  échecs (laissées fusionnées)        : {}", stats.failed);
    }
    Ok(())
}
