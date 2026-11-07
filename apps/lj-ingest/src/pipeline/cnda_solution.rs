//! Backfill de `solution_uid` dans les `source_fields` CNDA.
//!
//! `parse_cnda` lit le sens du dispositif — connaissance CNDA que l'extracteur
//! générique n'a pas — et pose `solution_uid` en `source_fields` pour qu'il
//! franchisse la décomposition `source_payload ⟷ (full_text, source_fields)`. Les
//! provenances écrites avant portent des `source_fields` sans la clé ; on la
//! recalcule ici depuis la reconstruction, sans re-télécharger le PDF.

use anyhow::{anyhow, Result};

use lj_parse::cnda_solution_uid_from_source_fields;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Aligne `solution_uid` des `source_fields` CNDA sur ce que le parser rend
/// aujourd'hui — pose la clé manquante, corrige la divergente, retire celle qu'une
/// règle resserrée ne justifie plus. Dry-run par défaut (`apply = false`).
/// Rejouable : on ne touche que ce qui change.
pub async fn backfill_cnda_solution(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let candidates = repo.cnda_sources_for_solution().await?;
    let total = candidates.len();

    // Réconciliation, pas remplissage : la clé doit dire ce que le parser dit
    // AUJOURD'HUI. Une règle qui se resserre doit pouvoir la retirer.
    let (mut set_uids, mut set_values, mut remove_uids) = (Vec::new(), Vec::new(), Vec::new());
    for (source_uid, full_text, source_fields) in candidates {
        let attendu = cnda_solution_uid_from_source_fields(&full_text, &source_fields, &source_uid);
        let pose = source_fields
            .get("solution_uid")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if attendu == pose {
            continue;
        }
        match attendu {
            Some(uid) => {
                set_uids.push(source_uid);
                set_values.push(uid);
            }
            // Dispositif ambigu (révision, exclusion) ou introuvable :
            // `cnda_solution_uid` ne replie pas.
            None => remove_uids.push(source_uid),
        }
    }

    if !apply {
        tracing::info!(
            total,
            a_poser = set_uids.len(),
            a_retirer = remove_uids.len(),
            "backfill-cnda-solution: dry-run (--apply to write)"
        );
        return Ok(());
    }

    let (posees, retirees) = repo
        .align_source_fields_solution_uid(&set_uids, &set_values, &remove_uids)
        .await?;
    tracing::info!(total, posees, retirees, "backfill-cnda-solution: done");
    Ok(())
}
