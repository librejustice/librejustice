//! Bodily-injury quantum in the database: the ground truth at
//! `PC_GOLD_VERSION`, read from the JSON records versioned in LFS, two
//! automatic readings at `PC_READING_VERSION`, and the deterministic pass at
//! `PC_EXTRACT_VERSION`, which never replaces either.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use lj_quantum::ancrage::ancrer;
use lj_quantum::dispositif::{extract, Mode};
use lj_quantum::observation::{observations, Observation};
use lj_quantum::publication::{on_table, publish, Written};
use lj_quantum::{spreadsheet::compute, Quantum};
use lj_store::repository::{DecisionRepository, QuantumObservationRow, QuantumWriteItem};

use crate::config::Settings;

fn cents(x: Option<f64>) -> Option<i64> {
    x.map(|v| (v * 100.0).round() as i64)
}

fn row(o: &Observation) -> QuantumObservationRow {
    QuantumObservationRow {
        victime_ord: o.victime_ord,
        poste_ord: o.poste_ord,
        composante_ord: o.composante_ord.unwrap_or(-1),
        tranche_ord: o.tranche_ord.unwrap_or(-1),
        poste: o.poste.clone(),
        axe: o.axe.clone(),
        hors_table: o.hors_table.clone(),
        montant_cents: cents(o.montant),
        valeur_unitaire_cents: cents(o.valeur_unitaire),
        quantite: o.quantite,
        unite: o.unite.clone(),
        nature_aide: o.nature_aide.clone(),
        mode_emploi: o.mode_emploi.clone(),
        specialisee: o.specialisee,
        nuit: o.nuit,
        taux_pct: o.taux_pct,
        cotation: o.cotation,
        categorie: o.categorie.clone(),
        categorie_manque: o.categorie_manque.clone(),
        deces: o.deces,
        comparabilite: o.comparabilite.as_ref().map(|c| {
            serde_json::to_value(c).expect("Comparabilite ne porte que des chaînes et des taux")
        }),
        taux_dfp_pct: o.taux_dfp_pct,
        age: o.age.map(|a| a.round() as i16),
        sexe: o.sexe.clone(),
        role: o.role.clone(),
        lien: o.lien.clone(),
        preuve: o.preuve.clone(),
        equations: o.equations,
        origine: o.origine.clone(),
        cite: o.cite.clone(),
        indemnites_acquises_cents: cents(o.indemnites_acquises),
    }
}

/// The record judged by the spreadsheet, ready to write with the rows it carries.
fn item(decision_id: i64, q: &Quantum, rows: &[Observation]) -> Result<QuantumWriteItem> {
    let verdict = compute(q).verdict();
    Ok(QuantumWriteItem {
        decision_id,
        est_liquidation: q.est_liquidation,
        atteste: Some(verdict.holds()),
        raison_non_atteste: verdict
            .conflicts
            .first()
            .map(|c| format!("{} : {:.2} contre {:.2}", c.cell, c.first, c.second)),
        cases_confirmees: Some(verdict.confirmed as i32),
        quantum: serde_json::to_value(q)?,
        observations: rows.iter().map(row).collect(),
    })
}

/// The file name is the `source_uid` with its slash doubled into underscores.
fn source_uid(chemin: &Path) -> Option<String> {
    Some(chemin.file_stem()?.to_string_lossy().replace("__", "/"))
}

/// Reads the records of the directory, judges them, and writes them at
/// `PC_GOLD_VERSION`. A uid the fonds does not carry is reported and skipped.
pub async fn load_quantum_gold(dir: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let mut chemins: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("lecture de {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    chemins.sort();

    let mut lus: Vec<(String, Quantum)> = Vec::new();
    for chemin in &chemins {
        let brut = std::fs::read_to_string(chemin)
            .with_context(|| format!("lecture de {}", chemin.display()))?;
        let v: serde_json::Value = serde_json::from_str(&brut)
            .with_context(|| format!("json invalide dans {}", chemin.display()))?;
        let Some(volet) = v.get("quantum") else {
            continue;
        };
        let uid = source_uid(chemin)
            .ok_or_else(|| anyhow!("nom de fichier sans uid : {}", chemin.display()))?;
        let q: Quantum = serde_json::from_value(volet.clone())
            .with_context(|| format!("volet quantum illisible dans {}", chemin.display()))?;
        lus.push((uid, q));
    }

    let uids: Vec<String> = lus.iter().map(|(u, _)| u.clone()).collect();
    let ids: HashMap<String, i64> = DecisionRepository::new(&conn)
        .resolve_source_uids(&uids)
        .await?
        .into_iter()
        .collect();

    let mut items = Vec::new();
    let mut absents = Vec::new();
    for (uid, q) in &lus {
        let Some(&decision_id) = ids.get(uid) else {
            absents.push(uid.clone());
            continue;
        };
        items.push(item(decision_id, q, &observations(q, &compute(q)))?);
    }

    conn.batch_execute("BEGIN").await?;
    let ecrites = DecisionRepository::new(&conn)
        .replace_quantum_bulk(&items, lj_versions::PC_GOLD_VERSION)
        .await?;
    conn.batch_execute("COMMIT").await?;

    let tenus = items.iter().filter(|i| i.atteste == Some(true)).count();
    tracing::info!(
        records = items.len(),
        tenus,
        observations = ecrites,
        version = lj_versions::PC_GOLD_VERSION,
        "quantum ground truth loaded"
    );
    if !absents.is_empty() {
        tracing::warn!(
            absents = absents.len(),
            uids = absents.join(", "),
            "uids without a decision in the corpus"
        );
    }
    Ok(())
}

/// The deterministic sure-mode pass over the bodily-injury fonds, written at
/// `PC_EXTRACT_VERSION`, each value anchored on the passage that writes it. A
/// decision that no longer yields anything loses its automatic reading; a
/// manual revision is never touched.
pub async fn extract_quantum(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut ids = repo.quantum_fonds_ids().await?;
    if let Some(l) = limit {
        ids.truncate(l);
    }
    let fonds = ids.len();
    let (mut liquidations, mut ecrites, mut effacees) = (0usize, 0usize, 0u64);
    for lot in ids.chunks(500) {
        let textes = repo.decision_texts(lot).await?;
        let mut items = Vec::new();
        let mut vides = Vec::new();
        for (id, texte) in &textes {
            let scan = lj_extract::scan::scan(texte);
            let mut q = extract(&scan, Mode::Sure).quantum;
            if q.victimes.is_empty() {
                vides.push(*id);
            } else {
                q.ancres = ancrer(&q, texte);
                items.push(item(*id, &q, &observations(&q, &compute(&q)))?);
            }
        }
        conn.batch_execute("BEGIN").await?;
        ecrites += repo
            .replace_quantum_bulk(&items, lj_versions::PC_EXTRACT_VERSION)
            .await?;
        effacees += repo
            .delete_quantum_at_most(&vides, lj_versions::PC_EXTRACT_VERSION)
            .await?;
        conn.batch_execute("COMMIT").await?;
        liquidations += items.len();
    }
    tracing::info!(
        fonds,
        liquidations,
        observations = ecrites,
        effacees,
        version = lj_versions::PC_EXTRACT_VERSION,
        "quantum extracted over the corpus"
    );
    Ok(())
}

/// One `{source_uid, quantum}` line per decision, as a reading campaign writes it.
fn readings(path: &Path) -> Result<HashMap<String, Quantum>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l)
                .with_context(|| format!("invalid json in {}", path.display()))?;
            let uid = v["source_uid"]
                .as_str()
                .ok_or_else(|| anyhow!("line without source_uid in {}", path.display()))?
                .to_string();
            let q: Quantum = serde_json::from_value(v["quantum"].clone())
                .with_context(|| format!("unreadable quantum for {uid}"))?;
            Ok((uid, q))
        })
        .collect()
}

/// Two readings of the same decisions, written at `PC_READING_VERSION`: the
/// first one's record, and its rows under the publication guards, each row
/// they refuse carrying its reason. The texts are the corpus's, so a tariff is
/// checked against what the decision writes. Idempotent.
pub async fn load_quantum_readings(first: &Path, second: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (a, b) = (readings(first)?, readings(second)?);
    let mut uids: Vec<String> = a.keys().filter(|u| b.contains_key(*u)).cloned().collect();
    uids.sort();
    let ids: HashMap<i64, String> = repo
        .resolve_source_uids(&uids)
        .await?
        .into_iter()
        .map(|(uid, id)| (id, uid))
        .collect();
    let mut lot_ids: Vec<i64> = ids.keys().copied().collect();
    lot_ids.sort_unstable();

    let (mut records, mut kept, mut refused, mut rows_written) = (0usize, 0usize, 0usize, 0usize);
    for lot in lot_ids.chunks(500) {
        let mut items = Vec::new();
        for (id, text) in repo.decision_texts(lot).await? {
            let uid = &ids[&id];
            let rows = publish(&a[uid], &b[uid], &Written::new(&text));
            kept += rows.iter().filter(|o| on_table(o)).count();
            refused += rows
                .iter()
                .filter(|o| o.axe.is_some() && o.hors_table.is_some())
                .count();
            items.push(item(id, &a[uid], &rows)?);
        }
        conn.batch_execute("BEGIN").await?;
        rows_written += repo
            .replace_quantum_bulk(&items, lj_versions::PC_READING_VERSION)
            .await?;
        conn.batch_execute("COMMIT").await?;
        records += items.len();
    }
    tracing::info!(
        read_by_both = uids.len(),
        unresolved = uids.len() - ids.len(),
        records,
        kept,
        refused,
        observations = rows_written,
        version = lj_versions::PC_READING_VERSION,
        "quantum readings loaded"
    );
    Ok(())
}
