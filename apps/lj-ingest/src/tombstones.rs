//! Propagation des suppressions Judilibre vers la base (ADR 0087).
//!
//! Le downloader (`lj-sources`) écrit une tombstone par décision supprimée côté
//! Judilibre (action `deleted` de `/transactionalhistory`) dans
//! `tombstones.jsonl`, puis `compact_archives` la retire du cache JSONL local.
//! Mais compacter le cache ne dit jamais à Postgres de supprimer la ligne : sans
//! ce module, une décision retirée par la Cour de cassation reste servie
//! indéfiniment (le seul autre chemin de hard-delete, `reverses`, ne couvre que
//! l'opendata).
//!
//! On lit `tombstones.jsonl`, on reconstruit le `source_uid` (`judilibre/{id}`,
//! cf. `parse_judilibre`) et on hard-delete via `DecisionRepository::delete`
//! (cascade chunks/texte/refs, même chemin que `reverses`). Idempotent : un id
//! déjà absent renvoie `false` — on re-traite le fichier entier à chaque ingest,
//! comme `reverses` re-traite ses CSV.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use lj_store::repository::DecisionRepository;

/// Bilan d'une purge des tombstones (analogue à `reverses::PurgeSummary`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneSummary {
    /// ids uniques présents dans `tombstones.jsonl`.
    pub seen: usize,
    /// lignes effectivement supprimées de la base.
    pub deleted: usize,
    /// tombstones déjà purgées (ou jamais ingérées).
    pub already_absent: usize,
}

/// Extrait les ids uniques d'un `tombstones.jsonl` (une ligne JSON par
/// suppression, champ `id`). Ignore les lignes vides ; un id absent/vide est
/// sauté. Trié + dédupliqué (`BTreeSet`).
pub fn parse_tombstone_ids(text: &str) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).context("tombstones: ligne JSON invalide")?;
        if let Some(id) = value
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            ids.insert(id.to_string());
        }
    }
    Ok(ids)
}

/// Hard-delete des décisions Judilibre tombstoned. `data_dir` est le dossier
/// source Judilibre (qui contient `tombstones.jsonl`) ; fichier absent = no-op.
pub async fn prune_tombstones(
    data_dir: &Path,
    repo: &DecisionRepository<'_>,
) -> Result<PruneSummary> {
    let path = data_dir.join("tombstones.jsonl");
    if !path.exists() {
        return Ok(PruneSummary::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("tombstones: read {path:?}"))?;
    let ids = parse_tombstone_ids(&text)?;

    let mut summary = PruneSummary {
        seen: ids.len(),
        ..Default::default()
    };
    for id in &ids {
        let source_uid = format!("judilibre/{id}");
        if repo
            .delete(&source_uid)
            .await
            .with_context(|| format!("tombstones: delete {source_uid}"))?
        {
            summary.deleted += 1;
        } else {
            summary.already_absent += 1;
        }
    }
    tracing::info!(?summary, "judilibre_tombstones_prune");
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : dédup des ids, ordre stable, lignes vides ignorées.
    #[test]
    fn parse_dedups_and_skips_blanks() {
        let text = concat!(
            "{\"id\":\"b\",\"deleted_at\":\"2026-06-11T03:01:07+00:00\"}\n",
            "\n",
            "{\"id\":\"a\"}\n",
            "{\"id\":\"b\"}\n",
        );
        let ids = parse_tombstone_ids(text).unwrap();
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    // Spec : id absent ou vide → sauté (pas d'entrée fantôme).
    #[test]
    fn parse_skips_missing_or_empty_id() {
        let text = "{\"deleted_at\":\"x\"}\n{\"id\":\"\"}\n{\"id\":\"ok\"}\n";
        let ids = parse_tombstone_ids(text).unwrap();
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), vec!["ok".to_string()]);
    }

    // Spec : ligne JSON invalide = erreur franche (pas de silence).
    #[test]
    fn parse_rejects_invalid_json() {
        assert!(parse_tombstone_ids("not json\n").is_err());
    }
}
