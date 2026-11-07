//! Which of the decisions sharing an incoming record's address is its own: the
//! ECLI first, then what the court publishes of its ruling, then the text.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

/// What a record says of its decision beyond its address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Traits {
    pub ecli: Option<String>,
    pub publication_codes: Vec<String>,
    /// The order and court roots included (`FR`, `FR.CC`, `FR.CC.C2_CIVILE`).
    pub seat_paths: Vec<String>,
    pub solution_uid: Option<String>,
    /// ISO dates of the decisions it attacks or appeals.
    pub attacked_dates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    One(usize),
    /// Nothing but the text tells these apart.
    Tied(Vec<usize>),
    Neither,
}

/// Two texts at this identity or above are one decision, when the
/// discriminants leave several candidates.
pub const TIED_TEXT_IDENTITY: f64 = 0.99;

/// The plenary assembly and the plenary formation are one bench.
pub fn ecli_key(ecli: &str) -> String {
    ecli.trim().to_ascii_uppercase().replace(":AP", ":PL")
}

/// An equal ECLI decides, a different one discards the candidate, and the
/// candidates left are ranked on publication, chamber, solution and attacked
/// decision.
pub fn choose(incoming: &Traits, candidates: &[Traits]) -> Choice {
    let mut pool: Vec<usize> = (0..candidates.len()).collect();
    if let Some(key) = incoming.ecli.as_deref().map(ecli_key) {
        let same = |i: &usize| candidates[*i].ecli.as_deref().map(ecli_key) == Some(key.clone());
        if let Some(i) = pool.iter().copied().find(|i| same(i)) {
            return Choice::One(i);
        }
        pool.retain(|&i| candidates[i].ecli.is_none());
    }
    let Some(best) = pool.iter().map(|&i| score(incoming, &candidates[i])).max() else {
        return Choice::Neither;
    };
    pool.retain(|&i| score(incoming, &candidates[i]) == best);
    match pool.as_slice() {
        [i] => Choice::One(*i),
        _ => Choice::Tied(pool),
    }
}

fn score(a: &Traits, b: &Traits) -> u8 {
    u8::from(publication_agrees(a, b))
        + u8::from(chamber_agrees(a, b))
        + u8::from(solution_agrees(a, b))
        + u8::from(
            a.attacked_dates
                .iter()
                .any(|d| b.attacked_dates.contains(d)),
        )
}

fn publication_agrees(a: &Traits, b: &Traits) -> bool {
    let unpublished = |t: &Traits| t.publication_codes.iter().any(|c| c == "n");
    a.publication_codes.is_empty()
        || b.publication_codes.is_empty()
        || unpublished(a) == unpublished(b)
}

fn chamber_agrees(a: &Traits, b: &Traits) -> bool {
    let (ra, rb) = (rooms(a), rooms(b));
    ra.is_empty()
        || rb.is_empty()
        || ra
            .iter()
            .any(|p| rb.iter().any(|q| nested(p, q) || nested(q, p)))
}

fn rooms(t: &Traits) -> Vec<&str> {
    t.seat_paths
        .iter()
        .map(String::as_str)
        .filter(|p| p.matches('.').count() >= 2)
        .collect()
}

fn solution_agrees(a: &Traits, b: &Traits) -> bool {
    match (a.solution_uid.as_deref(), b.solution_uid.as_deref()) {
        (Some(x), Some(y)) => nested(x, y) || nested(y, x),
        _ => true,
    }
}

/// `inner` is `outer` or one of its refinements (`solution:CASSATION`,
/// `solution:CASSATION_PARTIELLE`).
fn nested(outer: &str, inner: &str) -> bool {
    inner
        .strip_prefix(outer)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '_']))
}

/// The lesser of the two shares of word 6-grams each text finds in the other.
pub fn text_identity(a: &str, b: &str) -> f64 {
    let (ga, gb) = (grams(a, 6, 1), grams(b, 6, 1));
    cover(&ga, &gb).min(cover(&gb, &ga))
}

/// The share of the shorter text's word 5-grams found in the longer one, the
/// one-letter words of anonymisation marks ignored.
pub fn text_containment(a: &str, b: &str) -> f64 {
    let (ga, gb) = (grams(a, 5, 2), grams(b, 5, 2));
    if ga.len() <= gb.len() {
        cover(&ga, &gb)
    } else {
        cover(&gb, &ga)
    }
}

fn cover(of: &HashSet<u64>, inside: &HashSet<u64>) -> f64 {
    of.intersection(inside).count() as f64 / of.len().max(1) as f64
}

fn grams(text: &str, n: usize, min_word: usize) -> HashSet<u64> {
    let folded = lj_fold::slug(text, ' ');
    let words: Vec<&str> = folded.split(' ').filter(|w| w.len() >= min_word).collect();
    words
        .windows(n)
        .map(|w| {
            let mut h = DefaultHasher::new();
            w.hash(&mut h);
            h.finish()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traits(ecli: Option<&str>, solution: Option<&str>) -> Traits {
        Traits {
            ecli: ecli.map(str::to_string),
            solution_uid: solution.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn an_equal_ecli_decides_and_the_plenary_benches_are_one() {
        let incoming = traits(Some("ECLI:FR:CCASS:2020:AP00123"), None);
        let cands = [
            traits(None, None),
            traits(Some("ecli:fr:ccass:2020:PL00123"), None),
        ];
        assert_eq!(choose(&incoming, &cands), Choice::One(1));
    }

    #[test]
    fn a_different_ecli_discards_and_none_left_is_neither() {
        let incoming = traits(Some("ECLI:FR:CCASS:2019:C200695"), None);
        let cands = [traits(Some("ECLI:FR:CCASS:2019:C200694"), None)];
        assert_eq!(choose(&incoming, &cands), Choice::Neither);
        let cands = [
            traits(Some("ECLI:FR:CCASS:2019:C200694"), None),
            traits(None, None),
        ];
        assert_eq!(choose(&incoming, &cands), Choice::One(1));
    }

    #[test]
    fn the_discriminants_rank_and_a_tie_goes_to_the_text() {
        let incoming = Traits {
            publication_codes: vec!["n".into()],
            seat_paths: vec!["FR".into(), "FR.CC".into(), "FR.CC.C2_CIVILE".into()],
            solution_uid: Some("solution:CASSATION".into()),
            ..Default::default()
        };
        let room = |p: &str, sol: &str| Traits {
            publication_codes: vec!["n".into()],
            seat_paths: vec!["FR".into(), "FR.CC".into(), p.into()],
            solution_uid: Some(sol.into()),
            ..Default::default()
        };
        let cands = [
            room("FR.CC.C1_CIVILE", "solution:CASSATION"),
            room("FR.CC.C2_CIVILE", "solution:CASSATION_PARTIELLE"),
            room("FR.CC.C2_CIVILE", "solution:REJET"),
        ];
        assert_eq!(choose(&incoming, &cands), Choice::One(1));
        let cands = [
            room("FR.CC.C2_CIVILE", "solution:CASSATION"),
            room("FR.CC.C2_CIVILE", "solution:CASSATION"),
        ];
        assert_eq!(choose(&incoming, &cands), Choice::Tied(vec![0, 1]));
    }

    #[test]
    fn text_measures() {
        let a = "La Cour, sur le moyen unique : attendu que la cour d'appel a retenu à bon droit que le bail était résilié ; rejette le pourvoi";
        assert_eq!(text_identity(a, a), 1.0);
        let longer = format!("{a}. Condamne M. X... aux dépens, et vu l'article 700 du code de procédure civile, rejette la demande");
        assert_eq!(text_containment(a, &longer), 1.0);
        assert!(text_identity(a, &longer) < 0.99);
        assert!(
            text_containment(
                a,
                "Tout autre texte sans aucun rapport avec le premier, ni de près ni de loin"
            ) < 0.1
        );
    }
}
