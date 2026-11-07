//! Ties the rows of a CNB snapshot without `avCnbfCode` to the registry, by four
//! rules in turn, each a tie only when unique on both sides: bar and name, the
//! namesakes of one bar split by their structure SIREN; a SIREN only this lawyer
//! holds, first name or surname kept; the name alone, on one active identity;
//! else a new `cnb:{barreau}:{nom}-{prenom}`. A row no rule tells apart is left
//! out, and the identities it may stand for are kept as they are.

use std::collections::{HashMap, HashSet};

use lj_store::repository::CnbEntityState;

use super::cnb_history::HistRow;
use super::registries::{slug, AvocatRow};

/// Share of rows tied to known identities under which the reload refuses to
/// write.
pub(super) const MIN_MAPPED: f64 = 0.95;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct BridgeCounts {
    pub(super) exact: usize,
    pub(super) siren: usize,
    pub(super) name: usize,
    pub(super) new: usize,
    pub(super) abstained: usize,
}

impl BridgeCounts {
    pub(super) fn mapped_share(&self) -> f64 {
        let mapped = self.exact + self.siren + self.name;
        let total = mapped + self.new + self.abstained;
        if total == 0 {
            return 0.0;
        }
        mapped as f64 / total as f64
    }
}

pub(super) struct Bridged {
    pub(super) rows: Vec<AvocatRow>,
    /// Identities a row left out may stand for, neither updated nor closed.
    pub(super) kept: HashSet<String>,
    pub(super) counts: BridgeCounts,
}

enum Tie {
    Known(usize),
    Out,
}

fn only(mut it: impl Iterator<Item = usize>) -> Option<usize> {
    let first = it.next()?;
    it.next().is_none().then_some(first)
}

fn legacy_uid(r: &HistRow) -> String {
    format!(
        "cnb:{}:{}-{}",
        r.barreau_slug,
        slug(&r.nom),
        slug(&r.prenom)
    )
}

pub(super) fn bridge(
    rows: Vec<HistRow>,
    existing: &[CnbEntityState],
    open_edges: &[(String, String, Option<chrono::NaiveDate>)],
) -> Bridged {
    let names: Vec<String> = existing.iter().map(|s| slug(&s.denomination)).collect();
    let mut by_key: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_uid: HashMap<&str, usize> = HashMap::new();
    for (i, s) in existing.iter().enumerate() {
        let bar = s.barreau.as_deref().unwrap_or_default();
        by_key.entry((bar, &names[i])).or_default().push(i);
        by_name.entry(&names[i]).or_default().push(i);
        by_uid.insert(&s.uid, i);
    }
    let mut holders: HashMap<&str, Vec<usize>> = HashMap::new();
    for (uid, structure, _) in open_edges {
        if let (Some(&i), Some(siren)) =
            (by_uid.get(uid.as_str()), structure.strip_prefix("siren:"))
        {
            holders.entry(siren).or_default().push(i);
        }
    }

    let keyed: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.barreau_slug.clone(),
                slug(&format!("{} {}", r.prenom, r.nom)),
            )
        })
        .collect();
    let mut per_key: HashMap<(&str, &str), usize> = HashMap::new();
    let mut per_key_siren: HashMap<(&str, &str, &str), usize> = HashMap::new();
    let mut per_name: HashMap<&str, usize> = HashMap::new();
    let mut per_siren: HashMap<&str, usize> = HashMap::new();
    for (r, (bar, name)) in rows.iter().zip(&keyed) {
        *per_key.entry((bar, name)).or_default() += 1;
        *per_name.entry(name).or_default() += 1;
        if let Some(s) = &r.siren {
            *per_key_siren.entry((bar, name, s)).or_default() += 1;
            *per_siren.entry(s).or_default() += 1;
        }
    }

    let mut ties: Vec<Option<Tie>> = (0..rows.len()).map(|_| None).collect();
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut kept: HashSet<usize> = HashSet::new();
    let mut counts = BridgeCounts::default();

    for (i, (r, (bar, name))) in rows.iter().zip(&keyed).enumerate() {
        let key = (bar.as_str(), name.as_str());
        let mut cands = by_key.get(&key).cloned().unwrap_or_default();
        if let Some(&c) = by_uid.get(legacy_uid(r).as_str()) {
            if !cands.contains(&c) {
                cands.push(c);
            }
        }
        let namesakes = per_key[&key] > 1;
        if cands.is_empty() && !namesakes {
            continue;
        }
        let pick = if namesakes {
            r.siren
                .as_deref()
                .filter(|s| per_key_siren[&(bar.as_str(), name.as_str(), *s)] == 1)
                .and_then(|s| holders.get(s))
                .and_then(|h| only(cands.iter().copied().filter(|c| h.contains(c))))
        } else {
            let active: Vec<usize> = cands
                .iter()
                .copied()
                .filter(|&c| existing[c].active)
                .collect();
            if active.is_empty() {
                only(cands.iter().copied())
            } else {
                only(active.into_iter())
            }
        };
        match pick {
            Some(c) if claimed.insert(c) => {
                ties[i] = Some(Tie::Known(c));
                counts.exact += 1;
            }
            _ => {
                ties[i] = Some(Tie::Out);
                kept.extend(cands);
            }
        }
    }

    for (i, r) in rows.iter().enumerate() {
        if ties[i].is_some() {
            continue;
        }
        let Some(s) = r.siren.as_deref().filter(|s| per_siren[s] == 1) else {
            continue;
        };
        let Some(c) = holders.get(s).and_then(|h| only(h.iter().copied())) else {
            continue;
        };
        let (first, surname) = (slug(&r.prenom), slug(&r.nom));
        let same = !first.is_empty()
            && (names[c].starts_with(&format!("{first}-"))
                || names[c].ends_with(&format!("-{surname}")));
        if same && !kept.contains(&c) && claimed.insert(c) {
            ties[i] = Some(Tie::Known(c));
            counts.siren += 1;
        }
    }

    for (i, (_, name)) in keyed.iter().enumerate() {
        if ties[i].is_some() || per_name[name.as_str()] > 1 {
            continue;
        }
        let Some(c) = by_name
            .get(name.as_str())
            .and_then(|h| only(h.iter().copied()))
        else {
            continue;
        };
        if existing[c].active && !kept.contains(&c) && claimed.insert(c) {
            ties[i] = Some(Tie::Known(c));
            counts.name += 1;
        }
    }

    let mut fresh: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for (r, tie) in rows.into_iter().zip(ties) {
        let uid = match tie {
            Some(Tie::Known(c)) => existing[c].uid.clone(),
            Some(Tie::Out) => {
                counts.abstained += 1;
                continue;
            }
            None => {
                let uid = legacy_uid(&r);
                if by_uid.contains_key(uid.as_str()) || !fresh.insert(uid.clone()) {
                    counts.abstained += 1;
                    continue;
                }
                counts.new += 1;
                uid
            }
        };
        out.push(AvocatRow {
            uid,
            barreau: r.barreau,
            barreau_slug: r.barreau_slug,
            nom: r.nom,
            prenom: r.prenom,
            bar_entree: r.serment,
            siren: r.siren,
            date_entree: None,
        });
    }
    Bridged {
        rows: out,
        kept: kept.into_iter().map(|c| existing[c].uid.clone()).collect(),
        counts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(barreau: &str, nom: &str, prenom: &str, siren: Option<&str>) -> HistRow {
        HistRow {
            barreau: barreau.to_string(),
            barreau_slug: slug(barreau),
            nom: nom.to_string(),
            prenom: prenom.to_string(),
            serment: None,
            siren: siren.map(str::to_string),
        }
    }

    fn id(uid: &str, barreau: &str, denomination: &str, active: bool) -> CnbEntityState {
        CnbEntityState {
            uid: uid.to_string(),
            active,
            activity: None,
            barreau: Some(barreau.to_string()),
            denomination: denomination.to_string(),
            surname_key: None,
            forme: None,
        }
    }

    fn edge(uid: &str, siren: &str) -> (String, String, Option<chrono::NaiveDate>) {
        (uid.to_string(), format!("siren:{siren}"), None)
    }

    #[test]
    fn bridge_ties_each_row_once_or_leaves_it_out() {
        let existing = [
            id("cnb:1", "agen", "Louise BERTIN", true),
            id("cnb:2", "paris", "Marie MARTIN", true),
            id("cnb:3", "paris", "Marie MARTIN", true),
            id("cnb:4", "paris", "Paul DURAND", true),
            id("cnb:5", "paris", "Paul DURAND", true),
            id("cnb:6", "ajaccio", "Claire LEROY", true),
            id("cnb:7", "albi", "Jean DUPONT", true),
            id("cnb:8", "bordeaux", "Hélène FAURE", true),
            id("cnb:9", "lyon", "Jean ROUX", true),
            id("cnb:10", "nice", "Jean ROUX", false),
            id("cnb:lille:blanc-anne", "lille", "Anne BLANC", false),
        ];
        let open = [
            edge("cnb:2", "111111111"),
            edge("cnb:3", "222222222"),
            edge("cnb:6", "333333333"),
            edge("cnb:7", "444444444"),
        ];
        let rows = vec![
            row("AGEN", "BERTIN", "Louise", None),
            row("PARIS", "MARTIN", "Marie", Some("222222222")),
            row("PARIS", "MARTIN", "Marie", Some("111111111")),
            row("PARIS", "DURAND", "Paul", None),
            row("PARIS", "DURAND", "Paul", None),
            row("AJACCIO", "LEROY-MOREAU", "Claire", Some("333333333")),
            row("ALBI", "MARTIN", "Paul", Some("444444444")),
            row("CHARENTE", "FAURE", "Hélène", None),
            row("MARSEILLE", "ROUX", "Jean", None),
            row("LILLE", "BLANC", "Anne", None),
        ];
        let b = bridge(rows, &existing, &open);
        let tied: Vec<(&str, &str)> = b
            .rows
            .iter()
            .map(|r| (r.nom.as_str(), r.uid.as_str()))
            .collect();
        assert_eq!(
            tied,
            [
                ("BERTIN", "cnb:1"),
                ("MARTIN", "cnb:3"),
                ("MARTIN", "cnb:2"),
                ("LEROY-MOREAU", "cnb:6"),
                ("MARTIN", "cnb:albi:martin-paul"),
                ("FAURE", "cnb:8"),
                ("ROUX", "cnb:marseille:roux-jean"),
                ("BLANC", "cnb:lille:blanc-anne"),
            ]
        );
        assert_eq!(
            b.counts,
            BridgeCounts {
                exact: 4,
                siren: 1,
                name: 1,
                new: 2,
                abstained: 2
            }
        );
        assert_eq!(
            b.kept,
            HashSet::from(["cnb:4".to_string(), "cnb:5".to_string()])
        );
    }
}
