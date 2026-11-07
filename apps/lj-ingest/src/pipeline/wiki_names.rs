//! Job `sync-wiki-names`: the usage names French Wikipedia and Wikidata give the decisions of the corpus.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_core::case_names::case_name_key;
use lj_core::citation::name_words;
use lj_parse::{wiki_list_entries, wiki_page_entry, wiki_usage_name, WikiEntry};
use lj_sources::wiki;
use lj_store::repository::{DecisionRepository, ElectedCaseName};

use crate::case_names::{add_short_keys, keeps};
use crate::config::Settings;

const CATEGORIES: [&str; 3] = [
    "Catégorie:Jurisprudence du Conseil d'État français",
    "Catégorie:Jurisprudence de la Cour de cassation française",
    "Catégorie:Jurisprudence du Tribunal des conflits (France)",
];
const MAJOR_DECISIONS: &str = "Grands arrêts en droit administratif français";
/// A line of the list writes the day, never the court.
const LIST_COURTS: [&str; 2] = ["ce", "tc"];
const SOURCE: &str = "wikimedia";

/// The names one page, list line or Wikidata item gives, and what anchors them.
#[derive(Debug, Default)]
struct Named {
    origin: String,
    names: Vec<String>,
    legifrance: Vec<String>,
    court: Option<&'static str>,
    date: Option<NaiveDate>,
    dockets: Vec<String>,
    ecli: Option<String>,
    wikidata: Option<String>,
}

impl Named {
    fn from_entry(origin: &str, entry: WikiEntry, wikidata: Option<String>) -> Self {
        Named {
            origin: origin.to_string(),
            names: Some(entry.name)
                .filter(|n| !n.is_empty())
                .into_iter()
                .collect(),
            legifrance: entry.legifrance,
            court: entry.court,
            date: entry.date.and_then(|d| d.parse().ok()),
            dockets: entry.dockets,
            wikidata,
            ..Default::default()
        }
    }
}

/// A decision read that day, and the words of its title and opening.
type HeadWords = (i64, HashSet<String>);

enum Anchor {
    Decisions(BTreeSet<i64>),
    SeveralThatDay(usize),
    Nothing,
}

#[derive(Default)]
struct Gathered {
    names: Vec<String>,
    joint: Vec<String>,
    wikidata: Option<String>,
}

pub async fn sync_wiki_names() -> Result<()> {
    let settings = Settings::from_env()?;
    let client = wiki::http_client().map_err(|e| anyhow!("wiki client: {e}"))?;

    let mut titles = BTreeSet::from([MAJOR_DECISIONS.to_string()]);
    for category in CATEGORIES {
        let pages = wiki::category_pages(&client, category)
            .await
            .map_err(|e| anyhow!("{category}: {e}"))?;
        titles.extend(pages);
    }
    let titles: Vec<String> = titles.into_iter().collect();
    let pages = wiki::pages(&client, &titles)
        .await
        .map_err(|e| anyhow!("wikipedia pages: {e}"))?;
    let items = wiki::wikidata_decisions(&client)
        .await
        .map_err(|e| anyhow!("wikidata: {e}"))?;

    let mut named: Vec<Named> = Vec::new();
    let mut without_infobox = 0usize;
    for page in &pages {
        if page.title == MAJOR_DECISIONS {
            named.extend(
                wiki_list_entries(&page.wikitext)
                    .into_iter()
                    .map(|e| Named::from_entry(MAJOR_DECISIONS, e, None)),
            );
        } else if let Some(entry) = wiki_page_entry(&page.title, &page.wikitext) {
            named.push(Named::from_entry(&page.title, entry, page.wikidata.clone()));
        } else {
            without_infobox += 1;
        }
    }
    let wikidata_items = items.len();
    for item in items {
        let names = [item.label_fr.as_deref(), item.page_fr.as_deref()]
            .into_iter()
            .flatten()
            .map(wiki_usage_name)
            .filter(|n| !n.is_empty())
            .collect();
        named.push(Named {
            origin: item.item.clone(),
            names,
            legifrance: item.legifrance.into_iter().collect(),
            ecli: item.ecli,
            wikidata: Some(item.item),
            ..Default::default()
        });
    }

    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let anchors = anchor(&repo, &named).await?;

    let mut by_decision: BTreeMap<i64, Gathered> = BTreeMap::new();
    let (mut nothing, mut several) = (0usize, 0usize);
    for (n, a) in named.iter().zip(anchors) {
        let name = n.names.first().map(String::as_str).unwrap_or("");
        let date = n.date.map(|d| d.to_string()).unwrap_or_default();
        let ids = match a {
            Anchor::Decisions(ids) => ids,
            Anchor::SeveralThatDay(count) => {
                tracing::warn!(
                    origin = n.origin,
                    name,
                    date,
                    count,
                    "several decisions of its day write the name"
                );
                several += 1;
                continue;
            }
            Anchor::Nothing => {
                tracing::warn!(
                    origin = n.origin,
                    name,
                    date,
                    "entry without a decision in the corpus"
                );
                nothing += 1;
                continue;
            }
        };
        for id in &ids {
            let g = by_decision.entry(*id).or_default();
            match ids.len() {
                1 => g.names.extend(n.names.iter().cloned()),
                _ => g.joint.extend(n.names.iter().cloned()),
            }
            if n.wikidata.is_some() {
                g.wikidata.clone_from(&n.wikidata);
            }
        }
    }

    let qids: Vec<(i64, String)> = by_decision
        .iter()
        .filter_map(|(id, g)| Some((*id, g.wikidata.clone()?)))
        .collect();
    let ids: Vec<i64> = by_decision.keys().copied().collect();
    let current: HashMap<i64, (String, String)> = repo
        .current_case_names(&ids)
        .await?
        .into_iter()
        .filter_map(|(id, name, source)| Some((id, (name?, source?))))
        .collect();
    let mut elected: BTreeMap<i64, ElectedCaseName> = BTreeMap::new();
    for (id, g) in by_decision {
        let mut keys: Vec<String> = g
            .names
            .iter()
            .chain(&g.joint)
            .map(|n| case_name_key(n))
            .filter(|k| !k.is_empty())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.is_empty() {
            continue;
        }
        elected.insert(
            id,
            ElectedCaseName {
                decision_id: id,
                name: longest(&g.names).filter(|n| match current.get(&id) {
                    Some((name, source)) => !keeps(name, source, n, SOURCE),
                    None => true,
                }),
                keys,
                source: SOURCE,
            },
        );
    }
    let shorts = add_short_keys(&repo, &mut elected).await?;
    let elected: Vec<ElectedCaseName> = elected.into_values().collect();
    let changed = repo.put_case_names(&elected).await?;
    let kept: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
    let cleared = repo.clear_case_names_except(&[SOURCE], &kept).await?;
    let items_changed = repo.put_wikidata_ids(&qids).await?;
    println!(
        "wiki-names: {} pages read, {without_infobox} without a decision infobox, {wikidata_items} \
         Wikidata items, {} entries, {nothing} without a decision, {several} with several that day, \
         {} named decisions of which {changed} changed and {} cleared, {shorts} short keys too \
         common, {} Wikidata ids of which {items_changed} changed",
        pages.len(),
        named.len(),
        elected.len(),
        cleared.len(),
        qids.len(),
    );
    Ok(())
}

/// The name of most words, the first on a tie: the long form holds the short one.
fn longest(names: &[String]) -> Option<String> {
    let words = |n: &String| case_name_key(n).split(' ').count();
    names
        .iter()
        .fold(None, |best: Option<&String>, n| match best {
            Some(b) if words(b) >= words(n) => Some(b),
            _ => Some(n),
        })
        .cloned()
}

/// Légifrance ids, then the ECLI, then court, day and numbers, then the one decision of its day writing the name.
async fn anchor(repo: &DecisionRepository<'_>, named: &[Named]) -> Result<Vec<Anchor>> {
    let uids: Vec<String> = named
        .iter()
        .flat_map(|n| n.legifrance.iter().flat_map(|id| source_uids(id)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let by_uid: HashMap<String, i64> = repo.resolve_source_uids(&uids).await?.into_iter().collect();
    let eclis: Vec<String> = named.iter().filter_map(|n| n.ecli.clone()).collect();
    let mut by_ecli: HashMap<String, BTreeSet<i64>> = HashMap::new();
    for (ecli, id) in repo.resolve_eclis(&eclis).await? {
        by_ecli.entry(ecli).or_default().insert(id);
    }

    let mut found: Vec<BTreeSet<i64>> = named
        .iter()
        .map(|n| {
            let mut ids: BTreeSet<i64> = n
                .legifrance
                .iter()
                .flat_map(|id| source_uids(id))
                .filter_map(|uid| by_uid.get(&uid).copied())
                .collect();
            if let Some(e) = n.ecli.as_ref().and_then(|e| by_ecli.get(e)) {
                ids.extend(e);
            }
            ids
        })
        .collect();

    let (mut jurs, mut dates, mut dockets, mut owner) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, n) in named.iter().enumerate() {
        if let (true, Some(court), Some(date)) = (found[i].is_empty(), n.court, n.date) {
            for docket in &n.dockets {
                jurs.push(court);
                dates.push(date);
                dockets.push(docket.as_str());
                owner.push(i);
            }
        }
    }
    let resolved = repo.resolve_docket_anchors(&jurs, &dates, &dockets).await?;
    for (i, id) in owner.into_iter().zip(resolved) {
        found[i].extend(id);
    }

    let mut days: BTreeSet<(&str, NaiveDate)> = BTreeSet::new();
    for (i, n) in named.iter().enumerate() {
        if let (true, Some(date)) = (found[i].is_empty() && n.ecli.is_none(), n.date) {
            days.extend(courts_of(n).iter().map(|c| (*c, date)));
        }
    }
    let (day_jurs, day_dates): (Vec<&str>, Vec<NaiveDate>) = days.into_iter().unzip();
    let mut heads: HashMap<(String, NaiveDate), Vec<HeadWords>> = HashMap::new();
    for h in repo.day_heads(&day_jurs, &day_dates).await? {
        let words = case_name_key(&h.head)
            .split(' ')
            .map(str::to_string)
            .collect();
        heads
            .entry((h.jurisdiction, h.date))
            .or_default()
            .push((h.decision_id, words));
    }

    Ok(named
        .iter()
        .zip(found)
        .map(|(n, ids)| {
            if !ids.is_empty() {
                return Anchor::Decisions(ids);
            }
            let (Some(date), Some(name)) = (n.date, n.names.first()) else {
                return Anchor::Nothing;
            };
            let key = case_name_key(name);
            let words = name_words(&key);
            if words.is_empty() || n.ecli.is_some() {
                return Anchor::Nothing;
            }
            let writing: BTreeSet<i64> = courts_of(n)
                .iter()
                .filter_map(|c| heads.get(&(c.to_string(), date)))
                .flatten()
                .filter(|(_, head)| words.iter().all(|w| head.contains(*w)))
                .map(|(id, _)| *id)
                .collect();
            match writing.len() {
                0 => Anchor::Nothing,
                1 => Anchor::Decisions(writing),
                count => Anchor::SeveralThatDay(count),
            }
        })
        .collect())
}

fn courts_of(n: &Named) -> &'static [&'static str] {
    match n.court {
        Some("ce") => &["ce"],
        Some("tc") => &["tc"],
        Some("cc") => &["cc"],
        _ => &LIST_COURTS,
    }
}

/// The source uids a Légifrance id is ingested under.
fn source_uids(id: &str) -> Vec<String> {
    let funds: &[&str] = match id.starts_with("CETATEXT") {
        true => &["dila-jade"],
        false => &["dila-cass", "dila-inca", "dila-capp"],
    };
    funds.iter().map(|f| format!("{f}/{id}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_form_is_shown() {
        let names = vec![
            "Dame Lamotte".to_string(),
            "Ministre de l'agriculture c/ Dame Lamotte".to_string(),
            "Lamotte".to_string(),
        ];
        assert_eq!(
            longest(&names).as_deref(),
            Some("Ministre de l'agriculture c/ Dame Lamotte")
        );
    }
}
