//! Ré-extraction des champs structurés depuis les payloads stockés (ADR 0085).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};

use lj_core::decision::Decision;
use lj_store::db::Pool;
use lj_store::repository::{DecisionRepository, ExtractedFields, REEXTRACTABLE_FIELDS};

use crate::config::Settings;

/// Taille d'un lot écrit en une transaction (fetch `id = ANY`, DELETE+INSERT bulk).
const BATCH: usize = 256;

/// Ré-extrait les champs structurés depuis les payloads stockés.
///
/// Pipeline **concurrent** (ADR 0065 §perf) : un seul scan construit la worklist
/// des ids périmés, partitionnée en `workers` tranches disjointes ; chaque worker
/// (sa propre connexion) fait `fetch → extract (spawn_blocking) → write` en
/// **recouvrement**. L'ancienne boucle sérielle mono-connexion laissait 11/12
/// cœurs au repos (CPU et DB en ping-pong, + keyset filtré dégradant). Aucun
/// ré-chunking ni ré-embedding.
pub async fn reextract_fields(
    fields: Option<&[String]>,
    overwrite: bool,
    full: bool,
    jurisdiction_types: Option<&[String]>,
    citing_ref_uid: Option<&str>,
    workers: Option<usize>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let selected = normalize_reextract_fields(fields)?;

    // Concurrence DB = nombre de workers (1 connexion chacun). Override explicite
    // `--workers N` (prod : `--workers 2` reste doux pendant que le site sert) ;
    // défaut auto : ~cœurs − 4, borné [2, 8] (laisse de la marge à Postgres
    // co-localisé pour un run ad-hoc agressif).
    let workers = workers.map(|w| w.max(1)).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(4))
            .unwrap_or(4)
            .clamp(2, 8)
    });
    let pool = lj_store::db::build_pool(&settings.db_url, workers + 1)
        .map_err(|e| anyhow!("build_pool: {e}"))?;

    // Worklist : un seul scan (cf. `stale_decision_ids_for_reextract`) plutôt que
    // des keyset filtrés répétés. Modes scopés (`--juridiction-type`/
    // `--citing-ref-uid`) : on draine leur keyset (jeux petits) pour bâtir la
    // même worklist, puis on parallélise pareil.
    let ids = build_worklist(&pool, full, jurisdiction_types, citing_ref_uid).await?;
    let total = ids.len();
    tracing::info!(total, workers, "reextract : worklist construite");
    if ids.is_empty() {
        return Ok(());
    }

    let started = Instant::now();
    let processed = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));

    // Ticker de progression : lit les compteurs atomiques toutes les 5 s.
    let ticker = {
        let processed = processed.clone();
        let errors = errors.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            tick.tick().await; // immédiat — on saute le premier
            loop {
                tick.tick().await;
                let done = processed.load(Ordering::Relaxed);
                let rate = done as f64 / started.elapsed().as_secs_f64().max(1e-3);
                let eta_s = ((total - done.min(total)) as f64 / rate.max(1e-3)) as i64;
                tracing::info!(
                    processed = done,
                    total,
                    errors = errors.load(Ordering::Relaxed),
                    rate_per_s = rate as i64,
                    eta_s,
                    "reextract progress"
                );
            }
        })
    };

    // Tranches disjointes contiguës (équilibrées) → un worker par tranche. Les
    // workers tournent en **concurrence coopérative sur une seule task**
    // (`try_join_all`, pas `spawn`) : leurs futures ne sont pas `Send` (les params
    // `dyn ToSql` du repo ne le sont pas), mais ça suffit — les allers-retours DB
    // se recouvrent (N connexions en vol), et le CPU part en `spawn_blocking`
    // (multi-thread). Le premier worker en erreur fait échouer l'ensemble.
    let chunk_size = total.div_ceil(workers);
    let worker_futs: Vec<_> = ids
        .chunks(chunk_size)
        .map(|slice| {
            worker_loop(
                pool.clone(),
                slice.to_vec(),
                selected.clone(),
                overwrite,
                &processed,
                &errors,
            )
        })
        .collect();
    let result = futures::future::try_join_all(worker_futs).await;
    ticker.abort();
    result.map_err(|e| e.context("reextract worker"))?;

    tracing::info!(
        processed = processed.load(Ordering::Relaxed),
        errors = errors.load(Ordering::Relaxed),
        "Re-extraction terminée"
    );

    // Réconciliation des liens pendants en fin de run (ADR 0240) : les décisions
    // (re)liées pendant la passe peuvent être la CIBLE de liens pendants plus
    // anciens (chronologie, citations décision→décision et texte→décision), et
    // les lignes `decision_party` réécrites repartent pendantes. Un seul helper
    // partagé avec `db reconcile`.
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    super::reconcile::reconcile_pending(&repo).await
}

/// Construit la worklist d'ids selon le mode. Défaut (version-gated) : un scan.
/// Scopé : draine le keyset correspondant (jeux petits) pour réutiliser le même
/// pipeline parallèle en aval.
async fn build_worklist(
    pool: &Pool,
    full: bool,
    jurisdiction_types: Option<&[String]>,
    citing_ref_uid: Option<&str>,
) -> Result<Vec<i64>> {
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);
    if full {
        return Ok(repo.all_decision_ids_for_reextract().await?);
    }
    match (citing_ref_uid, jurisdiction_types) {
        (None, None) => Ok(repo.stale_decision_ids_for_reextract().await?),
        (uid, jts) => {
            let mut out = Vec::new();
            let mut last_id = 0i64;
            loop {
                let ids = match (uid, jts) {
                    (Some(uid), _) => {
                        repo.decision_ids_for_reextract_by_citing_ref_uid(
                            last_id,
                            BATCH as i64,
                            uid,
                        )
                        .await?
                    }
                    (None, Some(jts)) => {
                        repo.decision_ids_for_reextract_by_juridiction(last_id, BATCH as i64, jts)
                            .await?
                    }
                    (None, None) => unreachable!(),
                };
                let Some(&max) = ids.last() else { break };
                last_id = max;
                out.extend(ids);
            }
            Ok(out)
        }
    }
}

/// Boucle d'un worker : traite sa tranche d'ids par lots de [`BATCH`], chacun
/// `fetch → extract (CPU sur le pool blocking) → write transaction`, sur sa
/// propre connexion. La passe CPU tourne en `spawn_blocking` (ne bloque pas le
/// runtime) ; `workers` lots s'extraient donc en parallèle sur autant de cœurs.
async fn worker_loop(
    pool: Pool,
    slice: Vec<i64>,
    selected: Vec<String>,
    overwrite: bool,
    processed: &AtomicUsize,
    errors: &AtomicUsize,
) -> Result<()> {
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Réglages de session du run bulk (le pool est local à `reextract_fields`,
    // jamais recyclé ailleurs) :
    // - `session_replication_role = replica` coupe les triggers RI — la
    //   validation FK per-row du COPY `legal_citation` pesait ~40 % du lot au
    //   profil (1,6 M `FOR KEY SHARE`/min) pour des cibles lues du MÊME
    //   catalogue dans la même base (snapshot lien) ;
    // - `synchronous_commit = off` : pas d'attente de flush WAL au COMMIT de
    //   chaque lot — le reextract est idempotent, un crash rejoue le lot.
    conn.batch_execute(
        "SET statement_timeout = 0; \
         SET session_replication_role = replica; \
         SET synchronous_commit = off",
    )
    .await?;
    let repo = DecisionRepository::new(&conn);
    // Contexte d'extraction du run (linker + vocab compilé, ADR 0145/0156) —
    // hydraté par le premier worker, partagé par tous (`&'static`, traverse le
    // spawn_blocking).
    let ctx = super::extract_ctx(&conn).await?;

    for batch in slice.chunks(BATCH) {
        let rows: Vec<(i64, String, serde_json::Value, String)> =
            repo.fetch_reextract_inputs_batch(batch).await?;
        if rows.is_empty() {
            continue;
        }

        // CPU (re-parse + re-extract) hors du thread async, parallélisé rayon
        // (pool global partagé entre workers) : le lot s'extrait en ~1/N du
        // temps sériel, la connexion du worker repart plus vite en écriture.
        let extracted = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            rows.into_par_iter()
                .map(|(id, full_text, source_fields, source_uid)| {
                    reextract_one(id, &full_text, &source_fields, &source_uid, ctx)
                        .map(|fields| (id, fields))
                })
                .collect::<Vec<Result<(i64, ExtractedFields)>>>()
        })
        .await
        .map_err(|e| anyhow!("join extract: {e}"))?;

        let mut items: Vec<(i64, ExtractedFields)> = Vec::with_capacity(extracted.len());
        let mut batch_errors = 0usize;
        for item in extracted {
            match item {
                Ok(pair) => items.push(pair),
                Err(e) => {
                    tracing::error!(error = %e, "reextract échec");
                    batch_errors += 1;
                }
            }
        }
        if batch_errors > 0 {
            errors.fetch_add(batch_errors, Ordering::Relaxed);
        }
        if items.is_empty() {
            continue;
        }

        conn.batch_execute("BEGIN").await?;
        match repo
            .update_extracted_fields_bulk(&items, Some(&selected_refs), overwrite)
            .await
        {
            Ok(()) => {
                conn.batch_execute("COMMIT").await?;
                processed.fetch_add(items.len(), Ordering::Relaxed);
            }
            Err(e) => {
                let _ = conn.batch_execute("ROLLBACK").await;
                // `anyhow::Error::new` préserve la chaîne `StoreError → DbError`
                // (SQLSTATE + message), qu'un `{e}` aplatirait.
                return Err(anyhow::Error::new(e).context("update_extracted_fields_bulk"));
            }
        }
    }
    Ok(())
}

/// Re-parse + re-extract depuis la reconstruction `(full_text, source_fields)`
/// (ADR 0085) — chemin linéaire unique, quel que soit le format du payload
/// d'origine (le texte extrait vit en base, jamais re-parsé). `member_name`
/// reconstruit par `from_source_fields` (provenance) ; `classify_uid`/fond/numéro
/// dérivés du `source_uid`.
fn reextract_one(
    decision_id: i64,
    full_text: &str,
    source_fields: &serde_json::Value,
    source_uid: &str,
    ctx: &super::ExtractCtx,
) -> Result<ExtractedFields> {
    let decision = Decision::from_source_fields(full_text, source_fields, source_uid);
    lj_ingest::extract::extracted_fields(
        &decision,
        &ctx.link,
        &ctx.vocab,
        &ctx.chrono,
        &ctx.jur_labels,
    )
    .map_err(|e| anyhow!("extract id={decision_id}: {e}"))
}

/// Normalise la liste de champs ré-extractibles (port de `_normalize_reextract_fields`).
fn normalize_reextract_fields(fields: Option<&[String]>) -> Result<Vec<String>> {
    let Some(fields) = fields.filter(|f| !f.is_empty()) else {
        return Ok(REEXTRACTABLE_FIELDS.iter().map(|s| s.to_string()).collect());
    };
    // dict.fromkeys : dédup en préservant l'ordre.
    let mut seen = std::collections::HashSet::new();
    let mut selected: Vec<String> = Vec::new();
    for f in fields {
        let f = f.trim();
        if f.is_empty() || !seen.insert(f.to_string()) {
            continue;
        }
        selected.push(f.to_string());
    }
    let unknown: Vec<&String> = selected
        .iter()
        .filter(|f| !REEXTRACTABLE_FIELDS.contains(&f.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(anyhow!(
            "fields inconnus : {unknown:?}. Choix : {}",
            REEXTRACTABLE_FIELDS.join(", ")
        ));
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_fields_defaults_to_all() {
        let all = normalize_reextract_fields(None).unwrap();
        assert_eq!(all.len(), REEXTRACTABLE_FIELDS.len());
        assert!(all.contains(&"legal_references".to_string()));
    }

    #[test]
    fn normalize_fields_rejects_unknown() {
        let err = normalize_reextract_fields(Some(&["bogus".to_string()]));
        assert!(err.is_err());
    }

    #[test]
    fn normalize_fields_dedups_preserving_order() {
        let fields = vec![
            "date_lecture".to_string(),
            "date_lecture".to_string(),
            "solution_uid".to_string(),
        ];
        let out = normalize_reextract_fields(Some(&fields)).unwrap();
        assert_eq!(
            out,
            vec!["date_lecture".to_string(), "solution_uid".to_string()]
        );
    }
}
