//! Backfills hors-migration et dédup rétroactive (canonical_ref, fusion
//! cross-source, ECLI, normalisation caractères du référentiel).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_core::decision::Decision;
use lj_parse::FromSourceFields;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Backfill hors-migration de `decisions.canonical_ref` : **matérialise** la
/// colonne (recalcule `canonical_ref` pour les décisions où elle manque).
/// Reconstruit chaque `Decision` depuis `(full_text, source_fields)`, calcule la
/// citation légale via le module PUR `lj_extract::identity`, l'écrit en masse.
/// Keyset par id sur les seules décisions dont `canonical_ref IS NULL` : repris
/// sans recalcul de ce qui est déjà posé. Re-extract CPU parallèle (rayon), une
/// transaction implicite par lot.
///
/// **Ne FUSIONNE rien et ne re-split rien** : la réparation des faux merges est
/// une étape distincte. Ce backfill ne fait que peupler la colonne.
///
/// `force = false` : ne traite que `canonical_ref IS NULL` (peuplement initial).
/// `force = true` : **re-dérive toutes** les décisions (`full_text` présent) —
/// pour faire passer les clés historiques 3-champs `{nom}|{rg}|{date}` en
/// 4-champs `{type}|{location}|{rg}|{date}`. Une clé recalculée `None` laisse
/// l'existante (jamais d'écrasement vers `NULL`).
pub async fn backfill_canonical_ref(force: bool) -> Result<()> {
    let settings = Settings::from_env()?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Backfill de maintenance : on lève la borne API (`build_pool` pose
    // `statement_timeout=30s`). Les scans de frontière du keyset au resume et les
    // agrégats de fusion sont des batches longs assumés, pas des requêtes
    // interactives.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut last_id: i64 = 0;
    let mut updated_total: u64 = 0;
    let mut errors = 0usize;
    const BATCH: i64 = 256;

    loop {
        let ids = if force {
            repo.decision_ids_for_canonical_ref_recompute(last_id, BATCH)
                .await?
        } else {
            repo.decision_ids_for_canonical_ref_backfill(last_id, BATCH)
                .await?
        };
        let Some(&max_batch_id) = ids.last() else {
            break;
        };
        let mut rows: Vec<(i64, String, serde_json::Value, String)> = Vec::new();
        for id in ids {
            if let Some((full_text, source_fields, source_uid)) =
                repo.fetch_reextract_inputs(id).await?
            {
                rows.push((id, full_text, source_fields, source_uid));
            }
        }
        last_id = max_batch_id;
        if rows.is_empty() {
            continue;
        }

        // `None` = pas de clé exploitable (la colonne reste NULL).
        let computed: Vec<Result<(i64, Option<String>)>> = rows
            .into_par_iter()
            .map(|(id, full_text, source_fields, source_uid)| {
                canonical_ref_one(&full_text, &source_fields, &source_uid).map(|key| (id, key))
            })
            .collect();

        let mut ids_w: Vec<i64> = Vec::new();
        let mut keys_w: Vec<String> = Vec::new();
        for item in computed {
            match item {
                Ok((id, Some(key))) => {
                    ids_w.push(id);
                    keys_w.push(key);
                }
                Ok((_, None)) => {}
                Err(e) => {
                    tracing::error!(error = %e, "canonical-ref failed");
                    errors += 1;
                }
            }
        }

        if !ids_w.is_empty() {
            let updated = repo.update_canonical_refs_bulk(&ids_w, &keys_w).await?;
            updated_total += updated;
        }
        tracing::info!(
            updated_total,
            errors,
            last_id,
            "backfill-canonical-ref progress"
        );
    }

    tracing::info!(updated_total, errors, "canonical_ref backfill done");
    Ok(())
}

/// Reconstruit une `Decision` depuis `(full_text, source_fields)` et calcule son
/// `canonical_ref`. Calque [`reextract_one`], mais rend la clé au lieu des
/// `ExtractedFields`. `None` si la décision n'a pas d'identité stable
/// (discriminants manquants) ou si la juridiction n'est pas routée.
fn canonical_ref_one(
    full_text: &str,
    source_fields: &serde_json::Value,
    source_uid: &str,
) -> Result<Option<String>> {
    let decision = Decision::from_source_fields(full_text, source_fields, source_uid);
    // Juridiction hors des 7 ordres routés → pas de clé, jamais une erreur dure.
    if lj_extract::extract::routed(&decision).is_err() {
        return Ok(None);
    }
    Ok(lj_extract::identity::decision_canonical_ref(&decision))
}

/// Fusion rétroactive des **faux splits cross-source** : des décisions distinctes
/// partageant un `canonical_ref` avec des **sources disjointes** sont la même
/// décision vue par plusieurs sources. Pour chaque cluster sûr
/// (`fetch_cross_source_merge_groups`), garde l'autorité (rang max) et fusionne
/// les autres dedans (`merge_into`). **Ne touche jamais** un cluster à source
/// répétée (affaires sérielles same-source). Aucun re-embed : le texte et les
/// chunks du gardien sont conservés, ceux des perdants disparaissent (cascade).
/// Idempotent : relancé, plus aucun cluster à fusionner.
pub async fn merge_cross_source_duplicates() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let groups = repo.fetch_cross_source_merge_groups().await?;
    tracing::info!(
        clusters = groups.len(),
        "merge-cross-source: clusters to merge"
    );

    let mut merged_clusters = 0usize;
    let mut merged_losers = 0usize;
    for group in &groups {
        let (keeper_id, _) = group[0];
        for &(loser_id, _) in &group[1..] {
            repo.merge_into(keeper_id, loser_id).await?;
            merged_losers += 1;
        }
        merged_clusters += 1;
        if merged_clusters.is_multiple_of(100) {
            tracing::info!(
                merged_clusters,
                merged_losers,
                "merge-cross-source progress"
            );
        }
    }

    tracing::info!(merged_clusters, merged_losers, "merge-cross-source done");
    Ok(())
}

/// Absorbs each record holding no ruling into the same-day decision that joins its number,
/// which then carries both, on the days Judilibre fed in the last `days`.
pub async fn absorb_copies(days: i32) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut keepers_of: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for pair in repo.joinder_copy_pairs(days).await? {
        if !lj_extract::joinder::holds_ruling(&pair.copy_text)
            && pair
                .dockets
                .iter()
                .any(|d| lj_extract::joinder::joins(&pair.keeper_text, d))
        {
            keepers_of.entry(pair.copy).or_default().insert(pair.keeper);
        }
    }
    let mut keepers = Vec::new();
    for (copy, named_by) in keepers_of {
        let [keeper] = named_by.iter().copied().collect::<Vec<_>>()[..] else {
            tracing::info!(
                copy,
                ?named_by,
                "absorb-copies: named by several decisions, left apart"
            );
            continue;
        };
        repo.absorb_into(copy, keeper).await?;
        tracing::info!(copy, keeper, "absorb-copies: decision absorbed");
        keepers.push(keeper);
    }
    super::reextract::fuse_written(&repo, &keepers).await?;
    tracing::info!(absorbed = keepers.len(), "absorb-copies done");
    Ok(())
}

/// Backfill batché de la colonne `decisions.ecli` depuis
/// `source_fields->>'ecli'` (champ d'affichage/interop). Boucle keyset sur
/// `backfill_ecli_batch`. Idempotent : n'écrit que les lignes `ecli IS NULL`
/// portant la clé `ecli` ; un ECLI déjà posé n'est jamais écrasé.
pub async fn backfill_ecli() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Backfill de maintenance : on lève la borne API (`build_pool` pose
    // `statement_timeout=30s`). Les scans de frontière du keyset au resume et les
    // agrégats de fusion sont des batches longs assumés, pas des requêtes
    // interactives.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut last_id: i64 = 0;
    let mut updated_total: u64 = 0;
    const BATCH: i64 = 1024;

    while let Some((updated, max_batch_id)) = repo.backfill_ecli_batch(last_id, BATCH).await? {
        last_id = max_batch_id;
        updated_total += updated;
        tracing::info!(updated_total, last_id, "backfill-ecli progress");
    }

    tracing::info!(updated_total, "ECLI backfill done");
    Ok(())
}
