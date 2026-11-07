//! Maintenance des citations : recompute des arrays dénormalisés de
//! facettes/filtres depuis `legal_citation`. Les citations elles-mêmes sont
//! écrites liées à l'ingest (linker in-pass) et rejouées par la passe intégrale
//! hebdomadaire (`reextract-fields --full`). Idempotent.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Largeur d'une tranche de `decision_id` pour le recompute des arrays.
const EDGE_ID_SPAN: i64 = 50_000;

/// Recompute des arrays dénormalisés depuis `legal_citation`. Filet
/// hebdomadaire post-passe intégrale et réparation après changement de la source
/// des arrays ; en régime normal `replace_citations(_bulk)` les tient à jour dans
/// la même transaction. Lots autocommit ; la garde IS DISTINCT FROM ne réécrit
/// que les décisions/chunks dont le token change. Rapporte la dérive corrigée,
/// `tracing::warn` pour Grafana. Le grain dit laquelle. Au grain décision : les
/// familles « et suivants » se lisent du TOC en vigueur aujourd'hui, donc un
/// article qui entre dans un code change la famille de ses voisins — c'est le
/// symptôme d'un array anachronique (une décision n'a pu citer que le code de sa
/// date), pas un régime sain. Au grain chunk sans dérive de décision, c'est un
/// écrivain de chunks qui fuit. Idempotent.
pub async fn resync_arrays() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);

    let Some((lo, hi)) = repo.decision_id_bounds().await? else {
        println!("resync-legal-arrays: no decision.");
        return Ok(());
    };
    let (mut drift_decisions, mut drift_chunks) = (0i64, 0i64);
    let mut cursor = lo;
    while cursor <= hi {
        let next = cursor.saturating_add(EDGE_ID_SPAN);
        let (d, c) = repo.resync_legal_arrays_range(cursor, next).await?;
        drift_decisions += d;
        drift_chunks += c;
        if d > 0 || c > 0 {
            // Coordonnées de la dérive : la tranche d'ids suffit à retrouver
            // les décisions fautives (re-run ciblé + logs de la période).
            tracing::warn!(
                id_lo = cursor,
                id_hi = next,
                drift_decisions = d,
                drift_chunks = c,
                "resync arrays : dérive corrigée sur la tranche — au grain \
                 décision, suit le TOC du jour (familles « et suivants », array \
                 anachronique) ou une migration ; au grain chunk seul, fuite \
                 d'écrivain"
            );
        }
        tracing::info!(
            done_id = next.min(hi + 1),
            max_id = hi,
            "resync arrays (progression)"
        );
        cursor = next;
    }
    println!(
        "resync-legal-arrays : terminé ({lo}..={hi}), dérive corrigée : \
         {drift_decisions} décision(s), {drift_chunks} chunk(s)."
    );
    // Même dérivée de `legal_citation` que les arrays : les décomptes de
    // décisions citantes (plan des pages citantes + IDF co-citations) se recomptent
    // dans la même passe hebdomadaire.
    let terms = repo.rebuild_citing_decision_counts().await?;
    println!("citing_decision_counts: {terms} term(s) upserted.");
    Ok(())
}
