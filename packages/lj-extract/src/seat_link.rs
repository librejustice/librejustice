//! Naming a room is an assertion about a whole court's output. Poitiers writes
//! « 1ère Chambre » at the registry and « 1ère chambre civile » in the body,
//! Besançon writes the same registry field and three different things in the
//! body: the two are indistinguishable decision by decision, and only what the
//! rest of the court wrote separates them.
//!
//! The registry is the court's own key for a room, and where it names the
//! matter it names the room: Caen writes « 2ème Chambre civile » on every
//! decision of the chamber whose arrêts print « deuxième chambre civile et
//! commerciale » and whose ordonnances print « 2ème chambre civile », one room
//! under two graphies of the same court. The vocabulary speaks only where the
//! registry is silent.
//!
//! The administrative registry keys its rooms the other way round. A tribunal's
//! chamber is a number whose matters an arrêté sets, and what the key appends to
//! it is the sitting, not the room: TA Paris writes « 1re Section - 1re Chambre »
//! and « 1re section - 1re Chambre - OQTF 6 sem. » for the one chamber that
//! hears foreigners' cases like every chamber of the tribunal does, where Melun
//! writes « 12ème chambre, éloignement » on every decision of the chamber it
//! dedicates to it. So a number the registry also writes bare is the room, and
//! the matter names a numbered chamber only when every form of the key carries
//! it. A tribunal judiciaire keys its cabinets the same way, « Ch4.3 JCP » and
//! « 4ème chambre civile » under one number at Grenoble, and a key that says
//! more than the number without naming a matter is such a sitting; « Deuxième
//! Chambre » beside « 2ème Ch Civile Cab 1 » at Colmar is the short spelling of
//! the civil chamber, as at a cour d'appel.
//!
//! A court also renames a room. Melun keyed « Reconduite à la frontière » until
//! the month it keyed « 12ème chambre, éloignement », Colmar keyed nothing on
//! its chambre sociale until it keyed « Chambre 4 A », Orléans keys « Chambre
//! Commerciale » where the body of 84 decisions wrote « 2e chambre commerciale ».
//! The room a matter names alone and the single numbered room carrying that
//! matter are one room when their months succeed each other, or when the
//! registry keys only one of the two and the court leaves no number bare; the
//! later form names it, or the keyed one.
//!
//! The snapshot is built on the seat as WRITTEN, never on its resolution: fed
//! its own output it would confirm yesterday's reading, and a room made
//! nameless by one aberrant decision could never get its name back.

use std::collections::{HashMap, HashSet};

use lj_core::chamber::{ChamberId, Container, Seat};

/// What a court's output says its rooms are called, one entry per room, and the
/// rooms it folds into one another.
#[derive(Debug, Default, Clone)]
pub struct SeatVocabulary {
    rooms: HashMap<String, Room>,
    folds: HashMap<String, Fold>,
}

/// The name of one room, resolved by the cases in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Room {
    /// The registry writes several matters under one number: as many rooms,
    /// told apart by the registry alone, so a seat it left silent keeps what
    /// its body wrote.
    Split,
    /// The registry writes the number bare as well as suffixed: the number is
    /// the room and the suffix a sitting, so every seat of the room drops the
    /// matter, the one that carries it included.
    General,
    /// One room, called this. Empty means the matters contradict each other and
    /// the segment stays bare.
    Named(Vec<&'static str>),
}

/// Two rooms of one court that are one: where a seat of the first goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fold {
    /// The room a matter names alone joins the numbered room, under its number
    /// and its name.
    Numbered {
        container: Option<Container>,
        chamber: ChamberId,
        specialty: Vec<&'static str>,
    },
    /// The numbered room joins the room its matter names alone.
    Unnumbered(Vec<&'static str>),
}

/// One row of the corpus feeding the snapshot.
#[derive(Debug, Clone)]
pub struct SeatRow {
    pub seat: Seat,
    /// Registry field `chamber`, verbatim. At the Cassation it holds the source
    /// code (`civ1`), which names the matter as a code.
    pub chamber: Option<String>,
    /// Registry field `formation`, verbatim: the only one the administrative
    /// order fills.
    pub formation: Option<String>,
    /// First and last month the corpus wrote this seat under these keys, as
    /// `year * 12 + month`. The snapshot reads it; a decision being resolved
    /// carries none.
    pub span: Option<(i32, i32)>,
}

/// What the snapshot knows of a room besides its name: a seat in it, whether
/// the registry ever keyed it, and the months it spans.
#[derive(Debug, Clone)]
struct RoomFacts {
    seat: Seat,
    keyed: bool,
    span: Option<(i32, i32)>,
}

impl RoomFacts {
    fn new(seat: &Seat) -> Self {
        RoomFacts {
            seat: seat.clone(),
            keyed: false,
            span: None,
        }
    }

    fn absorb(&mut self, keyed: bool, span: Option<(i32, i32)>) {
        self.keyed |= keyed;
        if let Some((first, last)) = span {
            self.span = Some(match self.span {
                Some((a, b)) => (a.min(first), b.max(last)),
                None => (first, last),
            });
        }
    }
}

/// The named numbered rooms of a court, and whether it leaves a number bare.
#[derive(Debug, Default)]
struct Court {
    named: Vec<(String, Vec<&'static str>, Vec<&'static str>, RoomFacts)>,
    bare: bool,
}

impl SeatVocabulary {
    /// `rows` = the corpus, deduplicated: the seat as written, and the registry
    /// fields the decision carried.
    pub fn new(rows: impl IntoIterator<Item = SeatRow>) -> Self {
        let mut registry: HashMap<String, HashSet<Vec<&'static str>>> = HashMap::new();
        let mut seen: HashMap<String, HashSet<Vec<&'static str>>> = HashMap::new();
        let mut numbered: HashMap<String, RoomFacts> = HashMap::new();
        let mut unnumbered: HashMap<String, RoomFacts> = HashMap::new();
        for row in rows {
            let jt = row.seat.jurisdiction_type.as_str();
            let from_registry =
                registry_matters(jt, row.chamber.as_deref(), row.formation.as_deref());
            let Some(key) = room_key(&row.seat) else {
                let mut named = row.seat.clone();
                if let Some(matters) = from_registry.as_ref().filter(|m| !m.is_empty()) {
                    named.specialty = matters.clone();
                }
                if named.specialty.is_empty() {
                    continue;
                }
                let Some(path) = named.paths().pop() else {
                    continue;
                };
                unnumbered
                    .entry(path)
                    .or_insert_with(|| RoomFacts::new(&named))
                    .absorb(from_registry.is_some(), row.span);
                continue;
            };
            numbered
                .entry(key.clone())
                .or_insert_with(|| RoomFacts::new(&row.seat))
                .absorb(from_registry.is_some(), row.span);
            if !row.seat.specialty.is_empty() {
                seen.entry(key.clone())
                    .or_default()
                    .insert(row.seat.specialty.clone());
            }
            let entry = registry.entry(key).or_default();
            match from_registry {
                Some(matters) if !matters.is_empty() => {
                    entry.insert(matters);
                }
                Some(_)
                    if bare_key_is_a_sitting(
                        jt,
                        row.chamber.as_deref(),
                        row.formation.as_deref(),
                    ) =>
                {
                    entry.insert(Vec::new());
                }
                _ => {}
            }
        }
        let rooms: HashMap<String, Room> = registry
            .into_iter()
            .map(|(key, from_registry)| {
                let seen = seen.remove(&key).unwrap_or_default();
                (key, name_room(from_registry, seen))
            })
            .collect();
        let folds = fold_rooms(&rooms, &numbered, &unnumbered);
        SeatVocabulary { rooms, folds }
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// The seat its court would name. A registry that names the matter names
    /// the room, whatever the body added to it, unless its court writes that
    /// number bare as well; the vocabulary speaks where the registry is silent;
    /// a room the court folded into another goes there; and a room the snapshot
    /// never saw is left alone, which is what the first pass of a corpus sees.
    pub fn resolve(&self, row: &SeatRow) -> Seat {
        let mut resolved = row.seat.clone();
        let key = room_key(&row.seat);
        let room = key.as_deref().and_then(|k| self.rooms.get(k));
        if room == Some(&Room::General) {
            resolved.specialty.clear();
            return resolved;
        }
        let from_registry = registry_matters(
            row.seat.jurisdiction_type.as_str(),
            row.chamber.as_deref(),
            row.formation.as_deref(),
        )
        .unwrap_or_default();
        if !from_registry.is_empty() {
            resolved.specialty = from_registry;
        } else if let Some(Room::Named(name)) = room {
            resolved.specialty = name.clone();
        }
        let fold = match &key {
            Some(k) => self.folds.get(k),
            None => resolved.paths().pop().and_then(|p| self.folds.get(&p)),
        };
        match fold {
            Some(Fold::Numbered {
                container,
                chamber,
                specialty,
            }) => {
                resolved.container = *container;
                resolved.chamber = Some(*chamber);
                resolved.specialty = specialty.clone();
            }
            Some(Fold::Unnumbered(specialty)) => {
                resolved.container = None;
                resolved.chamber = None;
                resolved.specialty = specialty.clone();
            }
            None => {}
        }
        resolved
    }
}

/// The room a seat sits in, matters stripped: a court, its container, its
/// number. `None` when no number identifies the room, the matter being then
/// the room's own name.
fn room_key(seat: &Seat) -> Option<String> {
    seat.chamber.as_ref()?;
    let mut bare = seat.clone();
    bare.specialty.clear();
    bare.paths().pop()
}

/// The court of a room path: its first three segments, order, type and court.
fn court_of(path: &str) -> String {
    path.splitn(4, '.').take(3).collect::<Vec<_>>().join(".")
}

/// The matters the last segment of a room path carries, as the path derivation
/// folded them: `C4_SOCIALE` and `SOCIALE` both carry the social matter.
fn room_matters(path: &str) -> Option<Vec<&'static str>> {
    let segment = path.rsplit('.').next()?;
    let is_chamber_head = |head: &str| {
        head.strip_prefix('C').is_some_and(|id| {
            (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit() || c == '-'))
                || (id.len() == 1 && id.chars().all(|c| c.is_ascii_uppercase()))
        })
    };
    let glued = match segment.split_once('_') {
        Some((head, rest)) if is_chamber_head(head) => rest,
        _ => segment,
    };
    lj_core::chamber::read_matters(glued)
}

/// The matters the REGISTRY names, read by the reader the extraction runs, minus
/// the banner: the banner is the body, and the cases turn on what the registry
/// alone distinguishes. `None` when the registry wrote no key at all.
fn registry_matters(
    jurisdiction_type: &str,
    chamber: Option<&str>,
    formation: Option<&str>,
) -> Option<Vec<&'static str>> {
    use crate::formation::parse_formation;
    let chamber = chamber.filter(|c| !c.trim().is_empty());
    let formation = formation.filter(|f| !f.trim().is_empty());
    chamber.or(formation)?;
    let jt = Some(jurisdiction_type);
    let axes = match jurisdiction_type {
        "TA" | "CAA" | "CE" => parse_formation(jt, None, None, None, formation),
        "CC" => parse_formation(jt, chamber, None, None, formation),
        "CA" | "TJ" | "TCOM" | "CPH" => parse_formation(jt, None, chamber, None, formation),
        _ => return None,
    };
    Some(axes.chamber.specialty)
}

/// Where the registry keys sittings under a number, a key that names no matter
/// is one of them and the number is the room: every key of the administrative
/// order, and a key of a tribunal judiciaire that says more than the number.
fn bare_key_is_a_sitting(
    jurisdiction_type: &str,
    chamber: Option<&str>,
    formation: Option<&str>,
) -> bool {
    match jurisdiction_type {
        "TA" | "CAA" | "CE" => true,
        "TJ" => chamber
            .or(formation)
            .is_some_and(crate::formation::says_more_than_the_number),
        _ => false,
    }
}

/// The cases of the rule, in order, without a single threshold.
fn name_room(from_registry: HashSet<Vec<&'static str>>, seen: HashSet<Vec<&'static str>>) -> Room {
    let mut registry: Vec<Vec<&'static str>> = from_registry.into_iter().collect();
    if let Some(bare) = registry.iter().position(Vec::is_empty) {
        registry.swap_remove(bare);
        if !registry.is_empty() {
            return Room::General;
        }
    }
    if registry.len() > 1 {
        return Room::Split;
    }
    if let Some(only) = registry.pop() {
        return Room::Named(only);
    }
    let mut composed: Vec<Vec<&'static str>> =
        seen.iter().filter(|s| s.len() > 1).cloned().collect();
    if composed.len() == 1 {
        return Room::Named(composed.pop().expect("one composed matter"));
    }
    if composed.is_empty() && seen.len() == 1 {
        return Room::Named(seen.into_iter().next().expect("one matter"));
    }
    Room::Named(Vec::new())
}

/// In one court, the room a matter names alone and the single numbered room
/// carrying that matter are one room when their months succeed each other, the
/// later form naming it, or when the registry keys only one of the two and the
/// court leaves no number bare, the keyed form naming it.
fn fold_rooms(
    rooms: &HashMap<String, Room>,
    numbered: &HashMap<String, RoomFacts>,
    unnumbered: &HashMap<String, RoomFacts>,
) -> HashMap<String, Fold> {
    let mut courts: HashMap<String, Court> = HashMap::new();
    for (key, facts) in numbered {
        let court = courts.entry(court_of(key)).or_default();
        match rooms.get(key) {
            Some(Room::Named(name)) if !name.is_empty() => {
                let mut named = facts.seat.clone();
                named.specialty = name.clone();
                let matters = named
                    .paths()
                    .pop()
                    .and_then(|p| room_matters(&p))
                    .unwrap_or_default();
                court
                    .named
                    .push((key.clone(), matters, name.clone(), facts.clone()));
            }
            Some(Room::Split) => {}
            _ => court.bare = true,
        }
    }
    let mut folds = HashMap::new();
    for (path, facts) in unnumbered {
        let Some(court) = courts.get(&court_of(path)) else {
            continue;
        };
        let Some(matters) = room_matters(path) else {
            continue;
        };
        let mut candidates = court
            .named
            .iter()
            .filter(|(_, carried, _, _)| matters.iter().all(|m| carried.contains(m)));
        let (Some((key, _, name, room)), None) = (candidates.next(), candidates.next()) else {
            continue;
        };
        let toward_numbered = match (facts.span, room.span) {
            (Some((_, last)), Some((first, _))) if last <= first => true,
            (Some((first, _)), Some((_, last))) if last <= first => false,
            _ if !court.bare && facts.keyed != room.keyed => room.keyed,
            _ => continue,
        };
        if toward_numbered {
            folds.insert(
                path.clone(),
                Fold::Numbered {
                    container: room.seat.container,
                    chamber: room.seat.chamber.expect("a numbered room"),
                    specialty: name.clone(),
                },
            );
        } else {
            folds.insert(key.clone(), Fold::Unnumbered(matters));
        }
    }
    folds
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The referential's own `&'static str`, which is what a seat holds.
    fn matters(names: &[&str]) -> Vec<&'static str> {
        names
            .iter()
            .map(|n| {
                lj_core::chamber::static_specialty(&format!("matiere:{n}")).expect("a known matter")
            })
            .collect()
    }

    fn seat(code: &str, number: u32, specialty: &[&str]) -> Seat {
        Seat {
            jurisdiction_type: if code == "cc" { "CC" } else { "CA" }.to_string(),
            jurisdiction_code: Some(code.to_string()),
            chamber: Some(ChamberId::Number(number)),
            specialty: matters(specialty),
            ..Default::default()
        }
    }

    fn row(seat: Seat, chamber: &str) -> SeatRow {
        SeatRow {
            seat,
            chamber: Some(chamber.to_string()),
            formation: None,
            span: None,
        }
    }

    /// A tribunal's row: the registry writes `Formation_Jugement` and nothing else.
    fn ta_row(
        code: &str,
        section: Option<u32>,
        number: Option<u32>,
        specialty: &[&str],
        formation: &str,
    ) -> SeatRow {
        SeatRow {
            seat: Seat {
                jurisdiction_type: "TA".to_string(),
                jurisdiction_code: Some(code.to_string()),
                container: section.map(Container::Section),
                chamber: number.map(ChamberId::Number),
                specialty: matters(specialty),
                ..Default::default()
            },
            chamber: None,
            formation: Some(formation.to_string()),
            span: None,
        }
    }

    /// A row of a tribunal judiciaire, keyed at the registry's `chamber`.
    fn tj_row(code: &str, number: Option<u32>, specialty: &[&str], chamber: &str) -> SeatRow {
        SeatRow {
            seat: Seat {
                jurisdiction_type: "TJ".to_string(),
                jurisdiction_code: Some(code.to_string()),
                chamber: number.map(ChamberId::Number),
                specialty: matters(specialty),
                ..Default::default()
            },
            chamber: Some(chamber.to_string()),
            formation: None,
            span: None,
        }
    }

    /// A row whose registry wrote nothing.
    fn silent(mut row: SeatRow) -> SeatRow {
        row.chamber = None;
        row.formation = None;
        row
    }

    /// The months of a row, `(year, month)` to `(year, month)`.
    fn dated(mut row: SeatRow, first: (i32, u32), last: (i32, u32)) -> SeatRow {
        let month = |(y, m): (i32, u32)| y * 12 + m as i32 - 1;
        row.span = Some((month(first), month(last)));
        row
    }

    /// The five courts of the rule, plus the Cassation, whose registry field names
    /// the matter as a code.
    #[test]
    fn the_courts_of_the_rule_name_their_rooms() {
        let caen = SeatVocabulary::new([
            row(seat("ca_caen", 2, &["CIVILE"]), "2ème Chambre civile"),
            row(seat("ca_caen", 2, &["SOCIALE"]), "2ème chambre sociale"),
        ]);
        // The arrêts print « deuxième chambre civile et commerciale », the
        // ordonnances « 2ème chambre civile »: one registry key for both.
        for written in [vec!["CIVILE", "COMMERCIALE"], vec!["CIVILE"], vec![]] {
            assert_eq!(
                caen.resolve(&row(seat("ca_caen", 2, &written), "2ème Chambre civile"))
                    .specialty,
                matters(&["CIVILE"])
            );
        }
        assert_eq!(
            caen.resolve(&row(
                seat("ca_caen", 2, &["SOCIALE"]),
                "2ème chambre sociale"
            ))
            .specialty,
            matters(&["SOCIALE"])
        );
        // Where the registry is silent, the body stands alone: Montpellier's
        // « 3ème chambre correctionnelle » has no registry field at all.
        assert_eq!(
            caen.resolve(&row(seat("ca_caen", 2, &["CORRECTIONNELLE"]), ""))
                .specialty,
            matters(&["CORRECTIONNELLE"])
        );
        assert!(caen
            .resolve(&row(seat("ca_caen", 2, &[]), ""))
            .specialty
            .is_empty());

        let montpellier = SeatVocabulary::new([
            row(seat("ca_montpellier", 2, &["CIVILE"]), "2e chambre civile"),
            row(
                seat("ca_montpellier", 2, &["SOCIALE"]),
                "2e chambre sociale",
            ),
            row(
                seat("ca_montpellier", 2, &["FAMILLE"]),
                "2e chambre de la famille",
            ),
        ]);
        for (matter, label) in [
            ("CIVILE", "2e chambre civile"),
            ("SOCIALE", "2e chambre sociale"),
            ("FAMILLE", "2e chambre de la famille"),
        ] {
            assert_eq!(
                montpellier
                    .resolve(&row(seat("ca_montpellier", 2, &[matter]), label))
                    .specialty,
                matters(&[matter])
            );
        }

        let besancon = SeatVocabulary::new([
            row(
                seat("ca_besancon", 1, &["CIVILE", "COMMERCIALE"]),
                "1ère Chambre",
            ),
            row(seat("ca_besancon", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_besancon", 1, &[]), "1ère Chambre"),
        ]);
        for written in [vec!["CIVILE", "COMMERCIALE"], vec!["CIVILE"], vec![]] {
            assert_eq!(
                besancon
                    .resolve(&row(seat("ca_besancon", 1, &written), "1ère Chambre"))
                    .specialty,
                matters(&["CIVILE", "COMMERCIALE"]),
                "the composed matter names the whole room"
            );
        }

        let poitiers = SeatVocabulary::new([
            row(seat("ca_poitiers", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_poitiers", 1, &[]), "1ère Chambre"),
        ]);
        assert_eq!(
            poitiers
                .resolve(&row(seat("ca_poitiers", 1, &[]), "1ère Chambre"))
                .specialty,
            matters(&["CIVILE"])
        );

        let pau = SeatVocabulary::new([
            row(seat("ca_pau", 1, &[]), "1ère Chambre"),
            row(seat("ca_pau", 1, &["CORRECTIONNELLE"]), "1ère Chambre"),
            row(seat("ca_pau", 1, &["CIVILE"]), "1ère Chambre"),
        ]);
        for written in [vec![], vec!["CORRECTIONNELLE"], vec!["CIVILE"]] {
            assert!(
                pau.resolve(&row(seat("ca_pau", 1, &written), "1ère Chambre"))
                    .specialty
                    .is_empty(),
                "contradictory matters without a composed one leave the segment bare"
            );
        }

        let cc = SeatVocabulary::new([
            row(seat("cc", 1, &["CIVILE"]), "civ1"),
            row(seat("cc", 2, &["CIVILE"]), "civ2"),
            row(seat("cc", 3, &["CIVILE"]), "civ3"),
        ]);
        for n in [1, 2, 3] {
            assert_eq!(
                cc.resolve(&row(seat("cc", n, &[]), &format!("civ{n}")))
                    .specialty,
                matters(&["CIVILE"]),
                "chamber {n} keeps its matter"
            );
        }
    }

    /// The registry keys of TA Paris and TA Melun: the tribunal's
    /// own site says every chamber of Paris hears foreigners' cases and Melun
    /// dedicates a single-judge chamber to removals.
    #[test]
    fn the_administrative_number_is_the_room_and_the_suffix_a_sitting() {
        let vocab = SeatVocabulary::new([
            ta_row(
                "ta_paris",
                Some(1),
                Some(1),
                &[],
                "1re Section - 1re Chambre",
            ),
            ta_row(
                "ta_paris",
                Some(1),
                Some(1),
                &["ETRANGERS"],
                "1re section - 1re Chambre - OQTF 6 sem.",
            ),
            ta_row(
                "ta_paris",
                Some(1),
                Some(1),
                &[],
                "1re Section - 1re Chambre - R.222-13",
            ),
            ta_row(
                "ta_melun",
                None,
                Some(12),
                &["ETRANGERS"],
                "12ème chambre, éloignement",
            ),
            ta_row(
                "ta_melun",
                None,
                Some(12),
                &["ETRANGERS"],
                "12ème chambre, éloignement (Collégiale)",
            ),
            ta_row("ta_melun", None, Some(1), &[], "1ère chambre"),
            ta_row("ta_melun", None, Some(1), &[], "1ère chambre, JU"),
        ]);
        assert!(
            vocab
                .resolve(&ta_row(
                    "ta_paris",
                    Some(1),
                    Some(1),
                    &["ETRANGERS"],
                    "1re section - 1re Chambre - OQTF 6 sem.",
                ))
                .specialty
                .is_empty(),
            "the general chamber sits on the OQTF too"
        );
        assert!(vocab
            .resolve(&ta_row(
                "ta_paris",
                Some(1),
                Some(1),
                &[],
                "1re Section - 1re Chambre"
            ))
            .specialty
            .is_empty());
        assert_eq!(
            vocab
                .resolve(&ta_row(
                    "ta_melun",
                    None,
                    Some(12),
                    &[],
                    "12ème chambre, éloignement (Collégiale)",
                ))
                .specialty,
            matters(&["ETRANGERS"]),
            "a chamber the registry never writes without its matter is dedicated to it"
        );
        assert!(vocab
            .resolve(&ta_row("ta_melun", None, Some(1), &[], "1ère chambre, JU"))
            .specialty
            .is_empty());

        // The judicial bare form is a short spelling of the same room: Basse-Terre
        // writes both « 1ère Chambre » and « 1ère chambre civile » for its one civil chamber.
        let basse_terre = SeatVocabulary::new([
            row(seat("ca_basse_terre", 1, &[]), "1ère Chambre"),
            row(
                seat("ca_basse_terre", 1, &["CIVILE"]),
                "1ère chambre civile",
            ),
        ]);
        assert_eq!(
            basse_terre
                .resolve(&row(seat("ca_basse_terre", 1, &[]), "1ère Chambre"))
                .specialty,
            matters(&["CIVILE"])
        );
    }

    /// The keys of TJ Grenoble and TJ Colmar: cabinets under a
    /// number at Grenoble, a short spelling at Colmar.
    #[test]
    fn a_tribunal_keys_its_cabinets_under_the_number() {
        let vocab = SeatVocabulary::new([
            tj_row("tj_grenoble", Some(4), &["CIVILE"], "4ème chambre civile"),
            tj_row("tj_grenoble", Some(4), &[], "Ch4.3 JCP"),
            tj_row("tj_grenoble", Some(4), &[], "Ch4.2 Inférieur à 10000 €"),
            tj_row("tj_grenoble", Some(4), &["BAUX"], "Ch4.4 TPBR"),
            tj_row("tj_colmar", Some(2), &["CIVILE"], "2ème Ch Civile Cab 1"),
            tj_row("tj_colmar", Some(2), &[], "Deuxième Chambre"),
        ]);
        for (written, key) in [
            (vec!["CIVILE"], "4ème chambre civile"),
            (vec![], "Ch4.3 JCP"),
            (vec!["BAUX"], "Ch4.4 TPBR"),
        ] {
            assert!(
                vocab
                    .resolve(&tj_row("tj_grenoble", Some(4), &written, key))
                    .specialty
                    .is_empty(),
                "{key}: the fourth chamber is the room, its cabinets sittings"
            );
        }
        assert_eq!(
            vocab
                .resolve(&tj_row("tj_colmar", Some(2), &[], "Deuxième Chambre"))
                .specialty,
            matters(&["CIVILE"]),
            "a key that says only the number is a spelling of the room"
        );
    }

    /// Melun, Colmar, Orléans, Montpellier and Besançon.
    #[test]
    fn a_court_that_renamed_a_room_keeps_one_room() {
        let melun = SeatVocabulary::new([
            dated(
                ta_row(
                    "ta_melun",
                    None,
                    None,
                    &["ETRANGERS"],
                    "Reconduite à la frontière",
                ),
                (2022, 1),
                (2024, 6),
            ),
            dated(
                ta_row(
                    "ta_melun",
                    None,
                    Some(12),
                    &["ETRANGERS"],
                    "12ème chambre, éloignement",
                ),
                (2024, 6),
                (2026, 6),
            ),
            dated(
                ta_row("ta_melun", None, Some(1), &[], "1ère chambre"),
                (2022, 1),
                (2026, 6),
            ),
        ]);
        let resolved = melun.resolve(&ta_row(
            "ta_melun",
            None,
            None,
            &["ETRANGERS"],
            "Reconduite à la frontière",
        ));
        assert_eq!(resolved.chamber, Some(ChamberId::Number(12)));
        assert_eq!(resolved.specialty, matters(&["ETRANGERS"]));
        assert_eq!(
            resolved.paths().pop().as_deref(),
            Some("FR.TA.MELUN.C12_ETRANGERS"),
            "the handover month is a succession"
        );

        // Colmar keyed nothing on its chambre sociale until 2021 and « Chambre
        // 4 A » since 2022, the body naming the matter both times.
        let colmar = SeatVocabulary::new([
            dated(
                silent(row(seat("ca_colmar", 4, &["SOCIALE"]), "")),
                (2002, 1),
                (2021, 6),
            )
            .clone()
            .into_unnumbered(),
            dated(
                row(seat("ca_colmar", 4, &["SOCIALE"]), "Chambre 4 A"),
                (2022, 1),
                (2026, 6),
            ),
            dated(
                row(seat("ca_colmar", 8, &[]), "Chambre 8"),
                (2002, 1),
                (2026, 6),
            ),
        ]);
        let old = silent(row(seat("ca_colmar", 4, &["SOCIALE"]), "")).into_unnumbered();
        assert_eq!(
            colmar.resolve(&old).paths().pop().as_deref(),
            Some("FR.CA.COLMAR.C4_SOCIALE")
        );

        // Orléans keys « Chambre Commerciale »; the body of its decisions with no
        // key wrote « 2e chambre commerciale », and the court leaves no number
        // bare.
        let orleans = SeatVocabulary::new([
            dated(
                row(seat("ca_orleans", 2, &["COMMERCIALE"]), "").into_unnumbered(),
                (2000, 1),
                (2026, 6),
            )
            .keyed("Chambre Commerciale"),
            dated(
                silent(row(seat("ca_orleans", 2, &["COMMERCIALE"]), "")),
                (2018, 1),
                (2021, 6),
            ),
            dated(
                row(seat("ca_orleans", 1, &["CIVILE"]), "1ère chambre civile"),
                (2000, 1),
                (2026, 6),
            ),
        ]);
        let numbered = silent(row(seat("ca_orleans", 2, &["COMMERCIALE"]), ""));
        assert_eq!(
            orleans.resolve(&numbered).paths().pop().as_deref(),
            Some("FR.CA.ORLEANS.COMMERCIALE")
        );

        // Montpellier's single judge and its 7th chamber sit in the same
        // months: two formations.
        let montpellier = SeatVocabulary::new([
            dated(
                ta_row(
                    "ta_montpellier",
                    None,
                    None,
                    &["ETRANGERS"],
                    "PROCEDURES 96 H H / 48 H",
                ),
                (2022, 1),
                (2026, 6),
            ),
            dated(
                ta_row(
                    "ta_montpellier",
                    None,
                    Some(7),
                    &["ETRANGERS"],
                    "7ème chambre OQTF 6 mois",
                ),
                (2025, 1),
                (2026, 6),
            ),
        ]);
        assert_eq!(
            montpellier
                .resolve(&ta_row(
                    "ta_montpellier",
                    None,
                    None,
                    &["ETRANGERS"],
                    "PROCEDURES 96 H H / 48 H",
                ))
                .paths()
                .pop()
                .as_deref(),
            Some("FR.TA.MONTPELLIER.ETRANGERS")
        );

        // Besançon has two chambers hearing civil cases: nowhere to fold.
        let besancon = SeatVocabulary::new([
            dated(
                silent(row(seat("ca_besancon", 1, &["CIVILE"]), "")).into_unnumbered(),
                (2001, 1),
                (2021, 6),
            ),
            dated(
                row(
                    seat("ca_besancon", 1, &["CIVILE", "COMMERCIALE"]),
                    "1ère Chambre",
                ),
                (2018, 1),
                (2026, 6),
            ),
            dated(
                row(seat("ca_besancon", 2, &["CIVILE"]), "2ème chambre civile"),
                (2018, 1),
                (2026, 6),
            ),
        ]);
        assert_eq!(
            besancon
                .resolve(&silent(row(seat("ca_besancon", 1, &["CIVILE"]), "")).into_unnumbered())
                .paths()
                .pop()
                .as_deref(),
            Some("FR.CA.BESANCON.CIVILE")
        );
    }

    impl SeatRow {
        /// The same row with its number dropped.
        fn into_unnumbered(mut self) -> SeatRow {
            self.seat.chamber = None;
            self
        }

        /// The same row keyed at the registry.
        fn keyed(mut self, chamber: &str) -> SeatRow {
            self.chamber = Some(chamber.to_string());
            self
        }
    }

    #[test]
    fn a_room_path_carries_its_matters_in_its_last_segment() {
        for (path, expected) in [
            ("FR.CA.COLMAR.C4_SOCIALE", vec!["SOCIALE"]),
            ("FR.CA.COLMAR.SOCIALE", vec!["SOCIALE"]),
            (
                "FR.CA.BESANCON.CIVILE_COMMERCIALE",
                vec!["CIVILE", "COMMERCIALE"],
            ),
            (
                "FR.CA.BESANCON.C1_CIVILE_COMMERCIALE",
                vec!["CIVILE", "COMMERCIALE"],
            ),
            ("FR.TA.NIMES.PROTECTION_SOCIALE", vec!["PROTECTION_SOCIALE"]),
            ("FR.CA.CAYENNE.CD_EXPROPRIATION", vec!["EXPROPRIATION"]),
        ] {
            assert_eq!(room_matters(path), Some(matters(&expected)), "{path}");
        }
        assert_eq!(room_matters("FR.CA.PARIS.P1.C11"), None);
        assert_eq!(room_matters("FR.TCOM.PARIS.CHAMBRE_MIXTE"), None);
    }

    /// A room made nameless by one aberrant decision gets its name back when the
    /// corpus loses the aberration, and only if the snapshot reads what was
    /// written.
    #[test]
    fn the_snapshot_reads_the_written_seat_and_not_its_resolution() {
        let aberrant = [
            row(seat("ca_pau", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_pau", 1, &["CORRECTIONNELLE"]), "1ère Chambre"),
        ];
        let yesterday = SeatVocabulary::new(aberrant.clone());
        assert!(yesterday.resolve(&aberrant[0]).specialty.is_empty());

        let corrected: Vec<SeatRow> = aberrant[..1].to_vec();
        let probe = row(seat("ca_pau", 1, &[]), "1ère Chambre");
        assert_eq!(
            SeatVocabulary::new(corrected.clone())
                .resolve(&probe)
                .specialty,
            matters(&["CIVILE"]),
            "read on what was written, the room is named again"
        );

        let resolved: Vec<SeatRow> = corrected
            .iter()
            .map(|r| SeatRow {
                seat: yesterday.resolve(r),
                ..r.clone()
            })
            .collect();
        assert!(
            SeatVocabulary::new(resolved)
                .resolve(&probe)
                .specialty
                .is_empty(),
            "read on its own output, it stays bare for ever"
        );
    }

    #[test]
    fn resolving_twice_gives_the_same_seat() {
        let rows = [
            row(
                seat("ca_besancon", 1, &["CIVILE", "COMMERCIALE"]),
                "1ère Chambre",
            ),
            row(seat("ca_besancon", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_caen", 1, &["CIVILE"]), "1ère Chambre civile"),
            row(seat("ca_caen", 1, &["SOCIALE"]), "1ère chambre sociale"),
            row(seat("ca_pau", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_pau", 1, &["CORRECTIONNELLE"]), "1ère Chambre"),
        ];
        let vocab = SeatVocabulary::new(rows.clone());
        for r in &rows {
            let once = vocab.resolve(r);
            let again = vocab.resolve(&SeatRow {
                seat: once.clone(),
                ..r.clone()
            });
            assert_eq!(again, once, "{:?}", r.seat);
        }
    }

    #[test]
    fn the_order_of_the_corpus_does_not_name_the_room() {
        let rows = vec![
            row(seat("ca_besancon", 1, &["CIVILE"]), "1ère Chambre"),
            row(seat("ca_besancon", 1, &[]), "1ère Chambre"),
            row(
                seat("ca_besancon", 1, &["CIVILE", "COMMERCIALE"]),
                "1ère Chambre",
            ),
        ];
        let forward = SeatVocabulary::new(rows.clone());
        let backward = SeatVocabulary::new(rows.into_iter().rev().collect::<Vec<_>>());
        let probe = row(seat("ca_besancon", 1, &[]), "1ère Chambre");
        assert_eq!(
            forward.resolve(&probe).specialty,
            backward.resolve(&probe).specialty
        );
    }

    /// The first pass of a corpus, where no seat has been written yet.
    #[test]
    fn an_unknown_room_keeps_what_the_source_wrote() {
        let empty = SeatVocabulary::new([]);
        assert!(empty.is_empty());
        assert_eq!(
            empty
                .resolve(&row(seat("ca_riom", 4, &["SOCIALE"]), "4ème chambre"))
                .specialty,
            matters(&["SOCIALE"])
        );
    }

    /// The registry reader must not drift from the one the extraction runs:
    /// `formation_axes_scanned` without a scan IS the registry alone.
    #[test]
    fn the_registry_reader_is_the_extractions_own() {
        for (jt, chamber, formation) in [
            ("CA", Some("1ère Chambre civile"), None),
            ("CC", Some("civ1"), None),
            ("TA", None, Some("4ème section")),
            ("TCOM", Some("Chambre 2"), None),
            ("CA", Some(""), Some("chambre sociale")),
        ] {
            let mut decision = crate::extract::tests::decision(jt);
            decision.chamber = chamber.map(str::to_string);
            decision.formation = formation.map(str::to_string);
            assert_eq!(
                registry_matters(jt, chamber, formation).unwrap_or_default(),
                crate::extract::formation_axes_scanned(&decision, None)
                    .chamber
                    .specialty,
                "{jt} {chamber:?} {formation:?}"
            );
        }
    }
}
