//! A text that replaced another under the same name, renumbering its articles: which of
//! the two a citation naming no version reads, by the date of the decision that cites it.

use std::sync::LazyLock;

use jiff::civil::Date;

/// From `since` the successor is in force; until `settled` the courts still cite the
/// predecessor by the name both texts share.
const TEXT_SUCCESSIONS_TSV: &str = include_str!("../data/text_successions.tsv");

struct Succession {
    predecessor: &'static str,
    successor: &'static str,
    since: Date,
    settled: Date,
}

static SUCCESSIONS: LazyLock<Vec<Succession>> = LazyLock::new(|| {
    TEXT_SUCCESSIONS_TSV
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let [predecessor, successor, since, settled] = l.split('\t').collect::<Vec<_>>()[..]
            else {
                panic!("text_successions.tsv: predecessor, successor, since, settled")
            };
            Succession {
                predecessor,
                successor,
                since: since.parse().expect("text_successions.tsv: a date"),
                settled: settled.parse().expect("text_successions.tsv: a date"),
            }
        })
        .collect()
});

/// The text a citation of `uid` naming no version reads in a decision of `date`: the
/// predecessor before `since`, none until `settled` or without a date, else `uid`.
pub fn version_on(uid: &str, date: Option<Date>) -> Option<&str> {
    let Some(s) = SUCCESSIONS.iter().find(|s| s.successor == uid) else {
        return Some(uid);
    };
    match date? {
        d if d < s.since => Some(s.predecessor),
        d if d < s.settled => None,
        _ => Some(uid),
    }
}

/// The day a successor replaced `uid`, its rows closed on it.
pub fn replaced_on(uid: &str) -> Option<Date> {
    SUCCESSIONS
        .iter()
        .find(|s| s.predecessor == uid)
        .map(|s| s.since)
}

/// The day `uid` came into force replacing its predecessor.
pub fn in_force_since(uid: &str) -> Option<Date> {
    SUCCESSIONS
        .iter()
        .find(|s| s.successor == uid)
        .map(|s| s.since)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUS_2010: &str = "OHADA/ACTE-UNIFORME-2010-ORGANISATION-DES-SURETES";
    const AUS_1997: &str = "OHADA/ACTE-UNIFORME-1997-ORGANISATION-DES-SURETES";

    #[test]
    fn a_successor_reads_by_the_decision_date() {
        let on = |d: &str| version_on(AUS_2010, Some(d.parse().unwrap()));
        assert_eq!(on("2011-05-15"), Some(AUS_1997));
        assert_eq!(on("2011-05-16"), None);
        assert_eq!(on("2023-05-15"), None);
        assert_eq!(on("2023-05-16"), Some(AUS_2010));
        assert_eq!(version_on(AUS_2010, None), None);
        assert_eq!(version_on(AUS_1997, None), Some(AUS_1997));
    }
}
