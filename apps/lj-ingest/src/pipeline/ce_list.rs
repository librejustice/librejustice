//! Job `sync-ce-list` : les grandes décisions que le Conseil d'État publie
//! nomment des décisions du fonds que rien d'autre ne nomme. Le titre donne le
//! nom, le lien ArianeWeb de la fiche donne l'ancre. Le titre indexé, lui, se
//! recompose à la passe `renvois`, qui relit toutes les décisions nommées.
//!
//! Une fiche en ancre parfois plusieurs (« Hardouin et Marie ») : ce nom n'est
//! celui d'aucune, et n'entre chez elles qu'en clé.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_core::case_names::case_name_key;
use lj_parse::{
    ce_list_names, is_pseudonymised, parse_ce_list_anchors, parse_ce_list_page, split_joint_name,
    CeListAnchor, CeListEntry,
};
use lj_sources::ce_list;
use lj_store::repository::{DecisionRepository, ElectedCaseName};

use crate::case_names::add_short_keys;
use crate::config::Settings;

pub async fn sync_ce_list() -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("ce-list");
    fs::create_dir_all(&dir)?;
    let client = ce_list::http_client().map_err(|e| anyhow!("client ce-list: {e}"))?;

    let (pages, entries) = enumerate(&client).await?;
    let (anchors, pseudonymes, sans_nom) = harvest(&client, &dir, &entries).await?;

    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let jurisdictions: Vec<&str> = anchors
        .iter()
        .map(|(a, _)| a.jurisdiction.as_str())
        .collect();
    let dates: Vec<NaiveDate> = anchors
        .iter()
        .map(|(a, _)| a.date.parse())
        .collect::<std::result::Result<_, _>>()?;
    let dockets: Vec<&str> = anchors.iter().map(|(a, _)| a.docket.as_str()).collect();
    let targets = repo
        .resolve_docket_anchors(&jurisdictions, &dates, &dockets)
        .await?;

    let mut couverture: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for ((_, name), target) in anchors.iter().zip(&targets) {
        if let Some(decision_id) = target {
            couverture
                .entry(case_name_key(name))
                .or_default()
                .insert(*decision_id);
        }
    }

    let mut by_decision: BTreeMap<i64, ElectedCaseName> = BTreeMap::new();
    let (mut orphelines, mut groupes) = (0usize, 0usize);
    for ((anchor, name), target) in anchors.iter().zip(&targets) {
        let Some(decision_id) = *target else {
            tracing::warn!(
                juridiction = anchor.jurisdiction,
                date = anchor.date,
                numero = anchor.docket,
                nom = name,
                "anchor without a decision in the corpus"
            );
            orphelines += 1;
            continue;
        };
        let key = case_name_key(name);
        if key.is_empty() {
            continue;
        }
        let entry = by_decision
            .entry(decision_id)
            .or_insert_with(|| ElectedCaseName {
                decision_id,
                name: None,
                keys: Vec::new(),
                source: "ce-list",
            });
        if couverture[&key].len() == 1 {
            entry.name = Some(name.clone());
        } else {
            groupes += 1;
        }
        if !entry.keys.contains(&key) {
            entry.keys.push(key);
        }
    }

    let courts = add_short_keys(&repo, &mut by_decision).await?;
    let elected: Vec<ElectedCaseName> = by_decision.into_values().collect();
    let poses = repo.put_case_names(&elected).await?;
    println!(
        "ce-list : {pages} pages lues, {} entrées, {} ancres dont {pseudonymes} pseudonymisées et \
         {sans_nom} sans nom à leur date, {} résolues au fonds, {orphelines} sans décision, \
         {} noms d'usage dont {groupes} en clé seule et {poses} changés, {courts} clés courtes \
         trop courantes écartées",
        entries.len(),
        anchors.len() + pseudonymes + sans_nom,
        anchors.len() - orphelines,
        elected.len(),
    );
    Ok(())
}

/// Toutes les entrées de la liste : les pages se suivent tant qu'elles sont
/// pleines, la dernière est plus courte que la première.
async fn enumerate(client: &reqwest::Client) -> Result<(u32, Vec<CeListEntry>)> {
    let mut entries: Vec<CeListEntry> = Vec::new();
    let mut pleine = usize::MAX;
    for page in 1.. {
        let html = ce_list::list_page(client, page)
            .await
            .map_err(|e| anyhow!("liste ce-list p{page}: {e}"))?;
        tokio::time::sleep(ce_list::THROTTLE).await;
        let lot = parse_ce_list_page(&html);
        let n = lot.len();
        entries.extend(lot);
        if page == 1 {
            pleine = n;
        }
        if n == 0 || n < pleine {
            return Ok((page, entries));
        }
    }
    unreachable!("la boucle rend sur la première page incomplète")
}

/// Les ancres nommables des fiches. Une ancre dont aucun segment du titre ne
/// porte la date n'est pas décrite par ce titre, et une ancre pseudonymisée ne
/// l'est par rien. Quand une date porte plusieurs ancres, les noms leur sont
/// appariés dans l'ordre du titre dès que le compte tombe juste ; sinon le nom
/// conjoint va à chacune, où il ne servira que de clé.
async fn harvest(
    client: &reqwest::Client,
    dir: &Path,
    entries: &[CeListEntry],
) -> Result<(Vec<(CeListAnchor, String)>, usize, usize)> {
    let mut anchors = Vec::new();
    let (mut pseudonymes, mut sans_nom) = (0usize, 0usize);
    for entry in entries {
        let (html, fetched) = ce_list::entry_page_cached(client, dir, &entry.path)
            .await
            .map_err(|e| anyhow!("fiche {}: {e}", entry.path))?;
        if fetched {
            tokio::time::sleep(ce_list::THROTTLE).await;
        }
        let names = ce_list_names(&entry.title);
        let mut par_date: BTreeMap<String, Vec<CeListAnchor>> = BTreeMap::new();
        for anchor in parse_ce_list_anchors(&html) {
            par_date
                .entry(anchor.date.clone())
                .or_default()
                .push(anchor);
        }
        for (date, lot) in par_date {
            let noms: Vec<&str> = names
                .iter()
                .filter(|n| n.date == date)
                .map(|n| n.name.as_str())
                .collect();
            let Some(&joint) = noms.first() else {
                for a in &lot {
                    tracing::warn!(
                        titre = entry.title,
                        date = a.date,
                        numero = a.docket,
                        "anchor without a name at its date"
                    );
                }
                sans_nom += lot.len();
                continue;
            };
            let parts = if noms.len() == lot.len() {
                Some(noms.iter().map(|n| (*n).to_string()).collect())
            } else if noms.len() == 1 {
                split_joint_name(joint, lot.len())
            } else {
                None
            };
            for (i, a) in lot.into_iter().enumerate() {
                let nom = parts
                    .as_ref()
                    .map_or(joint, |p: &Vec<String>| p[i].as_str());
                let nom = nom.to_string();
                if is_pseudonymised(&nom) {
                    pseudonymes += 1;
                    continue;
                }
                anchors.push((a, nom));
            }
        }
    }
    Ok((anchors, pseudonymes, sans_nom))
}
