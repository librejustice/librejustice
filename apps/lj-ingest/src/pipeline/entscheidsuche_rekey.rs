//! One-shot: an entscheidsuche spider that renamed its documents. Two of its job files, one
//! from before the renaming and one from after, pair each new id with the old ids of the same
//! file by checksum; or, for a spider that moved its dates by a day, each id the index no longer
//! serves pairs with the one it serves under the same docket a day apart. The decisions of one
//! pair become one, under the new id. A text the index lists under several ids of one docket,
//! each held as its own decision, becomes one decision holding every provenance.

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, bail, Result};
use futures::{stream, TryStreamExt};
use serde_json::Value;

use lj_sources::entscheidsuche::EntscheidsucheClient;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::europe::europe_pool;

const PREFIX: &str = "entscheidsuche/";
/// Groups applied at once, each on its own connection.
const WORKERS: usize = 3;

/// A file the spider holds under a new id, and the ids it held it under before.
#[derive(Debug, PartialEq)]
struct Group {
    new: String,
    old: Vec<String>,
}

/// Each document file of a job, `<spider>/<id>.<ext>` beside its `.json` metadata, by checksum.
fn files(job: &Value) -> Result<BTreeMap<String, Vec<String>>> {
    let entries = job
        .get("dateien")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("job file without dateien"))?;
    let mut by_checksum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, entry) in entries {
        let name = path.rsplit('/').next().unwrap_or(path);
        let Some((id, ext)) = name.rsplit_once('.') else {
            bail!("job file entry without extension: {path}");
        };
        if ext == "json" {
            continue;
        }
        let checksum = entry
            .get("checksum")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("job file entry without checksum: {path}"))?;
        by_checksum
            .entry(checksum.to_string())
            .or_default()
            .push(id.to_string());
    }
    Ok(by_checksum)
}

/// Each id of `after` whose file `before` held under other ids.
fn groups(before: &Value, after: &Value) -> Result<Vec<Group>> {
    let before = files(before)?;
    let mut groups = Vec::new();
    for (checksum, new) in files(after)? {
        let [new] = new.as_slice() else {
            bail!("job file holds one file under {new:?}");
        };
        let Some(old) = before.get(&checksum) else {
            continue;
        };
        let mut old: Vec<String> = old.iter().filter(|o| *o != new).cloned().collect();
        if old.is_empty() {
            continue;
        }
        old.sort();
        groups.push(Group {
            new: new.clone(),
            old,
        });
    }
    Ok(groups)
}

/// `<court>_<chamber>_<docket>` and the day an id ends on.
fn dated(id: &str) -> Option<(&str, jiff::civil::Date)> {
    let (stem, day) = id.rsplit_once('_')?;
    Some((stem, day.parse().ok()?))
}

/// Each id of `held` the index no longer lists, beside the one listed id of the same stem dated
/// a day apart.
fn shifted(listed: &[String], held: &[String]) -> Vec<Group> {
    let listed_set: std::collections::HashSet<&str> = listed.iter().map(String::as_str).collect();
    let mut by_stem: HashMap<&str, Vec<(&str, jiff::civil::Date)>> = HashMap::new();
    for id in listed {
        if let Some((stem, day)) = dated(id) {
            by_stem.entry(stem).or_default().push((id, day));
        }
    }
    let mut groups: Vec<Group> = held
        .iter()
        .filter(|id| !listed_set.contains(id.as_str()))
        .filter_map(|id| {
            let (stem, day) = dated(id)?;
            let near: Vec<&str> = by_stem
                .get(stem)?
                .iter()
                .filter(|(_, d)| d.since(day).is_ok_and(|span| span.get_days().abs() == 1))
                .map(|(i, _)| *i)
                .collect();
            let [new] = near.as_slice() else {
                return None;
            };
            Some(Group {
                new: new.to_string(),
                old: vec![id.clone()],
            })
        })
        .collect();
    groups.sort_by(|a, b| a.new.cmp(&b.new));
    groups
}

/// What one group writes: the copies absorbed into the keeper, then the provenances deleted
/// from it, then its own provenance renamed to the new id.
#[derive(Debug, Default, PartialEq)]
struct Merge {
    keeper: i64,
    copies: Vec<i64>,
    delete: Vec<String>,
    rename: Option<(String, String)>,
}

/// The keeper is the decision with a summary, else the longest published, else the first; its
/// own provenance carries its text and takes the new id.
fn merge(group: &Group, held: &HashMap<String, (i64, bool)>) -> Option<Merge> {
    let uid = |id: &str| format!("{PREFIX}{id}");
    let new = uid(&group.new);
    let old: Vec<(String, i64, bool)> = group
        .old
        .iter()
        .filter_map(|id| {
            let u = uid(id);
            held.get(&u).map(|&(d, s)| (u, d, s))
        })
        .collect();
    if old.is_empty() {
        return None;
    }
    let mut all = old.clone();
    if let Some(&(d, s)) = held.get(&new) {
        all.push((new.clone(), d, s));
    }
    let (keeper_uid, keeper, _) = all
        .iter()
        .min_by_key(|(u, d, s)| (!s, *u == new, *d))
        .cloned()
        .expect("old is not empty");
    let mut copies: Vec<i64> = all
        .iter()
        .map(|(_, d, _)| *d)
        .filter(|d| *d != keeper)
        .collect();
    copies.sort_unstable();
    copies.dedup();
    let mut delete: Vec<String> = all
        .iter()
        .map(|(u, _, _)| u.clone())
        .filter(|u| *u != keeper_uid)
        .collect();
    delete.sort();
    let rename = (keeper_uid != new).then_some((keeper_uid, new));
    Some(Merge {
        keeper,
        copies,
        delete,
        rename,
    })
}

/// `<court>_<chamber>_<docket>`, an id without its day.
fn stem(id: &str) -> &str {
    id.rsplit_once('_').map_or(id, |(stem, _)| stem)
}

/// One text held as several decisions under ids of one stem. `authority`: the decision holding
/// the lowest `source_uid`, which `reconcile` elects and whose day the merged decision takes.
#[derive(Debug, PartialEq)]
struct Twins {
    stem: String,
    merge: Merge,
    authority: i64,
}

impl Twins {
    fn court(&self) -> String {
        self.stem
            .splitn(3, '_')
            .take(2)
            .collect::<Vec<_>>()
            .join("_")
    }
}

/// Each text held as several decisions under ids of one stem, merged into the one with a
/// summary, else the oldest, every provenance kept on it.
fn twins(held: &HashMap<String, (i64, bool)>, digests: &HashMap<i64, String>) -> Vec<Twins> {
    type Held<'a> = Vec<(&'a str, i64, bool)>;
    let mut by_text: BTreeMap<(&str, &str), Held> = BTreeMap::new();
    for (uid, &(decision, summary)) in held {
        let Some(digest) = digests.get(&decision) else {
            continue;
        };
        let id = uid.strip_prefix(PREFIX).unwrap_or(uid);
        by_text
            .entry((stem(id), digest))
            .or_default()
            .push((uid, decision, summary));
    }
    by_text
        .into_iter()
        .filter_map(|((stem, _), mut held)| {
            held.sort_unstable();
            let keeper = held.iter().min_by_key(|(_, d, s)| (!s, *d))?.1;
            let mut copies: Vec<i64> = held
                .iter()
                .map(|(_, d, _)| *d)
                .filter(|d| *d != keeper)
                .collect();
            copies.sort_unstable();
            copies.dedup();
            (!copies.is_empty()).then(|| Twins {
                stem: stem.to_string(),
                merge: Merge {
                    keeper,
                    copies,
                    delete: Vec::new(),
                    rename: None,
                },
                authority: held[0].1,
            })
        })
        .collect()
}

/// Which of a group's days its text states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stated {
    /// The authority's, the group's earliest.
    Earliest,
    /// The authority's, later than another of the group's.
    Later,
    /// Another of the group's, not the authority's: merged, the decision would take a day its
    /// text does not state.
    Another,
    /// None of the group's.
    Neither,
}

fn stated(t: &Twins, read: &HashMap<i64, (Option<String>, String)>) -> Stated {
    let day = |d: &i64| {
        read.get(d)
            .and_then(|(day, _)| day.as_deref()?.parse::<jiff::civil::Date>().ok())
    };
    let Some((_, text)) = read.get(&t.authority) else {
        return Stated::Neither;
    };
    let others: Vec<jiff::civil::Date> = std::iter::once(&t.merge.keeper)
        .chain(&t.merge.copies)
        .filter(|d| **d != t.authority)
        .filter_map(day)
        .collect();
    match day(&t.authority) {
        Some(own) if lj_parse::states_day(text, own) => {
            if others.iter().all(|o| own <= *o) {
                Stated::Earliest
            } else {
                Stated::Later
            }
        }
        _ if others.iter().any(|o| lj_parse::states_day(text, *o)) => Stated::Another,
        _ => Stated::Neither,
    }
}

/// The twins the base holds, ids of one stem whose decisions serve one text, with the days it
/// states.
async fn held_twins(repo: &DecisionRepository<'_>) -> Result<Vec<(Twins, Stated)>> {
    let held = repo.entscheidsuche_held().await?;
    let mut ids_per_stem: HashMap<&str, usize> = HashMap::new();
    for (id, _, _) in &held {
        *ids_per_stem.entry(stem(id)).or_default() += 1;
    }
    let uids: Vec<String> = held
        .iter()
        .filter(|(id, _, _)| ids_per_stem[stem(id)] > 1)
        .map(|(id, _, _)| format!("{PREFIX}{id}"))
        .collect();
    let mut decisions = HashMap::new();
    for chunk in uids.chunks(20_000) {
        decisions.extend(repo.provenance_decisions(chunk).await?);
    }
    let mut per_stem: HashMap<&str, Vec<i64>> = HashMap::new();
    for (uid, (decision, _)) in &decisions {
        per_stem
            .entry(stem(&uid[PREFIX.len()..]))
            .or_default()
            .push(*decision);
    }
    let mut ids: Vec<i64> = per_stem
        .into_values()
        .filter_map(|mut d| {
            d.sort_unstable();
            d.dedup();
            (d.len() > 1).then_some(d)
        })
        .flatten()
        .collect();
    ids.sort_unstable();
    let mut digests = HashMap::new();
    for chunk in ids.chunks(20_000) {
        digests.extend(repo.text_digests(chunk).await?);
    }
    let found = twins(&decisions, &digests);
    let grouped: Vec<i64> = found
        .iter()
        .flat_map(|t| std::iter::once(t.merge.keeper).chain(t.merge.copies.iter().copied()))
        .collect();
    let mut read = HashMap::new();
    for chunk in grouped.chunks(500) {
        read.extend(repo.days_and_texts(chunk).await?);
    }
    Ok(found
        .into_iter()
        .map(|t| {
            let s = stated(&t, &read);
            (t, s)
        })
        .collect())
}

/// How the old ids pair with the new ones.
pub enum Pairing {
    /// A spider's job files from before and after the renaming.
    Jobs {
        spider: String,
        before: String,
        after: String,
    },
    /// A court whose ids moved by a day.
    Shifted { court: String },
    /// Every court: one text held as several decisions under ids of one stem.
    Twins,
}

/// Merges each renamed document's decisions into one, under its new id. **Dry run by default**:
/// without `apply`, counts and writes nothing. Idempotent: a group already merged holds no old id.
pub async fn entscheidsuche_rekey(pairing: Pairing, apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = EntscheidsucheClient::new(&settings.egress_proxy)?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    if !repo.entscheidsuche_lock("").await? {
        bail!("entscheidsuche rekey: a sync holds the lock");
    }
    let (label, groups) = match &pairing {
        Pairing::Jobs {
            spider,
            before,
            after,
        } => (
            spider.clone(),
            groups(
                &client.job(spider, before).await?,
                &client.job(spider, after).await?,
            )?,
        ),
        Pairing::Shifted { court } => {
            let courts = [court.as_str()];
            let mut listed: Vec<String> = Vec::new();
            loop {
                let page = client
                    .listing(&courts, listed.last().map(String::as_str))
                    .await?;
                if page.is_empty() {
                    break;
                }
                listed.extend(page.into_iter().map(|l| l.id));
            }
            let prefix = format!("{court}_");
            let held: Vec<String> = repo
                .entscheidsuche_held()
                .await?
                .into_iter()
                .map(|(id, _, _)| id)
                .filter(|id| id.starts_with(&prefix))
                .collect();
            (court.clone(), shifted(&listed, &held))
        }
        Pairing::Twins => {
            let found = held_twins(&repo).await?;
            let mut per_court: BTreeMap<String, [usize; 5]> = BTreeMap::new();
            let mut per_day: BTreeMap<Stated, usize> = BTreeMap::new();
            for (t, s) in &found {
                *per_day.entry(*s).or_default() += 1;
                let c = per_court.entry(t.court()).or_default();
                match s {
                    Stated::Earliest | Stated::Later => {
                        c[0] += 1;
                        c[1] += t.merge.copies.len();
                        c[2] += usize::from(t.authority != t.merge.keeper);
                    }
                    Stated::Another => c[3] += 1,
                    Stated::Neither => c[4] += 1,
                }
            }
            println!("entscheidsuche rekey twins: merged texts, copies absorbed, keepers to rewrite, left whose text states another day, left whose text states none");
            for (court, c) in &per_court {
                println!(
                    "  {court}\t{}\t{}\t{}\t{}\t{}",
                    c[0], c[1], c[2], c[3], c[4]
                );
            }
            println!("  days: {per_day:?}");
            for (t, s) in found.iter().filter(|(_, s)| *s >= Stated::Another) {
                println!("  left {s:?} {} {:?}", t.stem, t.merge);
            }
            if !apply {
                println!("dry run: nothing written (--apply to write).");
                return Ok(());
            }
            let rewrite: Vec<&str> = per_court
                .iter()
                .filter(|(_, c)| c[2] > 0)
                .map(|(court, _)| court.as_str())
                .collect();
            let merges: Vec<Merge> = found
                .into_iter()
                .filter(|(_, s)| *s < Stated::Another)
                .map(|(t, _)| t.merge)
                .collect();
            apply_merges(&pool, &merges).await?;
            println!(
                "then: sync-entscheidsuche --missing --rewrite --full --court {}",
                rewrite.join(" --court ")
            );
            return Ok(());
        }
    };
    let uids: Vec<String> = groups
        .iter()
        .flat_map(|g| std::iter::once(&g.new).chain(&g.old))
        .map(|id| format!("{PREFIX}{id}"))
        .collect();
    let mut held = HashMap::new();
    for chunk in uids.chunks(20_000) {
        held.extend(repo.provenance_decisions(chunk).await?);
    }
    let merges: Vec<Merge> = groups.iter().filter_map(|g| merge(g, &held)).collect();

    let new_held = groups
        .iter()
        .filter(|g| held.contains_key(&format!("{PREFIX}{}", g.new)))
        .count();
    let summarised_lost = merges
        .iter()
        .flat_map(|m| &m.delete)
        .filter(|u| held.get(*u).is_some_and(|&(_, s)| s))
        .count();
    println!(
        "entscheidsuche rekey {label}: {} documents renamed, {} with a decision held, {} new ids held, \
         {} copies absorbed, {} provenances deleted, {} renamed, {} deleted provenances whose decision had a summary",
        groups.len(),
        merges.len(),
        new_held,
        merges.iter().map(|m| m.copies.len()).sum::<usize>(),
        merges.iter().map(|m| m.delete.len()).sum::<usize>(),
        merges.iter().filter(|m| m.rename.is_some()).count(),
        summarised_lost,
    );
    for m in merges.iter().take(5) {
        println!("  {m:?}");
    }
    if !apply {
        println!("dry run: nothing written (--apply to write).");
        return Ok(());
    }
    apply_merges(&pool, &merges).await
}

/// Absorbs each merge's copies into its keeper, then deletes and renames its provenances.
async fn apply_merges(pool: &deadpool_postgres::Pool, merges: &[Merge]) -> Result<()> {
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = merges.len();
    stream::iter(merges.iter().map(Ok::<_, anyhow::Error>))
        .try_for_each_concurrent(WORKERS, |m| {
            let done = &done;
            async move {
                let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
                let repo = DecisionRepository::new(&conn);
                for &copy in &m.copies {
                    repo.absorb_into(copy, m.keeper).await?;
                }
                for uid in &m.delete {
                    repo.delete_provenance(uid).await?;
                }
                if let Some((from, to)) = &m.rename {
                    repo.rename_provenance(from, to).await?;
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n.is_multiple_of(2_000) {
                    tracing::info!(done = n, total, "entscheidsuche rekey");
                }
                Ok(())
            }
        })
        .await?;
    tracing::warn!(merged = total, "entscheidsuche rekey done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(files: &[(&str, &str)]) -> Value {
        let mut dateien = serde_json::Map::new();
        for (id, checksum) in files {
            dateien.insert(
                format!("VD_FindInfo/{id}.pdf"),
                json!({"checksum": checksum}),
            );
            dateien.insert(
                format!("VD_FindInfo/{id}.json"),
                json!({"checksum": "meta"}),
            );
        }
        json!({ "dateien": dateien })
    }

    #[test]
    fn a_renamed_file_groups_every_id_it_was_held_under() {
        let before = job(&[
            ("D1", "a"),
            ("H1", "a"),
            ("D2", "b"),
            ("D3", "c"),
            ("D4", "d"),
        ]);
        let after = job(&[("U1", "a"), ("U2", "b"), ("D3", "c"), ("U5", "e")]);
        assert_eq!(
            groups(&before, &after).unwrap(),
            vec![
                Group {
                    new: "U1".into(),
                    old: vec!["D1".into(), "H1".into()],
                },
                Group {
                    new: "U2".into(),
                    old: vec!["D2".into()],
                },
            ]
        );
    }

    #[test]
    fn an_id_moved_by_a_day_pairs_with_its_one_neighbour() {
        let listed: Vec<String> = [
            "ZH_SVG_001_AB-1999-00506_2003-06-23",
            "ZH_SVG_001_AL-2003-00175_2004-03-24",
            "ZH_SVG_001_IV-2010-00001_2011-01-05",
            "ZH_SVG_001_UV-2011-00279_2013-05-17",
            "ZH_SVG_001_UV-2011-00279_2013-05-15",
        ]
        .map(String::from)
        .to_vec();
        let held: Vec<String> = [
            "ZH_SVG_001_AB-1999-00506_2003-06-22",
            "ZH_SVG_001_AL-2003-00175_2004-03-24",
            "ZH_SVG_001_IV-2010-00001_2010-08-20",
            "ZH_SVG_001_UV-2011-00279_2013-05-16",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            shifted(&listed, &held),
            vec![Group {
                new: "ZH_SVG_001_AB-1999-00506_2003-06-23".into(),
                old: vec!["ZH_SVG_001_AB-1999-00506_2003-06-22".into()],
            }]
        );
    }

    fn held(rows: &[(&str, i64, bool)]) -> HashMap<String, (i64, bool)> {
        rows.iter()
            .map(|(id, d, s)| (format!("{PREFIX}{id}"), (*d, *s)))
            .collect()
    }

    fn uid(id: &str) -> String {
        format!("{PREFIX}{id}")
    }

    #[test]
    fn the_summarised_old_decision_keeps_its_public_id_and_takes_the_new_id() {
        let g = Group {
            new: "U1".into(),
            old: vec!["D1".into(), "H1".into(), "H2".into()],
        };
        let m = merge(
            &g,
            &held(&[
                ("D1", 10, false),
                ("H1", 11, true),
                ("H2", 12, false),
                ("U1", 90, false),
            ]),
        )
        .unwrap();
        assert_eq!(
            m,
            Merge {
                keeper: 11,
                copies: vec![10, 12, 90],
                delete: vec![uid("D1"), uid("H2"), uid("U1")],
                rename: Some((uid("H1"), uid("U1"))),
            }
        );
    }

    #[test]
    fn a_new_decision_keeps_only_a_summary_none_of_the_old_ones_has() {
        let g = Group {
            new: "U1".into(),
            old: vec!["D1".into()],
        };
        let m = merge(&g, &held(&[("D1", 10, false), ("U1", 90, true)])).unwrap();
        assert_eq!(
            m,
            Merge {
                keeper: 90,
                copies: vec![10],
                delete: vec![uid("D1")],
                rename: None,
            }
        );
    }

    #[test]
    fn an_old_decision_not_yet_read_under_its_new_id_is_renamed() {
        let g = Group {
            new: "U1".into(),
            old: vec!["D1".into(), "H1".into()],
        };
        let m = merge(&g, &held(&[("D1", 10, false)])).unwrap();
        assert_eq!(
            m,
            Merge {
                keeper: 10,
                copies: vec![],
                delete: vec![],
                rename: Some((uid("D1"), uid("U1"))),
            }
        );
        assert_eq!(merge(&g, &held(&[("U1", 90, false)])), None);
    }

    #[test]
    fn one_text_held_under_two_ids_of_a_docket_becomes_one_decision_with_both() {
        let held = held(&[
            ("GR_VG_006_PVG-2002-10_0000-00-00", 90, false),
            ("GR_VG_006_PVG-2002-10_2002-12-31", 10, true),
            ("SH_OG_001_40-2016-28_2016-11-25", 20, false),
            ("SH_OG_001_40-2016-28_2021-02-10", 21, false),
            ("GR_VG_001_U-2005-21_2005-11-17", 40, false),
            ("GR_VG_001_U-2005-21_2006-11-17", 41, false),
            ("ZH_OG_001_LB110047_2011-11-15", 50, false),
            ("ZH_OG_001_LB110047_2012-01-13", 50, false),
        ]);
        let digests: HashMap<i64, String> = [
            (90, "a"),
            (10, "a"),
            (20, "b"),
            (21, "b"),
            (40, "c"),
            (41, "d"),
            (50, "e"),
        ]
        .into_iter()
        .map(|(d, t)| (d, t.to_string()))
        .collect();
        assert_eq!(
            twins(&held, &digests),
            vec![
                Twins {
                    stem: "GR_VG_006_PVG-2002-10".into(),
                    merge: Merge {
                        keeper: 10,
                        copies: vec![90],
                        delete: vec![],
                        rename: None,
                    },
                    authority: 90,
                },
                Twins {
                    stem: "SH_OG_001_40-2016-28".into(),
                    merge: Merge {
                        keeper: 20,
                        copies: vec![21],
                        delete: vec![],
                        rename: None,
                    },
                    authority: 20,
                },
            ]
        );
    }
}
