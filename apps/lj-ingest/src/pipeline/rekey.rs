//! Rebasage one-shot des clés d'article vers la clé publique slug
//! (`lj_core::article_key`, ADR 0209). Orchestration : mapping Rust →
//! fusion des doublons PK (variantes typographiques du même article) →
//! rebasage colonne par colonne → dédup TOC → resync des composites
//! décision/chunks depuis `legal_citation`. Idempotent (#7) :
//! `article_key` est un point fixe, une deuxième passe ne mappe rien.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Rebasage one-shot des `num_key` vers la clé d'IDENTITÉ (ADR 0236) :
/// `legal_article.num_key` (+ `legal_link.owner_num_key` qui la suit) et
/// `legal_toc_edge.child_num_key`, recalculés depuis le `num`/`label` source
/// par `lj_core::article_key::identity_key`. Une ligne dont la cible PK est
/// déjà occupée (variante typographique du même article, ou doublon interne
/// au lot) est sautée et loggée — elle garde son ancienne clé. Idempotent :
/// `identity_key` est un point fixe, une deuxième passe ne mappe rien.
pub async fn rekey_identity_keys() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    const PAGE: i64 = 50_000;
    let (mut scanned, mut updated, mut skipped, mut links) = (0u64, 0u64, 0u64, 0u64);
    let mut after_id = 0i64;
    loop {
        let page = repo.article_identity_page(after_id, PAGE).await?;
        let Some(last) = page.last() else { break };
        after_id = last.0;
        scanned += page.len() as u64;
        // Lignes à rebaser, dédupées sur la cible PK à l'intérieur du lot
        // (deux variantes distinctes pliant sur la même clé + même date : la
        // première gagne, la seconde est sautée comme un conflit existant).
        let mut targets = std::collections::HashSet::new();
        let (mut rows, mut new_keys) = (Vec::new(), Vec::new());
        for row in page {
            let new_key = lj_core::article_key::identity_key(&row.2);
            if new_key == row.3 {
                continue;
            }
            if targets.insert((row.1.clone(), new_key.clone(), row.4.clone())) {
                rows.push(row);
                new_keys.push(new_key);
            } else {
                skipped += 1;
                tracing::warn!(
                    id = row.0,
                    num = %row.2,
                    "doublon de cible dans le lot — clé conservée"
                );
            }
        }
        if !rows.is_empty() {
            let (u, s, l) = repo.apply_article_identity_keys(&rows, &new_keys).await?;
            updated += u;
            skipped += s;
            links += l;
        }
        if scanned % 500_000 == 0 {
            tracing::info!(
                scanned,
                updated,
                skipped,
                links,
                "rekey identité (articles)"
            );
        }
    }
    tracing::info!(
        scanned,
        updated,
        skipped,
        links,
        "rekey identité : articles terminés"
    );

    let (mut toc_scanned, mut toc_updated) = (0u64, 0u64);
    let mut after: Option<(String, i32)> = None;
    loop {
        let page = repo
            .toc_identity_page(after.as_ref().map(|(o, s)| (o.as_str(), *s)), PAGE)
            .await?;
        let Some(last) = page.last() else { break };
        after = Some((last.0.clone(), last.1));
        toc_scanned += page.len() as u64;
        let (mut owners, mut seqs, mut keys) = (Vec::new(), Vec::new(), Vec::new());
        for (owner, seq, label, old_key) in &page {
            let new_key = lj_core::article_key::identity_key(label);
            if new_key != *old_key {
                owners.push(owner.as_str());
                seqs.push(*seq);
                keys.push(new_key);
            }
        }
        if !owners.is_empty() {
            toc_updated += repo.apply_toc_identity_keys(&owners, &seqs, &keys).await?;
        }
    }
    tracing::info!(
        toc_scanned,
        toc_updated,
        "rekey identité : arêtes TOC terminées"
    );
    println!(
        "rekey-identity : {updated} articles rebasés ({skipped} sautés, {links} liens suivis), \
         {toc_updated} arêtes TOC."
    );
    Ok(())
}

pub async fn rekey_article_keys() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Backfill de maintenance : passes longues assumées (scans + updates de
    // masse), hors borne interactive.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let keys = repo.distinct_article_keys().await?;
    let pairs: Vec<(String, String)> = keys
        .into_iter()
        .filter_map(|old| {
            let new = lj_core::article_key::article_key(&old);
            (new != old).then_some((old, new))
        })
        .collect();
    tracing::info!(a_rebaser = pairs.len(), "mapping des clés d'article");
    if pairs.is_empty() {
        tracing::info!("clés déjà en alphabet public — rien à faire");
        return Ok(());
    }
    repo.create_rekey_map(&pairs).await?;

    let (articles_fusionnes, liens_fusionnes) = repo.rekey_merge_duplicate_articles().await?;
    tracing::info!(articles_fusionnes, liens_fusionnes, "doublons PK fusionnés");

    for (table, column) in [
        ("legal_article", "num_key"),
        ("legal_toc_edge", "child_num_key"),
        ("legal_link", "owner_num_key"),
        ("legal_link", "target_num_key"),
        ("text_case_citation", "owner_num_key"),
    ] {
        let n = repo.rekey_column(table, column).await?;
        tracing::info!(table, column, lignes = n, "colonne rebasée");
    }
    // Les clés des blobs `legal_citation.spans` (ADR 0247) se rebasent par
    // réécriture jsonb dédiée.
    let n = repo.rekey_citation_spans().await?;
    tracing::info!(decisions = n, "spans legal_citation rebasés");

    let toc_dedup = repo.rekey_dedup_toc_edges().await?;
    tracing::info!(toc_dedup, "arêtes TOC dédupliquées");

    // Composites décision/chunks : resync intégral depuis legal_citation —
    // la garde IS DISTINCT FROM des fonctions sync ne réécrit que le changé.
    let Some((_, max_id)) = repo.decision_id_bounds().await? else {
        return Ok(());
    };
    const BATCH: i64 = 20_000;
    let (mut d_total, mut c_total) = (0i64, 0i64);
    let mut lo = 0i64;
    while lo <= max_id {
        let (d, c) = repo.resync_legal_arrays_range(lo, lo + BATCH).await?;
        d_total += d;
        c_total += c;
        lo += BATCH;
        if (lo / BATCH) % 25 == 0 {
            tracing::info!(lo, max_id, d_total, c_total, "resync composites");
        }
    }
    tracing::info!(d_total, c_total, "composites resynchronisés");
    Ok(())
}
