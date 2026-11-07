//! What the two writers of `decisions.case_name_keys`, `renvois` and
//! `sync-ce-list`, share: the rarity thresholds, the short key, the ordinal
//! names learnt from the citations.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use chrono::NaiveDate;
use lj_core::case_names::{case_name_key, short_case_name_key, SHORT_NAME_RARITY};
use lj_core::citation::name_words;
use lj_extract::ordinal::{
    ordinal_occurrences, ordinal_winners, unresolved_anchors, Candidate, Occurrence, ROMANS,
};
use lj_store::repository::{DecisionRepository, ElectedCaseName};

/// Rarity threshold of a name, in decisions writing it: every elected name sits under it,
/// parsing scrap and parties like « Ville de Paris » or « La Poste » above.
pub const NOM_TROP_COURANT: i64 = 10_000;

/// Adds to every entry the short key its keys derive, unless the corpus writes it
/// in [`SHORT_NAME_RARITY`] decisions or more. Returns how many were refused.
pub async fn add_short_keys(
    repo: &DecisionRepository<'_>,
    by_decision: &mut BTreeMap<i64, ElectedCaseName>,
) -> Result<usize> {
    let shorts: BTreeSet<String> = by_decision
        .values()
        .flat_map(|e| e.keys.iter().filter_map(|k| short_case_name_key(k)))
        .collect();
    let uniques: Vec<&str> = shorts.iter().map(String::as_str).collect();
    let courants: HashSet<String> = repo
        .frequent_names(&uniques, SHORT_NAME_RARITY)
        .await?
        .into_iter()
        .collect();
    for e in by_decision.values_mut() {
        let derived: Vec<String> = e
            .keys
            .iter()
            .filter_map(|k| short_case_name_key(k))
            .collect();
        for short in derived {
            if !courants.contains(&short) && !e.keys.contains(&short) {
                e.keys.push(short);
            }
        }
    }
    Ok(courants.len())
}

/// The longest key a line of decisions shares, in words: above, the phrase
/// query costs more than the ordinal it could find.
const MAX_ORDINAL_KEY_WORDS: usize = 5;

/// By decision: jurisdiction code, docket keys, reading day ISO.
type Anchors = HashMap<i64, (Option<String>, Vec<String>, Option<String>)>;

/// Adds to the decision each ordinal designates the key `<name> <roman>`, read
/// in the texts that cite « Béziers II » with its number or its day beside. A
/// decision the anchors resolve outside the bearers enters with the name of
/// the line. Returns how many ordinal keys were learnt.
pub async fn add_ordinal_keys(
    repo: &DecisionRepository<'_>,
    by_decision: &mut BTreeMap<i64, ElectedCaseName>,
) -> Result<usize> {
    let mut bearers: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for e in by_decision.values() {
        for k in &e.keys {
            bearers.entry(k.as_str()).or_default().push(e.decision_id);
        }
    }
    bearers.retain(|k, ids| ids.len() >= 2 && k.split(' ').count() <= MAX_ORDINAL_KEY_WORDS);
    let bearers: Vec<(String, Vec<i64>)> = bearers
        .into_iter()
        .map(|(k, ids)| (k.to_string(), ids))
        .collect();
    let ids: BTreeSet<i64> = bearers
        .iter()
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect();
    let ids: Vec<i64> = ids.into_iter().collect();
    let anchors: Anchors = repo
        .decision_anchors(&ids)
        .await?
        .into_iter()
        .map(|(id, jurisdiction, dockets, date)| (id, (jurisdiction, dockets, date)))
        .collect();

    let mut learnt = 0;
    for (key, ids) in &bearers {
        let texts = repo.ordinal_citation_texts(key, &ROMANS).await?;
        let occurrences: Vec<Occurrence> = texts
            .iter()
            .flat_map(|t| ordinal_occurrences(t, key))
            .collect();
        if occurrences.is_empty() {
            continue;
        }
        let mut candidates: Vec<Candidate> = ids
            .iter()
            .filter_map(|id| {
                anchors.get(id).map(|(_, dockets, date)| Candidate {
                    id: *id,
                    dockets: dockets.clone(),
                    date: date.clone(),
                })
            })
            .collect();
        let jurisdiction = ids
            .iter()
            .find_map(|id| anchors.get(id).and_then(|(j, _, _)| j.clone()));
        let pending = unresolved_anchors(&occurrences, &candidates);
        if let (Some(jurisdiction), false) = (jurisdiction, pending.is_empty()) {
            let dates: Vec<NaiveDate> = pending
                .iter()
                .map(|(day, _)| day.parse())
                .collect::<Result<_, _>>()?;
            let numbers: Vec<&str> = pending.iter().map(|(_, n)| n.as_str()).collect();
            let jurisdictions = vec![jurisdiction.as_str(); pending.len()];
            let found = repo
                .resolve_docket_anchors(&jurisdictions, &dates, &numbers)
                .await?;
            for ((day, number), id) in pending.iter().zip(found) {
                if let Some(id) = id {
                    if !candidates.iter().any(|c| c.id == id) {
                        candidates.push(Candidate {
                            id,
                            dockets: vec![number.clone()],
                            date: Some(day.clone()),
                        });
                    }
                }
            }
        }
        let name = ids
            .iter()
            .find_map(|id| by_decision.get(id).and_then(|e| e.name.clone()));
        for (roman, id) in ordinal_winners(&occurrences, &candidates) {
            let entry = by_decision.entry(id).or_insert_with(|| ElectedCaseName {
                decision_id: id,
                name: name.clone(),
                keys: vec![key.clone()],
                source: "renvoi",
            });
            let ordinal = format!("{key} {roman}");
            if !entry.keys.contains(&ordinal) {
                entry.keys.push(ordinal);
                learnt += 1;
            }
        }
    }
    Ok(learnt)
}

/// Whether the name in place stays over the one `writer` brings, which then adds keys alone:
/// it holds every word of it and more. A writer of higher rank keeps it anyway.
pub fn keeps(name: &str, source: &str, incoming: &str, writer: &str) -> bool {
    let words = |n: &str| -> HashSet<String> {
        name_words(&case_name_key(n))
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let (held, given) = (words(name), words(incoming));
    source != writer && held.is_superset(&given) && held.len() > given.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_in_place_stays_when_it_says_more() {
        for (name, source, incoming, kept) in [
            (
                "Compagnie Air France c/ Epoux Barbier",
                "renvoi",
                "Époux Barbier",
                true,
            ),
            ("Abbé", "renvoi", "Abbé Bouteyre", false),
            ("Pelletier", "ce-list", "Pelletier", false),
            ("Nicolo", "wikimedia", "Nicolo", false),
        ] {
            assert_eq!(
                keeps(name, source, incoming, "wikimedia"),
                kept,
                "{name:?} against {incoming:?}"
            );
        }
    }
}
