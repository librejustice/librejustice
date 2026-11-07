//! What a query writes around the decision it cites: the court, the day, the
//! words of a reference. Pure; the resolver reads it, the projections too.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use lj_fold::fold;

/// Court names folded as [`crate::case_names::case_name_key`] folds a query,
/// longest first, under the `jurisdiction_type` of the fonds. The seat of a
/// court that has one is read by the caller against the referential.
const COURT_NAMES: &[(&str, &str)] = &[
    ("CJUE", "cour de justice des communautes europeennes"),
    ("CJUE", "cour de justice de l union europeenne"),
    ("CEDH", "cour europeenne des droits de l homme"),
    (
        "CNIL",
        "commission nationale de l informatique et des libertes",
    ),
    ("TCOM", "tribunal des activites economiques"),
    ("CNDA", "cour nationale du droit d asile"),
    ("CAA", "cour administrative d appel"),
    ("TJ", "tribunal de grande instance"),
    ("TCOM", "tribunal de commerce"),
    ("TC", "tribunal des conflits"),
    ("CONSTIT", "conseil constitutionnel"),
    ("TA", "tribunal administratif"),
    ("TJ", "tribunal judiciaire"),
    ("CC", "cour de cassation"),
    ("CE", "conseil d etat"),
    ("CA", "cour d appel"),
    ("CC", "cassation"),
    ("CJUE", "cjce"),
    ("CJUE", "cjue"),
    ("CEDH", "cedh"),
    ("CNDA", "cnda"),
    ("CNIL", "cnil"),
    ("TCOM", "tcom"),
    ("CC", "cass"),
    ("CAA", "caa"),
    ("TJ", "tgi"),
    ("TCOM", "tae"),
    ("CE", "ce"),
    ("CA", "ca"),
    ("TA", "ta"),
    ("TJ", "tj"),
];

/// Formations and the words a court name is made of, folded. One of them
/// beside a name or a number makes the query a citation.
const COURT_WORDS: &[&str] = &[
    "conseil",
    "etat",
    "cour",
    "tribunal",
    "administratif",
    "administrative",
    "appel",
    "judiciaire",
    "commerce",
    "constitutionnel",
    "tue",
    "assemblee",
    "ass",
    "section",
    "sect",
    "pleniere",
    "plen",
    "chambre",
    "ch",
    "civ",
    "com",
    "soc",
    "crim",
    "mixte",
];

/// What a citation carries besides its court: the words of the reference, the
/// day and its month, the articles.
const CITATION_WORDS: &[&str] = &[
    "arret",
    "arrets",
    "dit",
    "dite",
    "dits",
    "dites",
    "decision",
    "jugement",
    "ordonnance",
    "avis",
    "jurisprudence",
    "affaire",
    "req",
    "n",
    "no",
    "numero",
    "rg",
    "pourvoi",
    "c",
    "contre",
    "v",
    "vs",
    "de",
    "du",
    "des",
    "d",
    "l",
    "la",
    "le",
    "les",
    "et",
    "en",
    "au",
    "lecture",
    "audience",
    "lundi",
    "mardi",
    "mercredi",
    "jeudi",
    "vendredi",
    "samedi",
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

const MONTHS: [&str; 12] = [
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

fn is_citation_word(word: &str, seats: &HashSet<String>) -> bool {
    let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let ordinal = !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && matches!(
            &word[digits.len()..],
            "" | "e" | "er" | "re" | "eme" | "ere"
        );
    let roman = word.chars().all(|c| matches!(c, 'i' | 'v' | 'x'));
    ordinal
        || roman
        || COURT_WORDS.contains(&word)
        || CITATION_WORDS.contains(&word)
        || seats.contains(word)
}

/// The court a folded word sequence names, with the words outside its name.
/// `None` when no court name is written; a lone court word (« section »,
/// « chambre ») is not a name.
pub fn strip_court<'a>(words: &[&'a str]) -> Option<(&'static str, Vec<&'a str>)> {
    for (kind, name) in COURT_NAMES {
        let name: Vec<&str> = name.split(' ').collect();
        if name.len() > words.len() {
            continue;
        }
        if let Some(i) =
            (0..=words.len() - name.len()).find(|&i| words[i..i + name.len()] == name[..])
        {
            let rest = words[..i]
                .iter()
                .chain(&words[i + name.len()..])
                .copied()
                .collect();
            return Some((kind, rest));
        }
    }
    None
}

/// The words around a designation cite it and ask nothing else: a court is
/// named, and every other word is a citation word, a seat, a number, an
/// ordinal or a roman numeral.
pub fn remainder_cites(words: &[&str], seats: &HashSet<String>) -> bool {
    let (named, rest) = match strip_court(words) {
        Some((_, rest)) => (true, rest),
        None => (false, words.to_vec()),
    };
    (named || rest.iter().any(|w| COURT_WORDS.contains(w)))
        && rest.iter().all(|w| is_citation_word(w, seats))
}

/// The words that designate on their own, without a court beside them.
const DESIGNATING_WORDS: &[&str] = &[
    "arret",
    "arrets",
    "jurisprudence",
    "affaire",
    "dit",
    "dite",
    "dits",
    "dites",
    "c",
    "contre",
    "v",
    "vs",
];

/// The words a name must hold, and whether the query writes a court or a
/// designating word. A seat counts unless a court is named: « Cassis de Dijon ».
pub fn distinctive_words<'a>(words: &[&'a str], seats: &HashSet<String>) -> (Vec<&'a str>, bool) {
    let (court, rest) = match strip_court(words) {
        Some((_, rest)) => (true, rest),
        None => (false, words.to_vec()),
    };
    let no_seat = HashSet::new();
    let seats = if court { seats } else { &no_seat };
    let designates = court
        || rest
            .iter()
            .any(|w| COURT_WORDS.contains(w) || DESIGNATING_WORDS.contains(w));
    let distinctive = rest
        .into_iter()
        .filter(|w| !w.is_empty() && !is_citation_word(w, seats))
        .collect();
    (distinctive, designates)
}

const GLUE_WORDS: &[&str] = &["de", "du", "des", "d", "l", "la", "le", "les", "en", "et"];

/// A key glued by an article to a distinctive word outside it is a piece of
/// another name: « cassis » in « Cassis de Dijon », « loos » in « Van Gend en Loos ».
pub fn key_is_glued(words: &[&str], key: &str, distinctive: &[&str]) -> bool {
    let key: Vec<&str> = key.split(' ').collect();
    let Some(start) = (0..words.len().saturating_sub(key.len() - 1))
        .find(|&i| words[i..i + key.len()] == key[..])
    else {
        return false;
    };
    let end = start + key.len();
    let glued = |glue: Option<&&str>, word: Option<&&str>| {
        glue.is_some_and(|g| GLUE_WORDS.contains(g))
            && word.is_some_and(|w| distinctive.contains(w))
    };
    (start >= 2 && glued(words.get(start - 1), words.get(start - 2)))
        || glued(words.get(end), words.get(end + 1))
}

/// The words of a case name key that distinguish it: « commune beziers » for
/// « commune de beziers ii ».
pub fn name_words(key: &str) -> Vec<&str> {
    let none = HashSet::new();
    key.split(' ')
        .filter(|w| !w.is_empty() && !is_citation_word(w, &none))
        .collect()
}

/// Whether these words only point at the name beside them: « l'arrêt », « dite ».
pub fn only_designates(words: &[&str]) -> bool {
    words
        .iter()
        .all(|w| DESIGNATING_WORDS.contains(w) || GLUE_WORDS.contains(w))
}

/// The words of a folded query outside `key`, one of its contiguous n-grams.
/// Every word when the key is not there.
pub fn words_around<'a>(words: &[&'a str], key: &str) -> Vec<&'a str> {
    let cle: Vec<&str> = key.split(' ').collect();
    let debut = (0..words.len().saturating_sub(cle.len() - 1))
        .find(|&i| words[i..i + cle.len()] == cle[..]);
    match debut {
        Some(i) => words[..i]
            .iter()
            .chain(&words[i + cle.len()..])
            .copied()
            .collect(),
        None => words.to_vec(),
    }
}

/// A day written in full: `28 décembre 2009`, `1er avril 2025`, `17/07/2024`,
/// `2024-07-17`. A year alone is not a day.
static DAY_RE: LazyLock<Regex> = LazyLock::new(|| {
    use crate::docket_key::SLASH;
    Regex::new(&[
        r"\b(?:(?P<d1>1er|\d{1,2}) (?P<m1>janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) (?P<y1>\d{4})",
        &format!(r"|(?P<d2>\d{{1,2}}){SLASH}(?P<m2>\d{{1,2}}){SLASH}(?P<y2>\d{{4}})"),
        r"|(?P<y3>\d{4})-(?P<m3>\d{2})-(?P<d3>\d{2}))\b",
    ]
    .concat())
    .unwrap()
});

/// The first full day the query writes, as ISO `YYYY-MM-DD`, when it exists in
/// the calendar.
pub fn query_day(query: &str) -> Option<String> {
    let folded = fold(query);
    let caps = DAY_RE.captures(&folded)?;
    let (day, month, year) = if caps.name("d1").is_some() {
        let month = MONTHS.iter().position(|m| *m == &caps["m1"])? as u32 + 1;
        (
            caps["d1"].trim_end_matches("er").parse::<u32>().ok()?,
            month,
            caps["y1"].parse::<i32>().ok()?,
        )
    } else if caps.name("d2").is_some() {
        (
            caps["d2"].parse().ok()?,
            caps["m2"].parse().ok()?,
            caps["y2"].parse().ok()?,
        )
    } else {
        (
            caps["d3"].parse().ok()?,
            caps["m3"].parse().ok()?,
            caps["y3"].parse().ok()?,
        )
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    ((1..=days).contains(&day) && (1678..=2262).contains(&year))
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// The query cites a court and a full day and asks nothing else: it designates
/// the decisions of that court on that day.
pub fn cites_a_day(query: &str, seats: &HashSet<String>) -> bool {
    if query_day(query).is_none() {
        return false;
    }
    let folded = crate::case_names::case_name_key(query);
    let words: Vec<&str> = folded.split(' ').collect();
    strip_court(&words).is_some() && remainder_cites(&words, seats)
}

/// Whether a folded word sequence names a court.
pub fn names_a_court(words: &[&str]) -> bool {
    strip_court(words).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case_names::case_name_key;

    fn seats(cities: &[&str]) -> HashSet<String> {
        cities.iter().map(|c| c.to_string()).collect()
    }

    fn autour(q: &str, cle: &str, sieges: &HashSet<String>) -> bool {
        let plie = case_name_key(q);
        let mots: Vec<&str> = plie.split(' ').collect();
        remainder_cites(&words_around(&mots, cle), sieges)
    }

    fn distinctive(q: &str, cities: &[&str]) -> (Vec<String>, bool) {
        let folded = case_name_key(q);
        let words: Vec<&str> = folded.split(' ').collect();
        let (distinctive, designates) = distinctive_words(&words, &seats(cities));
        let distinctive = distinctive.into_iter().map(str::to_string).collect();
        (distinctive, designates)
    }

    #[test]
    fn a_name_is_read_from_the_words_beside_the_citation() {
        for (q, words, designates) in [
            ("arrêt Nicolo", vec!["nicolo"], true),
            ("Nicolo", vec!["nicolo"], false),
            ("CE, Ass., 20 octobre 1989, Nicolo", vec!["nicolo"], true),
            ("jurisprudence Tropic", vec!["tropic"], true),
            ("arrêts dits Jacques Vabre", vec!["jacques", "vabre"], true),
            ("arrêt Cassis de Dijon", vec!["cassis", "dijon"], true),
            ("Marckx c. Belgique", vec!["marckx", "belgique"], true),
            ("Béziers II", vec!["beziers"], false),
            (
                "Danthony consultation irrégulière",
                vec!["danthony", "consultation", "irreguliere"],
                false,
            ),
        ] {
            assert_eq!(
                distinctive(q, &[]),
                (words.iter().map(|w| w.to_string()).collect(), designates),
                "query {q:?}"
            );
        }
    }

    #[test]
    fn a_seat_distinguishes_unless_a_court_is_named() {
        let dijon = &["dijon"];
        assert_eq!(
            distinctive("Cassis de Dijon", dijon).0,
            vec!["cassis", "dijon"]
        );
        assert_eq!(
            distinctive("CA Dijon Lamotte", dijon),
            (vec!["lamotte".to_string()], true)
        );
    }

    #[test]
    fn a_key_glued_to_another_distinctive_word_is_a_piece_of_another_name() {
        for (q, key, glued) in [
            ("arrêt Cassis de Dijon", "cassis", true),
            ("arrêt Van Gend en Loos", "loos", true),
            ("arrêt Van Gend en Loos", "van gend en loos", false),
            ("Commune de Cassis", "commune de cassis", false),
            ("arrêt Cassis", "cassis", false),
            ("arrêt Blanco de 1873", "blanco", false),
        ] {
            let folded = case_name_key(q);
            let words: Vec<&str> = folded.split(' ').collect();
            let (distinctive, _) = distinctive_words(&words, &seats(&[]));
            assert_eq!(
                key_is_glued(&words, key, &distinctive),
                glued,
                "query {q:?}, key {key:?}"
            );
        }
    }

    #[test]
    fn la_juridiction_autour_du_nom_en_fait_une_citation() {
        let aucun = seats(&[]);
        for (q, cle, cite) in [
            ("Béziers conseil d'État", "beziers", true),
            ("Béziers CE 2001", "beziers", true),
            (
                "CE, Sect., 28 décembre 2009, Commune de Béziers",
                "commune de beziers",
                true,
            ),
            ("Cass. civ. 1re, 3 mars 2021, Durand", "durand", true),
            ("Béziers II", "beziers", false),
            ("Béziers 2001", "beziers", false),
            ("arrêt Béziers", "beziers", false),
            ("Béziers", "beziers", false),
            ("Béziers conseil municipal", "beziers", false),
            (
                "Béziers conseil d'État résiliation du contrat",
                "beziers",
                false,
            ),
        ] {
            assert_eq!(autour(q, cle, &aucun), cite, "requête {q:?}");
        }
    }

    #[test]
    fn le_siege_de_la_cour_est_un_mot_de_citation() {
        let marseille = seats(&["marseille"]);
        assert!(autour("Béziers CAA Marseille", "beziers", &marseille));
        assert!(!autour("Béziers CAA Marseille", "beziers", &seats(&[])));
        assert!(!autour("Béziers Marseille", "beziers", &marseille));
    }

    #[test]
    fn le_nom_de_la_cour_se_lit_en_entier() {
        let mots =
            |q: &str| -> Vec<String> { case_name_key(q).split(' ').map(str::to_string).collect() };
        for (q, kind, rest) in [
            ("cour d'appel de Toulouse", "CA", vec!["de", "toulouse"]),
            ("CAA Paris", "CAA", vec!["paris"]),
            (
                "cour administrative d'appel de Paris",
                "CAA",
                vec!["de", "paris"],
            ),
            ("Cass. soc.", "CC", vec!["soc"]),
            ("conseil d'État section", "CE", vec!["section"]),
            ("tribunal judiciaire de Paris", "TJ", vec!["de", "paris"]),
        ] {
            let m = mots(q);
            let m: Vec<&str> = m.iter().map(String::as_str).collect();
            let (k, r) = strip_court(&m).unwrap_or_else(|| panic!("{q}"));
            assert_eq!((k, r), (kind, rest), "requête {q:?}");
        }
        for q in ["section", "chambre sociale", "bail commercial"] {
            let m = mots(q);
            let m: Vec<&str> = m.iter().map(String::as_str).collect();
            assert!(strip_court(&m).is_none(), "requête {q:?}");
        }
    }

    #[test]
    fn le_jour_ecrit_en_entier_se_lit() {
        for (q, jour) in [
            ("cour d'appel de toulouse 14 mai 1997", Some("1997-05-14")),
            (
                "TJ Versailles, 1er juin 2024, n° 24/01234",
                Some("2024-06-01"),
            ),
            ("décision du 15/06/2021 sur le permis", Some("2021-06-15")),
            ("2021-07-01", Some("2021-07-01")),
            ("29 février 2024", Some("2024-02-29")),
            ("29 février 2023", None),
            ("conseil d'état 1920", None),
            ("Béziers 2001", None),
            ("2 mai au 30 juin 2026", Some("2026-06-30")),
            ("Cour d'appel de Rennes 28/02/ 2020", Some("2020-02-28")),
            ("ordonnance du 07 / 03 / 2023", Some("2023-03-07")),
        ] {
            assert_eq!(query_day(q).as_deref(), jour, "requête {q:?}");
        }
    }

    #[test]
    fn la_cour_et_le_jour_designent_sans_nom_ni_numero() {
        let sieges = seats(&["toulouse", "paris", "nimes"]);
        for (q, cite) in [
            ("cour d'appel de toulouse 14 mai 1997", true),
            ("caa paris 5 février 2024", true),
            ("tribunal judiciaire de paris 20 octobre 2023", true),
            ("CE, Sect., 28 décembre 2009", true),
            ("Conseil d'État 2023 permis de construire", false),
            ("conseil d'état 1920", false),
            (
                "cour d'appel nîmes 9 mars 2021 heures supplémentaires",
                false,
            ),
            ("loi du 26 janvier 2024 immigration", false),
            ("14 mai 1997", false),
        ] {
            assert_eq!(cites_a_day(q, &sieges), cite, "requête {q:?}");
        }
    }
}
