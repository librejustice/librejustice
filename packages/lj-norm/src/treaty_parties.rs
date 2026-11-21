//! The states a treaty France published binds, read from its title and the
//! keywords of the decree that publishes it. Pure, no I/O.

use std::sync::LazyLock;

use jiff::civil::Date;
use lj_fold::fold;

/// A `*` suffix makes the last word a stem, a `-` suffix a combining form read
/// only in a compound; a dissolved state has a four-letter code.
const STATE_NAMES_TSV: &str = include_str!("../data/state_names.tsv");

/// A dissolved state's successors, each reading the treaties it concluded before `from`.
const STATE_SUCCESSIONS_TSV: &str = include_str!("../data/state_successions.tsv");

struct StateName {
    words: Vec<String>,
    stem: bool,
    combining: bool,
    code: &'static str,
}

struct Succession {
    state: &'static str,
    successor: &'static str,
    from: Date,
}

static NAMES: LazyLock<Vec<StateName>> = LazyLock::new(|| {
    rows(STATE_NAMES_TSV)
        .map(|f| {
            let [code, name] = f[..] else {
                panic!("state_names.tsv: code, name")
            };
            let folded = fold(name.trim_end_matches(['*', '-']));
            StateName {
                words: words(&folded).into_iter().map(str::to_string).collect(),
                stem: name.ends_with('*'),
                combining: name.ends_with('-'),
                code,
            }
        })
        .collect()
});

static SUCCESSIONS: LazyLock<Vec<Succession>> = LazyLock::new(|| {
    rows(STATE_SUCCESSIONS_TSV)
        .map(|f| {
            let [state, successor, from] = f[..] else {
                panic!("state_successions.tsv: state, successor, from")
            };
            Succession {
                state,
                successor,
                from: from.parse().expect("state_successions.tsv: a date"),
            }
        })
        .collect()
});

fn rows(tsv: &'static str) -> impl Iterator<Item = Vec<&'static str>> {
    tsv.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').collect())
}

const FRANCE: &[&[&str]] = &[
    &["republique", "francaise"],
    &["republique", "de", "france"],
    &["la", "france"],
    &["gouvernement", "francais"],
    &["etat", "francais"],
];

/// Where the name of the other party ends.
const STOPS: &[&str] = &[
    "a",
    "afin",
    "ainsi",
    "amendant",
    "au",
    "aux",
    "completant",
    "concernant",
    "conclu",
    "conclue",
    "conclus",
    "creant",
    "dans",
    "en",
    "etablissant",
    "fait",
    "faite",
    "faites",
    "faits",
    "fixant",
    "instituant",
    "modifiant",
    "portant",
    "pour",
    "prorogeant",
    "reglant",
    "relatif",
    "relatifs",
    "relative",
    "relatives",
    "signe",
    "signee",
    "signees",
    "signes",
    "sous",
    "sur",
    "tendant",
    "visant",
];

const DETERMINERS: &[&str] = &["des", "l", "la", "le", "les", "un", "une"];

/// A party named in the plural is several states.
const PLURALS: &[&str] = &[
    "divers",
    "diverses",
    "etats",
    "gouvernements",
    "membres",
    "pays",
    "plusieurs",
    "republiques",
];

/// Heads that name an organisation, whatever state its name carries.
const ORGANISATION_HEADS: &[&str] = &[
    "agence",
    "aiea",
    "association",
    "banque",
    "bureau",
    "centre",
    "cern",
    "comite",
    "commission",
    "conference",
    "cour",
    "esa",
    "eumetsat",
    "fao",
    "fondation",
    "fonds",
    "force",
    "haut",
    "institut",
    "interpol",
    "laboratoire",
    "ligue",
    "observatoire",
    "ocde",
    "office",
    "oit",
    "oms",
    "ompi",
    "onu",
    "ordre",
    "organisation",
    "otan",
    "programme",
    "reseau",
    "secretariat",
    "societe",
    "systeme",
    "tribunal",
    "unesco",
    "universite",
];

/// Words that name an organisation once no state is named.
const ORGANISATION_WORDS: &[&str] = &["autorite", "communaute", "conseil", "union", "unies"];
const ORGANISATION_STEMS: &[&str] = &[
    "europeen",
    "intergouvernementa",
    "internationa",
    "mondia",
    "multilatera",
];

/// The ISO codes of the states that read today a treaty of `date`, France first;
/// `FR` alone with an organisation, `None` when unknown, a multilateral treaty
/// or an unnamed party, or a dissolved state's treaty without a date.
pub fn parties(title: &str, keywords: &[String], date: Option<Date>) -> Option<Vec<&'static str>> {
    let folded = fold(title);
    let words = words(&folded);
    let other = match other_party(&words) {
        Some(other) => named_party(other, keywords)?,
        None => compound_party(&words)?,
    };
    let Some(other) = other else {
        return Some(vec!["FR"]);
    };
    let mut readers = Vec::new();
    reading_states(other, date, &mut readers)?;
    readers.sort_unstable();
    readers.dedup();
    (!readers.is_empty()).then(|| std::iter::once("FR").chain(readers).collect())
}

/// `state` itself when it still exists, else the successors that took over a
/// treaty it concluded on `date`, each in turn.
fn reading_states(
    state: &'static str,
    date: Option<Date>,
    out: &mut Vec<&'static str>,
) -> Option<()> {
    let mut lines = SUCCESSIONS.iter().filter(|s| s.state == state).peekable();
    if lines.peek().is_none() {
        out.push(state);
        return Some(());
    }
    let date = date?;
    for line in lines.filter(|s| s.from > date) {
        reading_states(line.successor, Some(date), out)?;
    }
    Some(())
}

/// The state France's other party is, `Some(None)` for an organisation; the
/// keywords give it only when the title names it as no state.
fn named_party(other: &[&str], keywords: &[String]) -> Option<Option<&'static str>> {
    if let Some(head) = other.iter().position(|w| !DETERMINERS.contains(w)) {
        if PLURALS.contains(&other[head]) && name_at(&other[head..]).is_none() {
            return None;
        }
        if ORGANISATION_HEADS.contains(&other[head]) {
            return Some(None);
        }
    }
    match states(other).as_slice() {
        [one] => Some(Some(one)),
        [] if other.iter().any(|w| names_organisation(w)) => Some(None),
        [] => match keyword_states(keywords).as_slice() {
            [one] => Some(Some(one)),
            _ => None,
        },
        _ => None,
    }
}

fn names_organisation(word: &str) -> bool {
    ORGANISATION_HEADS.contains(&word)
        || ORGANISATION_WORDS.contains(&word)
        || ORGANISATION_STEMS.iter().any(|s| word.starts_with(s))
}

/// The other party of `accord franco-tunisien`, or of `convention vietnamo-française`.
fn compound_party(words: &[&str]) -> Option<Option<&'static str>> {
    let mut found: Vec<&'static str> = Vec::new();
    if let Some(at) = words.iter().position(|w| *w == "franco") {
        let mut left = at;
        while let Some(code) = left.checked_sub(1).and_then(|l| combining(words[l])) {
            found.push(code);
            left -= 1;
        }
        let mut next = at + 1;
        while let Some(code) = words.get(next).and_then(|w| combining(w)) {
            found.push(code);
            next += 1;
        }
        while let Some((len, code)) = name_at(&words[next..]) {
            found.push(code);
            next += len;
        }
    } else if let Some(code) = words
        .windows(2)
        .find(|w| w[1].starts_with("francais"))
        .and_then(|w| combining(w[0]))
    {
        found.push(code);
    }
    found.dedup();
    match found.as_slice() {
        [one] => Some(Some(one)),
        _ => None,
    }
}

fn combining(word: &str) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|n| n.combining && n.words == [word])
        .map(|n| n.code)
}

/// The words naming the other party, after `France et` when France opens the
/// list of parties, else between `entre` and `et France`.
fn other_party<'w>(words: &'w [&'w str]) -> Option<&'w [&'w str]> {
    let france: Vec<(usize, usize)> = (0..words.len())
        .filter_map(|i| {
            FRANCE
                .iter()
                .find(|f| words[i..].starts_with(f))
                .map(|f| (i, f.len()))
        })
        .collect();
    for &(at, len) in &france {
        let mut next = at + len;
        if words[next..].starts_with(&[",", "d", "une", "part", ","]) {
            next += 5;
        } else if words.get(next) == Some(&",") {
            next += 1;
        }
        if words.get(next) == Some(&"et") && opens_parties(words, at) {
            let start = next + 1;
            let end = words[start..]
                .iter()
                .position(|w| STOPS.contains(w) || is_punctuation(w))
                .map_or(words.len(), |p| start + p);
            return Some(&words[start..end]);
        }
    }
    france
        .iter()
        .find_map(|&(at, len)| named_before(words, at, len))
}

/// Nothing between the `entre` before France and France is a comma or a state.
fn opens_parties(words: &[&str], at: usize) -> bool {
    let from = words[..at]
        .iter()
        .rposition(|w| *w == "entre")
        .map_or(0, |e| e + 1);
    let before = &words[from..at];
    !before.contains(&",") && states(before).is_empty()
}

/// `entre <party> et le Gouvernement de la République française`, France last.
fn named_before<'w>(words: &'w [&'w str], at: usize, len: usize) -> Option<&'w [&'w str]> {
    if words.get(at + len) == Some(&"et") {
        return None;
    }
    let mut and = at;
    while and > 0 && matches!(words[and - 1], "de" | "gouvernement" | "l" | "la" | "le") {
        and -= 1;
    }
    if and == 0 || words[and - 1] != "et" {
        return None;
    }
    let and = and - 1;
    let open = words[..and].iter().rposition(|w| *w == "entre")? + 1;
    let mut party = &words[open..and];
    if let Some(stripped) = party.strip_suffix(&[",", "d", "une", "part", ","]) {
        party = stripped;
    }
    (!party.iter().any(|w| is_punctuation(w))).then_some(party)
}

fn is_punctuation(word: &str) -> bool {
    matches!(word, "," | ";" | "(" | ")")
}

/// The distinct states `words` name, the longest name read at each word.
fn states(words: &[&str]) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    let mut at = 0;
    while at < words.len() {
        match name_at(&words[at..]) {
            Some((len, code)) => {
                if !found.contains(&code) {
                    found.push(code);
                }
                at += len;
            }
            None => at += 1,
        }
    }
    found
}

/// The distinct states of the keywords that are a state's name whole.
fn keyword_states(keywords: &[String]) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    for keyword in keywords {
        let folded = fold(keyword);
        let words = words(&folded);
        if let Some((len, code)) = name_at(&words) {
            if len == words.len() && !found.contains(&code) {
                found.push(code);
            }
        }
    }
    found
}

/// The longest name of a state `words` opens with: its length in words, its code.
fn name_at(words: &[&str]) -> Option<(usize, &'static str)> {
    NAMES
        .iter()
        .filter(|n| !n.combining && n.words.len() <= words.len())
        .filter(|n| {
            let (last, rest) = n.words.split_last().expect("a name has a word");
            let tail = words[rest.len()];
            rest.iter().zip(words).all(|(a, b)| a == b)
                && match n.stem {
                    true => tail.starts_with(last.as_str()),
                    false => tail == last,
                }
        })
        .max_by_key(|n| n.words.len())
        .map(|n| (n.words.len(), n.code))
}

/// The words of a folded text, and the punctuation that ends a name.
fn words(folded: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in folded.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
            continue;
        }
        if let Some(s) = start.take() {
            out.push(&folded[s..i]);
        }
        if matches!(c, ',' | ';' | '(' | ')') {
            out.push(&folded[i..i + 1]);
        }
    }
    if let Some(s) = start {
        out.push(&folded[s..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parties;

    /// A title, its decree's keywords, the parties read.
    type Case = (
        &'static str,
        &'static [&'static str],
        Option<&'static [&'static str]>,
    );

    /// A title, its date, the parties read.
    type DatedCase = (
        &'static str,
        Option<&'static str>,
        Option<&'static [&'static str]>,
    );

    #[test]
    fn a_treaty_lists_france_and_its_other_party() {
        let cases: &[Case] = &[
            (
                "Accord entre le Gouvernement de la République française et le Gouvernement de la République tunisienne relatif au transport aérien, signé à Tunis le 18 mars 1999",
                &[],
                Some(&["FR", "TN"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et le Conseil fédéral suisse relatif à la réadmission des personnes en situation irrégulière, signé à Berne le 28 octobre 1998",
                &[],
                Some(&["FR", "CH"]),
            ),
            (
                "CONVENTION FRANCO-LAOTIENNE D'ENTRAIDE JUDICIAIRE ET D'ETABLISSEMENT D'UNE PROCEDURE D'EXEQUATUR SIMPLIFIEE DU 16 NOVEMBRE 1956",
                &[],
                Some(&["FR", "LA"]),
            ),
            (
                "CONVENTION ENTRE LE GOUVERNEMENT DE LA REPUBLIQUE DE HAUTE-VOLTA ET LE GOUVERNEMENT DE LA REPUBLIQUE FRANCAISE RELATIVE AUX DEPOTS ET CONSIGNATIONS EFFECTUES EN HAUTE-VOLTA",
                &[],
                Some(&["FR", "BF"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et le Gouvernement de la République fédérative du Brésil relatif à la construction d'un pont routier sur le fleuve Oyapock reliant la Guyane française et l'Etat de l'Amapa",
                &[],
                Some(&["FR", "BR"]),
            ),
            (
                "entente entre le Gouvernement de la République française et le Gouvernement du Québec en matière de mobilité étudiante au niveau universitaire",
                &[],
                Some(&["FR", "CA"]),
            ),
            (
                "ECHANGE DE NOTES FRANCO-BELGE DU 05-03-1982 CONCERNANT LA CREATION D'UN BUREAU A CONTROLES NATIONAUX JUXTAPOSES A MONT-SAINT-MARTIN-AUBANGE",
                &[],
                Some(&["FR", "BE"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et l'Organisation européenne pour la recherche nucléaire relatif au statut juridique de ladite organisation en France",
                &[],
                Some(&["FR"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et le Gouvernement royal relatif aux transports aériens",
                &["ACCORD INTERNATIONAL", "FRANCE", "CAMBODGE"],
                Some(&["FR", "KH"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et le Gouvernement royal relatif aux transports aériens",
                &["ACCORD INTERNATIONAL", "RESSORTISSANT CAMBODGIEN"],
                None,
            ),
            (
                "Convention sur la circulation routière, signée à Genève le 19 septembre 1949",
                &["ACCORD INTERNATIONAL", "SUISSE"],
                None,
            ),
            (
                "Convention belgo-franco-luxembourgeoise relative à l'exploitation des chemins de fer luxembourgeois",
                &[],
                None,
            ),
            (
                "Accord entre le Gouvernement du Royaume de Belgique, le Gouvernement de la République française et le Gouvernement du Royaume-Uni de Grande-Bretagne et d'Irlande du Nord concernant la circulation des trains",
                &[],
                None,
            ),
        ];
        for (title, keywords, expected) in cases {
            let keywords: Vec<String> = keywords.iter().map(|k| k.to_string()).collect();
            assert_eq!(
                parties(title, &keywords, None).as_deref(),
                *expected,
                "{title}"
            );
        }
    }

    #[test]
    fn a_dissolved_state_s_treaty_is_read_by_the_successors_it_predates() {
        let cases: &[DatedCase] = &[
            (
                "CONVENTION VETERINAIRE, DES DEUX PROTOCOLES ET DES ANNEXES, ENTRE LA FRANCE ET LA TCHECOSLOVAQUIE DU 30 MAI 1967",
                Some("1967-05-30"),
                Some(&["FR", "CZ", "SK"]),
            ),
            (
                "ACCORD ENTRE LA FRANCE ET LA YOUGOSLAVIE REGLEMENTANT L'EMPLOI DES TRAVAILLEURS YOUGOSLAVES EN FRANCE DU 25 JANVIER 1965",
                Some("1965-01-25"),
                Some(&["FR", "BA", "HR", "ME", "MK", "RS", "SI"]),
            ),
            (
                "Accord entre le Gouvernement de la République française et le Gouvernement de la République fédérale de Yougoslavie sur l'encouragement et la protection réciproques des investissements",
                Some("1997-03-28"),
                Some(&["FR", "ME", "RS"]),
            ),
            (
                "ACCORDS PARTICULIERS SIGNES LE 22 JUIN 1960 ENTRE LA REPUBLIQUE FRANCAISE ET LA FEDERATION DU MALI",
                Some("1960-06-22"),
                Some(&["FR", "ML", "SN"]),
            ),
            (
                "Accord de coopération culturelle entre le Gouvernement de la République française et le Gouvernement de l'Union des républiques socialistes soviétiques",
                Some("1966-06-30"),
                Some(&["FR", "RU"]),
            ),
            (
                "ACCORD FRANCO-YOUGOSLAVE DU 2 août 1958 SUR LE REGLEMENT DES CREANCES FRANCAISES",
                None,
                None,
            ),
        ];
        for (title, date, expected) in cases {
            let date = date.map(|d| d.parse().unwrap());
            assert_eq!(parties(title, &[], date).as_deref(), *expected, "{title}");
        }
    }
}
