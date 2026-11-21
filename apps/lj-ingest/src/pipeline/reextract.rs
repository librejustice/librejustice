//! Ré-extraction des champs structurés depuis les payloads stockés.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};

use lj_core::decision::Decision;
use lj_parse::FromSourceFields;
use lj_pg::db::Pool;
use lj_store::repository::{Courts, DecisionRepository, ExtractedFields, REEXTRACTABLE_FIELDS};

use crate::config::Settings;

/// Taille d'un lot écrit en une transaction (fetch `id = ANY`, DELETE+INSERT bulk).
const BATCH: usize = 256;

/// Enumeration keyset page, distinct from [`BATCH`]: on `decisions` a page costs its bound.
const SCAN_PAGE: usize = 20_000;

/// Ré-extrait les champs structurés depuis les payloads stockés.
///
/// Pipeline concurrent : un seul scan construit la worklist des ids périmés,
/// partitionnée en `workers` tranches disjointes ; chaque worker (sa propre
/// connexion) fait `fetch → extract (spawn_blocking) → write` en recouvrement.
/// Aucun ré-chunking ni ré-embedding.
pub async fn reextract_fields(
    fields: Option<&[String]>,
    dry_run: bool,
    worklist: Worklist<'_>,
    workers: Option<usize>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let selected = normalize_reextract_fields(fields)?;

    // Fixed, not sized on the machine: a manual pass lands mid-day on a database
    // that is serving traffic. The nightly chain asks for 8 explicitly.
    let workers = workers.map(|w| w.max(1)).unwrap_or(3);
    let pool = lj_pg::db::build_pool(&settings.db_url, workers + 1)
        .map_err(|e| anyhow!("build_pool: {e}"))?;

    // Worklist: one scan (`stale_decision_ids_for_reextract`). A scoped mode
    // (courts, `--citing-ref-uid`) drains its keyset into the same worklist,
    // parallelised the same way.
    let full = matches!(worklist, Worklist::Full);
    let ids = build_worklist(&pool, worklist).await?;
    let total = ids.len();
    tracing::info!(total, workers, "reextract: worklist built");
    if ids.is_empty() {
        return Ok(());
    }

    let started = Instant::now();
    let processed = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    // Compteurs du mode sonde : par champ colonne, lignes divergentes + deux
    // exemples ; par couche satellite, items dont l'extracteur ne produit rien.
    let diffs: Arc<Mutex<DiffReport>> = Arc::new(Mutex::new(DiffReport::default()));

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
    // workers tournent en concurrence coopérative sur une seule task
    // (`try_join_all`, pas `spawn`) : leurs futures ne sont pas `Send`, les params
    // `dyn ToSql` du repo ne l'étant pas. Le premier worker en erreur fait échouer
    // l'ensemble.
    let chunk_size = total.div_ceil(workers);
    let worker_futs: Vec<_> = ids
        .chunks(chunk_size)
        .map(|slice| {
            worker_loop(
                pool.clone(),
                slice.to_vec(),
                selected.clone(),
                dry_run.then(|| diffs.clone()),
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
        "Re-extraction done"
    );

    if dry_run {
        diffs.lock().unwrap().report(total);
        return Ok(());
    }

    // Fin de run : tout est déjà résolu at-write dans les lots (citations côté
    // source, sonde inversée côté cible, parties scopées) — reste l'annuaire, dont
    // les compteurs dérivent des liens fraîchement posés. Run ciblé → recount scopé
    // à la worklist ; `--full` → refresh complet. Le sweep intégral des pendants
    // reste dans `reconcile`.
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let annuaire = if full {
        repo.refresh_annuaire().await?
    } else {
        repo.refresh_annuaire_scoped(&ids).await?
    };
    tracing::info!(annuaire, "directory refreshed at the end of reextract");
    Ok(())
}

/// Agrégat du mode sonde (`--dry-run`) : ce que la passe réécrirait.
#[derive(Default)]
struct DiffReport {
    /// Champ colonne → (lignes divergentes, jusqu'à quatre exemples).
    columns: std::collections::BTreeMap<String, (i64, Vec<String>)>,
    /// Couche satellite → items dont l'extracteur ne produit RIEN, donc que la
    /// passe purgerait.
    satellites: std::collections::BTreeMap<&'static str, usize>,
}

impl DiffReport {
    fn absorb(
        &mut self,
        per_field: Vec<(String, i64, Vec<String>)>,
        selected: &[String],
        items: &[(i64, ExtractedFields)],
    ) {
        for (field, count, samples) in per_field {
            let slot = self.columns.entry(field).or_default();
            slot.0 += count;
            for s in samples {
                if slot.1.len() < 4 {
                    slot.1.push(s);
                }
            }
        }
        let muet =
            |vide: fn(&ExtractedFields) -> bool| items.iter().filter(|(_, e)| vide(e)).count();
        for (layer, muet) in [
            (
                "legal_references",
                muet(|e| e.citation_occurrences.is_none()),
            ),
            ("decision_links", muet(|e| e.decision_links.is_none())),
            ("case_citations", muet(|e| e.case_citations.is_none())),
            ("parties", muet(|e| e.parties.is_none())),
        ] {
            if selected.iter().any(|f| f == layer) {
                *self.satellites.entry(layer).or_default() += muet;
            }
        }
    }

    fn report(&self, total: usize) {
        for (field, (count, samples)) in &self.columns {
            if *count == 0 {
                continue;
            }
            tracing::info!(
                field,
                divergent = count,
                sur = total,
                exemples = ?samples,
                "dry-run: column the pass would rewrite"
            );
        }
        let intactes: Vec<&str> = self
            .columns
            .iter()
            .filter(|(_, (c, _))| *c == 0)
            .map(|(f, _)| f.as_str())
            .collect();
        tracing::info!(?intactes, "dry-run: columns identical to the extractor");
        for (layer, muet) in &self.satellites {
            tracing::info!(
                layer,
                extracteur_muet = muet,
                sur = total,
                "dry-run: satellite layer the pass would purge"
            );
        }
    }
}

/// Construit la worklist d'ids selon le mode. Défaut (version-gated) : un scan.
/// Scopé : draine le keyset correspondant (jeux petits) pour réutiliser le même
/// pipeline parallèle en aval.
/// Which decisions a pass re-extracts.
pub enum Worklist<'a> {
    /// Those whose extraction version is behind.
    Stale,
    Full,
    SeveralProvenances,
    CitingRefUid(&'a str),
    Courts {
        courts: &'a Courts,
        text_query: Option<&'a str>,
    },
}

async fn build_worklist(pool: &Pool, worklist: Worklist<'_>) -> Result<Vec<i64>> {
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // The worklist scan sweeps all of `decisions`; a parallel plan would take the
    // workers the whole cluster shares away from the searches being served.
    conn.batch_execute(
        "SET statement_timeout = 0; \
         SET max_parallel_workers_per_gather = 0",
    )
    .await?;
    let repo = DecisionRepository::new(&conn);
    match worklist {
        Worklist::Stale => Ok(repo.stale_decision_ids_for_reextract().await?),
        Worklist::Full => Ok(repo.all_decision_ids_for_reextract().await?),
        Worklist::SeveralProvenances => Ok(repo.all_decisions_with_several_provenances().await?),
        // Ciblé par texte cité : un seul scan GIN-piloté.
        Worklist::CitingRefUid(uid) => Ok(repo
            .decision_ids_for_reextract_by_citing_ref_uid(uid)
            .await?),
        Worklist::Courts { courts, text_query } => {
            let mut out = Vec::new();
            let mut last_id = 0i64;
            loop {
                let ids = repo
                    .decision_ids_for_reextract_by_juridiction(
                        last_id,
                        SCAN_PAGE as i64,
                        courts,
                        text_query,
                    )
                    .await?;
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
    diffs: Option<Arc<Mutex<DiffReport>>>,
    processed: &AtomicUsize,
    errors: &AtomicUsize,
) -> Result<()> {
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Réglages de session du run bulk (le pool est local à `reextract_fields`) :
    // - `session_replication_role = replica` coupe les triggers RI — les cibles du
    //   COPY `legal_citation` sont lues du MÊME catalogue, dans la même base ;
    // - `synchronous_commit = off` : pas d'attente de flush WAL au COMMIT de chaque
    //   lot, le reextract est idempotent et un crash rejoue le lot.
    conn.batch_execute(
        "SET statement_timeout = 0; \
         SET session_replication_role = replica; \
         SET synchronous_commit = off",
    )
    .await?;
    let repo = DecisionRepository::new(&conn);
    // Contexte d'extraction du run (linker + vocab compilé) — hydraté par le
    // premier worker, partagé par tous (`&'static`, traverse le spawn_blocking).
    let ctx = super::extract_ctx(&conn).await?;

    for batch in slice.chunks(BATCH) {
        let rows = repo.fetch_reextract_inputs_batch(batch).await?;
        if rows.is_empty() {
            continue;
        }

        // CPU (re-parse + re-extract) hors du thread async, parallélisé rayon (pool
        // global partagé entre workers).
        let extracted = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            rows.into_par_iter()
                .map(|(id, full_text, provenances, case_name)| {
                    reextract_one(id, &full_text, &provenances, case_name.as_deref(), ctx)
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
                    tracing::error!(error = %e, "reextract failed");
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

        if let Some(diffs) = &diffs {
            let per_field = repo
                .diff_extracted_fields_bulk(&items, &selected_refs)
                .await?;
            diffs.lock().unwrap().absorb(per_field, &selected, &items);
            processed.fetch_add(items.len(), Ordering::Relaxed);
            continue;
        }

        conn.batch_execute("BEGIN").await?;
        let write = async {
            repo.update_extracted_fields_bulk(&items, Some(&selected_refs))
                .await
                // `anyhow::Error::new` préserve la chaîne `StoreError → DbError`
                // (SQLSTATE + message).
                .map_err(|e| anyhow::Error::new(e).context("update_extracted_fields_bulk"))?;
            let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
            repo.mirror_text_facets(&ids).await?;
            // Sonde inversée : des dockets réécrits peuvent faire de ces
            // décisions la cible de pendants plus anciens.
            if selected.iter().any(|f| f == "docket_numbers") {
                super::attract::attract_pending_for(&repo, &ids).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match write {
            Ok(()) => {
                conn.batch_execute("COMMIT").await?;
                processed.fetch_add(items.len(), Ordering::Relaxed);
            }
            Err(e) => {
                let _ = conn.batch_execute("ROLLBACK").await;
                return Err(e);
            }
        }
    }
    Ok(())
}

/// The decisions just written that hold several provenances take the fusion of
/// all of them, in place of the incoming one's fields.
pub(super) async fn fuse_written(repo: &DecisionRepository<'_>, ids: &[i64]) -> Result<()> {
    let several = repo.decisions_with_several_provenances(ids).await?;
    if several.is_empty() {
        return Ok(());
    }
    let ctx = super::extract_ctx(repo.conn).await?;
    let rows = repo.fetch_reextract_inputs_batch(&several).await?;
    let items = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        rows.into_par_iter()
            .map(|(id, full_text, provenances, case_name)| {
                reextract_one(id, &full_text, &provenances, case_name.as_deref(), ctx)
                    .map(|fields| (id, fields))
            })
            .collect::<Result<Vec<(i64, ExtractedFields)>>>()
    })
    .await
    .map_err(|e| anyhow!("join fuse: {e}"))??;
    repo.update_extracted_fields_bulk(&items, None)
        .await
        .map_err(|e| anyhow::Error::new(e).context("fuse_written"))?;
    Ok(())
}

/// Re-parse + re-extract of every provenance from `(full_text, source_fields)`,
/// fused ([`lj_extract_rows::fuse_provenances`]): the extracted text lives in
/// the base and is never re-parsed. The authority's failure fails the decision;
/// another provenance that does not read is left out, logged.
pub(super) fn reextract_one(
    decision_id: i64,
    full_text: &str,
    provenances: &[(String, serde_json::Value)],
    case_name: Option<&str>,
    ctx: &super::ExtractCtx,
) -> Result<ExtractedFields> {
    let extract = |source_uid: &str, source_fields: &serde_json::Value| {
        let decision = Decision::from_source_fields(full_text, source_fields, source_uid);
        let fields = lj_extract_rows::reread_fields(&decision, source_fields, &ctx.snapshots())
            .map_err(|e| anyhow!("extract id={decision_id} {source_uid}: {e}"))?;
        Ok::<_, anyhow::Error>((decision.jurisdiction_type, fields))
    };
    let ((authority_uid, authority_fields), rest) = provenances
        .split_first()
        .ok_or_else(|| anyhow!("reextract id={decision_id}: no provenance"))?;
    let (jurisdiction_type, authority) = extract(authority_uid, authority_fields)?;
    let others = rest
        .iter()
        .filter_map(|(uid, sf)| match extract(uid, sf) {
            Ok((_, fields)) => Some(fields),
            Err(e) => {
                tracing::warn!(error = %e, "reextract: provenance left out of the fusion");
                None
            }
        })
        .collect();
    // The stored name does not go back into `case_name`: `set_source_case_names`
    // would rewrite it at rank `source`, above whoever elected it. It only feeds
    // the title.
    Ok(lj_extract_rows::with_search_title(
        lj_extract_rows::fuse_provenances(authority, others),
        jurisdiction_type.as_deref(),
        &ctx.jur_labels,
        case_name,
    ))
}

/// Normalise la liste de champs ré-extractibles.
fn normalize_reextract_fields(fields: Option<&[String]>) -> Result<Vec<String>> {
    let Some(fields) = fields.filter(|f| !f.is_empty()) else {
        return Ok(REEXTRACTABLE_FIELDS.iter().map(|s| s.to_string()).collect());
    };
    // Dédup en préservant l'ordre.
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
