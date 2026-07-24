//! Backfill de rattrapage + résolution des acteurs par décision (ADR
//! 0181/0182). L'écriture nominale est au fil de l'eau
//! (`update_extracted_fields` / ingest, patron `case_citation`) ; le
//! backfill reconstruit la relation depuis les colonnes NER plates de
//! `decisions` — mêmes lignes que le fil de l'eau (spans-évidences, nature,
//! `resolve_key` via `lj_extract::parties`), stampées de
//! l'`extract_version` de la décision source.

use crate::config::Settings;
use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionPartyRow, DecisionRepository};

/// Décisions lues par lot (keyset id).
const READ_BATCH: i64 = 1_000;
/// Décisions écrites par COPY.
const WRITE_BATCH: usize = 2_000;

/// (colonne, qualité, côté) — l'ordre définit `ord` intra-décision, identique
/// aux cellules du fil de l'eau (`lj_ingest::extract`). La colonne
/// `intervenors` est gatée (ADR 0182 §7 : pas d'émission relation en prod
/// tant que P < 85 % au banc).
const COLUMNS: &[(&str, &str, Option<&str>)] = &[
    ("applicant_companies", "party", Some("applicant")),
    ("defendant_companies", "party", Some("defendant")),
    ("applicant_law_firms", "law_firm", Some("applicant")),
    ("defendant_law_firms", "law_firm", Some("defendant")),
    ("applicant_counsel_names", "counsel_name", Some("applicant")),
    ("defendant_counsel_names", "counsel_name", Some("defendant")),
];

/// Backfill intégral : TRUNCATE + COPY (une transaction) depuis les colonnes
/// NER et `full_text` de `decisions`, puis résolution des clés pendantes
/// (règle #7 : rejouable, remplace tout).
pub async fn backfill_parties() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 3).map_err(|e| anyhow!("build_pool: {e}"))?;
    let reader = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let writer = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&writer)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&writer);

    let cols = COLUMNS
        .iter()
        .map(|(c, _, _)| *c)
        .collect::<Vec<_>>()
        .join(", ");
    let non_empty = COLUMNS
        .iter()
        .map(|(c, _, _)| format!("COALESCE(cardinality({c}), 0)"))
        .collect::<Vec<_>>()
        .join(" + ");
    let query = format!(
        "SELECT id, extract_version, full_text, {cols} FROM decisions \
         WHERE id > $1 AND {non_empty} > 0 ORDER BY id LIMIT {READ_BATCH}"
    );

    writer.batch_execute("BEGIN").await?;
    repo.decision_party_clear().await?;
    let mut cursor = 0i64;
    let mut decisions = 0u64;
    let mut rows = 0u64;
    let mut batch: Vec<(i64, i16, Vec<DecisionPartyRow>)> = Vec::with_capacity(WRITE_BATCH);
    loop {
        let fetched = reader.query(&query, &[&cursor]).await?;
        if fetched.is_empty() {
            break;
        }
        let last = fetched.len() < READ_BATCH as usize;
        for row in &fetched {
            let id: i64 = row.get(0);
            cursor = id;
            decisions += 1;
            let version: Option<i16> = row.get(1);
            let full_text: String = row.get(2);
            let lists: Vec<Vec<String>> = (0..COLUMNS.len())
                .map(|i| row.get::<_, Option<Vec<String>>>(i + 3).unwrap_or_default())
                .collect();
            let cells: Vec<lj_extract::parties::Cell<'_>> = COLUMNS
                .iter()
                .zip(&lists)
                .map(|((_, quality, side), values)| (*quality, *side, values.as_slice()))
                .collect();
            let parties: Vec<DecisionPartyRow> =
                lj_extract::parties::actor_rows(&full_text, &cells)
                    .into_iter()
                    .map(|r| DecisionPartyRow {
                        quality: r.quality.to_string(),
                        side: r.side.map(str::to_string),
                        value: r.value,
                        resolve_key: r.resolve_key,
                        nature: r.nature.map(|n| n.as_str().to_string()),
                        barreau: r.barreau,
                        role: r.role.map(str::to_string),
                        char_starts: r.char_starts,
                        char_ends: r.char_ends,
                    })
                    .collect();
            rows += parties.len() as u64;
            batch.push((id, version.unwrap_or(0), parties));
        }
        if batch.len() >= WRITE_BATCH || last {
            repo.decision_party_copy(&batch).await?;
            batch.clear();
        }
        if decisions % 200_000 < READ_BATCH as u64 {
            tracing::info!(decisions, rows, cursor, "parties-backfill : progression");
        }
    }
    if !batch.is_empty() {
        repo.decision_party_copy(&batch).await?;
    }
    writer.batch_execute("COMMIT").await?;
    tracing::info!(decisions, rows, "parties-backfill : chargé, résolution…");
    relink_with(&repo).await
}

/// Résolution seule (post-rechargement mensuel de registre).
pub async fn relink_parties() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    relink_with(&DecisionRepository::new(&conn)).await
}

pub(crate) async fn relink_with(repo: &DecisionRepository<'_>) -> Result<()> {
    let linked = repo.resolve_pending_parties().await?;
    let (total, resolved, siren, rna, cnb, oacc) = repo.decision_party_stats().await?;
    tracing::info!(
        linked,
        total,
        resolved,
        siren,
        rna,
        cnb,
        oacc,
        "parties : résolution"
    );
    println!(
        "decision_party : {total} lignes, {resolved} résolues \
         ({siren} siren:, {rna} rna:, {cnb} cnb:, {oacc} oacc:) — {linked} liées par cette passe."
    );
    // Rafraîchit les compteurs annuaire d'`entity` (ADR 0192/0239) depuis la
    // relation fraîchement résolue : `decision_count` et `annuaire_registre`
    // reflètent l'état des liens de cette passe.
    let annuaire = repo.refresh_annuaire().await?;
    tracing::info!(annuaire, "annuaire : compteurs rafraîchis");
    println!("annuaire : {annuaire} entités avec contentieux.");
    Ok(())
}
