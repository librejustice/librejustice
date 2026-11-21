//! The formation a CJUE decision names in its title, else in its composition, under
//! the court its CELEX letter names, which a title or a composition must agree with.

use std::sync::LazyLock;

use lj_core::chamber::{ChamberId, Container, Seat, Solemn};
use lj_fold::fold;
use regex::Regex;

use crate::formation::FormationAxes;

/// Characters of the text read: the deepest title measured opens at 7,753.
const HEAD: usize = 8_000;

const ORDINALS: [(&str, u32); 20] = [
    ("premiere", 1),
    ("deuxieme", 2),
    ("troisieme", 3),
    ("quatrieme", 4),
    ("cinquieme", 5),
    ("sixieme", 6),
    ("septieme", 7),
    ("huitieme", 8),
    ("neuvieme", 9),
    ("dixieme", 10),
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
    ("sixth", 6),
    ("seventh", 7),
    ("eighth", 8),
    ("ninth", 9),
    ("tenth", 10),
];

const COURT: &str = r"(?:court(?: of justice| of first instance)?(?: of the european (?:union|communities))?|cour(?: de justice)?|tribunal(?: de la fonction publique| de premiere instance)?(?: de l'union europeenne| des communautes europeennes)?|general court|civil service tribunal(?: of the european union)?)\b";

static TITLE: LazyLock<Regex> = LazyLock::new(|| {
    let ordinal = ordinal_alternation();
    Regex::new(&format!(
        r"^(?:arret|ordonnance|judgment|order)\s+(?:de la|du|of the)\s+(?:(?P<vice>vice-?\s?)?presidente?(?:\s+(?:de la|du|of the)\s+(?:(?P<nth>{ordinal})\s+(?:chambre|chamber)(?:\s+elargie)?(?:\s+(?:du|de la|of the)\s+(?P<its>{COURT}))?|(?P<presides>{COURT})))?|(?P<court>{COURT}))"
    ))
    .expect("regex cjue title")
});

static FORM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\(\s*\*?\s*\w{0,2}\s*\)\s*)*\((?P<form>[^()]{2,90})\)")
        .expect("regex cjue form")
});

static COMPOSITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<court>la cour|le tribunal(?: de premiere instance| de la fonction publique)?(?: de l'union europeenne| des communautes europeennes)?|the court(?: of first instance)?(?: of the european communities)?|the general court|the civil service tribunal(?: of the european union)?)\s*\((?P<form>[^()]{2,90})\)")
        .expect("regex cjue composition")
});

static CHAMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b(?P<nth>{})\s+(?:chambre|chamber)\b",
        ordinal_alternation()
    ))
    .expect("regex cjue chamber")
});

const TITLE_OPENERS: [&str; 4] = ["arret", "ordonnance", "judgment", "order"];

/// What the composition opens on, in capitals: a sentence of the grounds that starts
/// on « La Cour » is no composition.
const COMPOSITION_OPENERS: [&str; 5] = [
    "LA COUR",
    "LE TRIBUNAL",
    "THE COURT",
    "THE GENERAL COURT",
    "THE CIVIL SERVICE TRIBUNAL",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Court {
    Justice,
    General,
    CivilService,
}

fn ordinal_alternation() -> String {
    ORDINALS.map(|(w, _)| w).join("|")
}

fn ordinal(word: &str) -> u32 {
    ORDINALS
        .iter()
        .find(|(w, _)| *w == word)
        .map(|(_, n)| *n)
        .expect("the alternation reads the ordinals of the table")
}

/// `cjue/62021TJ0304`: the letter after the year.
fn court_of_celex(source_uid: &str) -> Option<Court> {
    let celex = source_uid.strip_prefix("cjue/")?;
    match celex.as_bytes() {
        [b'6', _, _, _, _, b'C', ..] => Some(Court::Justice),
        [b'6', _, _, _, _, b'T', ..] => Some(Court::General),
        [b'6', _, _, _, _, b'F', ..] => Some(Court::CivilService),
        _ => None,
    }
}

fn court_named(folded: &str) -> Court {
    if folded.contains("fonction publique") || folded.contains("civil service") {
        Court::CivilService
    } else if folded.starts_with("tribunal")
        || folded.starts_with("le tribunal")
        || folded.contains("general court")
        || folded.contains("first instance")
    {
        Court::General
    } else {
        Court::Justice
    }
}

/// The room a parenthesis names, `None` when it names none.
fn room(form: &str) -> Option<(Seat, Option<&'static str>)> {
    let chamber = CHAMBER
        .captures(form)
        .map(|c| ChamberId::Number(ordinal(&c["nth"])));
    let has = |words: &[&str]| words.iter().any(|w| form.contains(w));
    let formation_uid = if has(&[
        "elargie",
        "extended composition",
        "cinq juges",
        "five judges",
    ]) {
        Some("formation:A_CINQ")
    } else if has(&["juge unique", "single judge"]) {
        Some("formation:JUGE_UNIQUE")
    } else {
        None
    };
    let solemns: [(&[&str], Solemn); 6] = [
        (&["grande chambre", "grand chamber"], Solemn::GrandeChambre),
        (
            &["pleniere", "full court", "plenary"],
            Solemn::AssembleePleniere,
        ),
        (
            &["admission des pourvois", "whether appeals may proceed"],
            Solemn::AdmissionPourvois,
        ),
        (
            &["chambre des pourvois", "appeal chamber"],
            Solemn::ChambrePourvois,
        ),
        (
            &["prejudicielle", "preliminary ruling"],
            Solemn::ChambrePrejudicielle,
        ),
        (
            &["intermediaire", "intermediate chamber"],
            Solemn::ChambreIntermediaire,
        ),
    ];
    let solemn = solemns
        .into_iter()
        .find(|(words, _)| has(words))
        .map(|(_, s)| s)
        .filter(|_| chamber.is_none());
    if chamber.is_none() && solemn.is_none() && formation_uid.is_none() {
        return None;
    }
    Some((
        Seat {
            chamber,
            solemn,
            ..Default::default()
        },
        formation_uid,
    ))
}

fn in_title(lines: &[&str], court: Court) -> Option<(Seat, Option<&'static str>)> {
    for (i, line) in lines.iter().enumerate() {
        let opener = fold(line);
        if !TITLE_OPENERS.iter().any(|w| opener.starts_with(w)) {
            continue;
        }
        let folded = fold(&lines[i..(i + 3).min(lines.len())].join(" "));
        let Some(title) = TITLE.captures(&folded) else {
            continue;
        };
        let named = ["court", "its", "presides"]
            .into_iter()
            .find_map(|g| title.name(g));
        if named.is_some_and(|n| court_named(n.as_str()) != court) {
            continue;
        }
        if title.name("court").is_none() {
            return Some(match title.name("nth") {
                Some(nth) => (
                    Seat {
                        chamber: Some(ChamberId::Number(ordinal(nth.as_str()))),
                        ..Default::default()
                    },
                    Some("formation:JUGE_UNIQUE"),
                ),
                None => (
                    Seat {
                        solemn: Some(match title.name("vice") {
                            Some(_) => Solemn::VicePresident,
                            None => Solemn::President,
                        }),
                        ..Default::default()
                    },
                    None,
                ),
            });
        }
        let rest = &folded[title.get(0).expect("whole match").end()..];
        if let Some(found) = FORM.captures(rest).and_then(|f| room(&f["form"])) {
            return Some(found);
        }
    }
    None
}

fn in_composition(lines: &[&str], court: Court) -> Option<(Seat, Option<&'static str>)> {
    lines
        .iter()
        .filter(|l| COMPOSITION_OPENERS.iter().any(|o| l.starts_with(o)))
        .find_map(|line| {
            let folded = fold(line);
            let c = COMPOSITION.captures(&folded)?;
            (court_named(&c["court"]) == court)
                .then(|| room(&c["form"]))
                .flatten()
        })
}

/// The seat and the formation a CJUE decision's text names in its head.
pub(super) fn cjue_formation(text: &str, source_uid: &str) -> FormationAxes {
    let Some(court) = court_of_celex(source_uid) else {
        return FormationAxes::default();
    };
    let mut end = HEAD.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lines: Vec<&str> = text[..end]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let (mut seat, formation_uid) = in_title(&lines, court)
        .or_else(|| in_composition(&lines, court))
        .unwrap_or_default();
    seat.container = match court {
        Court::Justice => None,
        Court::General => Some(Container::Tribunal),
        Court::CivilService => Some(Container::TribunalFonctionPublique),
    };
    FormationAxes {
        chamber: seat,
        formation_uid,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_names_the_formation_and_the_celex_the_court() {
        let cases = [
            (
                "cjue/62024CJ0095",
                "ARRÊT DE LA COUR (cinquième chambre)\n21 mai 2026 (*)\n« Renvoi préjudiciel »",
                Some("5e chambre"),
                None,
            ),
            (
                "cjue/62021TJ0304",
                "ARRÊT DU TRIBUNAL (quatrième chambre élargie)\n12 juillet 2023",
                Some("Tribunal, 4e chambre"),
                Some("formation:A_CINQ"),
            ),
            (
                "cjue/62022TO0010",
                "ORDONNANCE DU TRIBUNAL (quatrième chambre, siégeant avec cinq juges)",
                Some("Tribunal, 4e chambre"),
                Some("formation:A_CINQ"),
            ),
            (
                "cjue/62012FJ0083",
                "ARRÊT DU TRIBUNAL DE LA FONCTION PUBLIQUE\nDE L\u{2019}UNION EUROPÉENNE (troisième chambre)\n12 février 2014 (*)",
                Some("Tribunal de la fonction publique, 3e chambre"),
                None,
            ),
            (
                "cjue/62014CO0431",
                "ORDONNANCE DE LA VICE-PRÉSIDENTE DE LA COUR\n3 juin 2015",
                Some("Vice-président"),
                None,
            ),
            (
                "cjue/62010TO0080",
                "ORDONNANCE DU PRÉSIDENT DU TRIBUNAL\n8 avril 2010",
                Some("Tribunal, Président"),
                None,
            ),
            (
                "cjue/61968CO0027",
                "ORDONNANCE DU PRÉSIDENT DE LA PREMIÈRE CHAMBRE\n10 juillet 1968",
                Some("1re chambre"),
                Some("formation:JUGE_UNIQUE"),
            ),
            (
                "cjue/62025CO0665",
                "ORDONNANCE DE LA COUR (chambre d\u{2019}admission des pourvois)",
                Some("Chambre d'admission des pourvois"),
                None,
            ),
            (
                "cjue/62025TO0790",
                "ORDONNANCE DU TRIBUNAL (juge unique)",
                Some("Tribunal"),
                Some("formation:JUGE_UNIQUE"),
            ),
            (
                "cjue/61997TO0172(01)",
                "Avis juridique important\n|\n61997B0172(01)\nOrder of the Court of First Instance (Fourth Chamber) of 28 January 1998. - Camar Srl v Commission.",
                Some("Tribunal, 4e chambre"),
                None,
            ),
            (
                "cjue/61999TJ0065",
                "Arrêt du Tribunal\nAffaire T-65/99\nStrintzis Lines Shipping SA\ncontre\nCommission\n«Concurrence»\nArrêt du Tribunal (cinquième chambre) du 11 décembre 2003\nSommaire de l'arrêt",
                Some("Tribunal, 5e chambre"),
                None,
            ),
            (
                "cjue/62019CJ0001",
                "JUDGMENT OF THE COURT (Grand Chamber)\n2 March 2021",
                Some("Grande chambre"),
                None,
            ),
            (
                "cjue/62005CJ0017",
                "ARRÊT DE LA COUR\n12 septembre 2006\nDans l\u{2019}affaire C-17/05,\nLA COUR (grande chambre),\ncomposée de M. V. Skouris, président",
                Some("Grande chambre"),
                None,
            ),
            (
                "cjue/62008CJ0012",
                "ARRÊT DE LA COUR\n15 mars 2010\nayant pour objet un pourvoi\nArrêt du Tribunal (troisième chambre) du 4 mai 2007",
                None,
                None,
            ),
            (
                "cjue/61982CJ0160",
                "Avis juridique important\n|\n61982J0160\nArrêt de la cour du 15 décembre 1982. - Commission contre Pays-Bas.",
                None,
                None,
            ),
        ];
        for (source_uid, text, label, formation_uid) in cases {
            let axes = cjue_formation(text, source_uid);
            assert_eq!(axes.chamber.label().as_deref(), label, "{text}");
            assert_eq!(axes.formation_uid, formation_uid, "{text}");
        }
    }
}
