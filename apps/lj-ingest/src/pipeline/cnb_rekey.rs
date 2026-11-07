//! Re-key one-shot du namespace `cnb:` vers l'identité CNBF.
//!
//! L'ancien schéma d'uid (`cnb:{barreau}:{nom}-{prenom}`) fait mourir
//! l'identité avec le barreau, et laisse des liens danglants (sorties et
//! changements de barreau) et des entités bidons (pieds de fichier du CSV). Ce
//! one-shot ponte l'existant vers `cnb:{avCnbfCode}` (snapshot courant),
//! désactive ce qui n'a pas de pont, réhydrate les danglants sans pont — puis le
//! reload upsert (`registries avocats`) prend le relais. Dry-run par défaut,
//! `--apply` pour écrire. Idempotent : replanifié depuis l'état courant à chaque
//! run.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use lj_store::repository::{CnbEntityState, CnbEntityUpdate, DecisionRepository, EntityWriteItem};

use super::registries::{fetch_avocat_rows, AvocatRow, CnbRows};
use crate::config::Settings;

/// Plan de re-key, calculé à froid (pur, testé) puis appliqué en une
/// transaction.
#[derive(Default)]
struct RekeyPlan {
    /// Entités pieds de fichier → DELETE (liens dé-liés d'abord).
    bogus: Vec<String>,
    /// Legacy → cnbf : rename de l'entité + remap des liens.
    renames: Vec<(String, String)>,
    /// Legacy → cible déjà prise (doublon d'une même personne) : remap des
    /// liens + DELETE du doublon.
    merges: Vec<(String, String)>,
    /// Legacy sans pont (sorti du registre) : inactif, span clos, conservé.
    deactivate: Vec<CnbEntityUpdate>,
    /// Uid danglant → cnbf : remap des liens seulement.
    dangling_remaps: Vec<(String, String)>,
    /// Uid danglant sans pont : entité inactive recréée depuis l'uid.
    rehydrate: Vec<EntityWriteItem>,
}

/// Segments d'un uid legacy `cnb:{barreau}:{nom}-{prenom}` —
/// `None` si l'uid est déjà au format CNBF (ou étranger).
fn legacy_segments(uid: &str) -> Option<(&str, &str)> {
    uid.strip_prefix("cnb:")?.split_once(':')
}

/// Entité pied de fichier : dénomination `{prenom} {nom}` = `{compteur} {date}`.
fn is_bogus(denomination: &str) -> bool {
    let mut it = denomination.split_whitespace();
    match (it.next(), it.next(), it.next()) {
        (Some(a), Some(b), None) => {
            a.bytes().all(|c| c.is_ascii_digit())
                && b.len() == 8
                && b.bytes().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Ponts uid legacy → code CNBF depuis le snapshot courant. Deux passes :
/// exacte `(barreau, nom-prenom)` puis, pour les non-pontés (changements de
/// barreau), `nom-prenom` seul — chaque clé n'est un pont que si elle est
/// UNIQUE dans le snapshot (l'ambiguïté s'abstient, comme partout).
struct Bridge {
    exact: HashMap<(String, String), Option<String>>,
    by_name: HashMap<String, Option<String>>,
}

impl Bridge {
    fn build(rows: &[AvocatRow]) -> Self {
        let mut exact: HashMap<(String, String), Option<String>> = HashMap::new();
        let mut by_name: HashMap<String, Option<String>> = HashMap::new();
        for r in rows {
            let np = r.nomprenom_slug();
            exact
                .entry((r.barreau_slug.clone(), np.clone()))
                .and_modify(|v| *v = None)
                .or_insert(Some(r.uid.clone()));
            by_name
                .entry(np)
                .and_modify(|v| *v = None)
                .or_insert(Some(r.uid.clone()));
        }
        Self { exact, by_name }
    }

    fn map(&self, barreau: &str, nomprenom: &str) -> Option<&str> {
        if let Some(Some(c)) = self
            .exact
            .get(&(barreau.to_string(), nomprenom.to_string()))
        {
            return Some(c);
        }
        match self.by_name.get(nomprenom) {
            Some(Some(c)) => Some(c),
            _ => None,
        }
    }
}

fn rekey_plan(
    rows: &[AvocatRow],
    existing: &[CnbEntityState],
    dangling: &[String],
    snapshot: chrono::NaiveDate,
) -> RekeyPlan {
    let bridge = Bridge::build(rows);
    let existing_uids: std::collections::HashSet<&str> =
        existing.iter().map(|s| s.uid.as_str()).collect();
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut plan = RekeyPlan::default();

    for state in existing {
        let Some((barreau, nomprenom)) = legacy_segments(&state.uid) else {
            continue; // déjà au format CNBF (run précédent partiel)
        };
        if is_bogus(&state.denomination) {
            plan.bogus.push(state.uid.clone());
            continue;
        }
        match bridge.map(barreau, nomprenom) {
            Some(target) => {
                let target = target.to_string();
                if existing_uids.contains(target.as_str()) || !claimed.insert(target.clone()) {
                    plan.merges.push((state.uid.clone(), target));
                } else {
                    plan.renames.push((state.uid.clone(), target));
                }
            }
            None => {
                // Sorti du registre avant l'apparition du code : inactif,
                // span clos au snapshot (actif jusqu'à une date inconnue ≤
                // snapshot).
                let mut activity = state
                    .activity
                    .clone()
                    .unwrap_or_else(|| serde_json::json!([{"src": "cnb"}]));
                if let Some(spans) = activity.as_array_mut() {
                    for s in spans {
                        if s.get("au").is_none() {
                            s["au"] = serde_json::Value::String(snapshot.to_string());
                        }
                    }
                }
                plan.deactivate.push(CnbEntityUpdate {
                    uid: state.uid.clone(),
                    active: false,
                    activity: Some(activity),
                    barreau: state.barreau.clone(),
                    denomination: state.denomination.clone(),
                    forme: state.forme.clone(),
                    surname_key: state.surname_key.clone(),
                });
            }
        }
    }

    for uid in dangling {
        let Some((barreau, nomprenom)) = legacy_segments(uid) else {
            continue;
        };
        match bridge.map(barreau, nomprenom) {
            Some(target) => plan.dangling_remaps.push((uid.clone(), target.to_string())),
            None => plan.rehydrate.push(EntityWriteItem {
                uid: uid.clone(),
                nature: "physique",
                denomination: nomprenom.replace('-', " "),
                sigle: None,
                forme: Some("avocat".to_string()),
                active: false,
                surname_key: None,
                category: "avocats",
                ape: None,
                barreau: Some(barreau.to_string()),
                alt_denominations: Vec::new(),
                activity: Some(serde_json::json!([
                    {"au": snapshot.to_string(), "src": "cnb"}
                ])),
            }),
        }
    }
    plan
}

/// Commande `rekey-cnb` : calcule le plan depuis le snapshot courant et
/// l'état en base ; l'applique en une transaction si `--apply`.
pub async fn rekey_cnb(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("registries");
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (CnbRows::Coded(rows), snapshot) = fetch_avocat_rows(&dir).await? else {
        anyhow::bail!("current cnb snapshot carries no avCnbfCode");
    };
    let existing = repo.entity_cnb_states().await?;
    let dangling = repo.dangling_cnb_actor_uids().await?;
    let plan = rekey_plan(&rows, &existing, &dangling, snapshot);

    println!(
        "rekey-cnb (snapshot {snapshot}) : {} legacy en base, {} danglants\n\
         → renames {} | merges {} | deactivate {} | bogus {} | \
         dangling remaps {} | rehydrate {}",
        existing
            .iter()
            .filter(|s| legacy_segments(&s.uid).is_some())
            .count(),
        dangling.len(),
        plan.renames.len(),
        plan.merges.len(),
        plan.deactivate.len(),
        plan.bogus.len(),
        plan.dangling_remaps.len(),
        plan.rehydrate.len(),
    );
    for (old, new) in plan.merges.iter().take(5) {
        println!("  merge ex.: {old} → {new}");
    }
    if !apply {
        println!("dry-run, re-run with --apply to write.");
        return Ok(());
    }

    conn.batch_execute("BEGIN; SET LOCAL statement_timeout = 0")
        .await?;
    let unlinked = repo.actor_unlink_uids(&plan.bogus).await?;
    let bogus_deleted = repo.entity_delete_bulk(&plan.bogus).await?;
    let renamed = repo.entity_rename_bulk(&plan.renames).await?;
    // Renames purs : les compteurs suivent la ligne d'entité, pas de recount.
    let (rename_links, _) = repo.actor_remap_entities(&plan.renames, false).await?;
    // Fusions + danglants : les cibles GAGNENT des liens → recount scopé.
    let merge_pairs: Vec<(String, String)> = plan
        .merges
        .iter()
        .chain(plan.dangling_remaps.iter())
        .cloned()
        .collect();
    let (merge_links, touched_decisions) = repo.actor_remap_entities(&merge_pairs, true).await?;
    let merged_deleted = repo
        .entity_delete_bulk(&plan.merges.iter().map(|m| m.0.clone()).collect::<Vec<_>>())
        .await?;
    let deactivated = repo.entity_cnb_update_bulk(&plan.deactivate).await?;
    for batch in plan.rehydrate.chunks(20_000) {
        repo.entity_copy(batch).await?;
    }
    let recounted = repo.refresh_annuaire_scoped(&touched_decisions).await?;
    conn.batch_execute("COMMIT").await?;

    tracing::info!(
        renamed,
        rename_links,
        merge_links,
        merged_deleted,
        deactivated,
        bogus_deleted,
        unlinked,
        rehydrated = plan.rehydrate.len(),
        recounted,
        "rekey-cnb applied"
    );
    println!(
        "appliqué : {renamed} renames ({rename_links} liens), {merge_links} liens fusionnés, \
         {merged_deleted} doublons supprimés, {deactivated} désactivés, \
         {bogus_deleted} bidons purgés ({unlinked} liens dé-liés), \
         {} réhydratés, {recounted} compteurs recomptés.",
        plan.rehydrate.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn row(cnbf: &str, barreau_slug: &str, nom: &str, prenom: &str) -> AvocatRow {
        AvocatRow {
            uid: format!("cnb:{cnbf}"),
            barreau: barreau_slug.to_uppercase(),
            barreau_slug: barreau_slug.to_string(),
            nom: nom.to_string(),
            prenom: prenom.to_string(),
            bar_entree: None,
            siren: None,
            date_entree: None,
        }
    }

    fn state(uid: &str, denomination: &str) -> CnbEntityState {
        CnbEntityState {
            uid: uid.to_string(),
            active: true,
            activity: None,
            barreau: None,
            denomination: denomination.to_string(),
            surname_key: None,
            forme: None,
        }
    }

    #[test]
    fn rekey_plan_spec() {
        let snapshot = d("2026-07-17");
        let rows = [
            row("900001", "agen", "BERTIN", "Louise"),
            row("111111", "lyon", "MARTIN", "Sophie"),
            // Homonymes nationaux : nom-prenom non unique → pas de pont nom seul.
            row("222222", "paris", "DUPONT", "Jean"),
            row("333333", "nice", "DUPONT", "Jean"),
        ];

        // Pont exact (barreau, nom-prenom) → rename.
        let existing = [state("cnb:agen:bertin-louise", "Louise Bertin")];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert_eq!(
            plan.renames,
            vec![(
                "cnb:agen:bertin-louise".to_string(),
                "cnb:900001".to_string()
            )]
        );

        // Changement de barreau : pont par nom-prenom unique national.
        let existing = [state("cnb:marseille:martin-sophie", "Sophie Martin")];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert_eq!(
            plan.renames,
            vec![(
                "cnb:marseille:martin-sophie".to_string(),
                "cnb:111111".to_string()
            )]
        );

        // Homonyme national ambigu, aucun barreau ne matche → désactivé.
        let existing = [state("cnb:albi:dupont-jean", "Jean Dupont")];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert!(plan.renames.is_empty());
        assert_eq!(plan.deactivate.len(), 1);
        assert!(!plan.deactivate[0].active);
        assert_eq!(
            plan.deactivate[0].activity,
            Some(serde_json::json!([{"au": "2026-07-17", "src": "cnb"}]))
        );

        // Homonyme ambigu MAIS barreau exact → pont.
        let existing = [state("cnb:paris:dupont-jean", "Jean Dupont")];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert_eq!(
            plan.renames,
            vec![(
                "cnb:paris:dupont-jean".to_string(),
                "cnb:222222".to_string()
            )]
        );

        // Doublon d'une même personne sous deux barreaux : un rename (exact),
        // l'autre fusionne.
        let existing = [
            state("cnb:agen:bertin-louise", "Louise Bertin"),
            state("cnb:albi:bertin-louise", "Louise Bertin"),
        ];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(plan.merges.len(), 1);
        assert_eq!(plan.merges[0].1, "cnb:900001");

        // Pied de fichier → purge, jamais ponté.
        let existing = [state("cnb:agen:20260101-100", "100 20260101")];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert_eq!(plan.bogus, vec!["cnb:agen:20260101-100"]);
        assert!(plan.renames.is_empty() && plan.deactivate.is_empty());

        // Danglants : ponté → remap des liens ; sans pont → réhydraté inactif.
        let dangling = [
            "cnb:marseille:martin-sophie".to_string(),
            "cnb:albi:disparu-paul".to_string(),
        ];
        let plan = rekey_plan(&rows, &[], &dangling, snapshot);
        assert_eq!(
            plan.dangling_remaps,
            vec![(
                "cnb:marseille:martin-sophie".to_string(),
                "cnb:111111".to_string()
            )]
        );
        assert_eq!(plan.rehydrate.len(), 1);
        let r = &plan.rehydrate[0];
        assert_eq!(r.uid, "cnb:albi:disparu-paul");
        assert!(!r.active);
        assert_eq!(r.denomination, "disparu paul");
        assert_eq!(r.barreau.as_deref(), Some("albi"));
        assert_eq!(
            r.activity,
            Some(serde_json::json!([{"au": "2026-07-17", "src": "cnb"}]))
        );

        // Cible déjà au format CNBF en base (run partiel antérieur) → merge.
        let existing = [
            state("cnb:900001", "Louise Bertin"),
            state("cnb:agen:bertin-louise", "Louise Bertin"),
        ];
        let plan = rekey_plan(&rows, &existing, &[], snapshot);
        assert!(plan.renames.is_empty());
        assert_eq!(
            plan.merges,
            vec![(
                "cnb:agen:bertin-louise".to_string(),
                "cnb:900001".to_string()
            )]
        );
    }
}
