//! Titre lisible d'une décision côté API : résolution de la
//! juridiction (nom réel assaini, sinon libellé référentiel du type fourni
//! par l'appelant), siège composé du libellé de chambre du référentiel et des
//! uids `formation:*` / `office:*`, puis au
//! composeur canonique [`lj_core::titles`] — le même qui écrit
//! `search_title` à l'ingest.

use lj_core::chamber::{ChamberId, Container, Joined, Seat, Solemn};
use lj_dtos::{SeatChamber, SeatContainer, SeatJoined, SeatParts};

/// Nom de juridiction affichable : `jurisdiction_name` assaini, sinon
/// `type_label` — le libellé du `jurisdiction_type` déjà résolu par l'appelant
/// (référentiel `jurisdiction_type:*`), ou le code brut faute de référentiel chargé.
pub fn decision_jurisdiction(type_label: &str, jurisdiction_name: Option<&str>) -> String {
    if let Some(name) = jurisdiction_name {
        let cleaned = name.trim().replace(" ,", ",");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    type_label.to_string()
}

/// A decision's seat by its parts: the axes its title shows once
/// [`lj_core::titles::seat_axes`] has dropped what `jurisdiction` already says.
pub fn seat_parts(
    jurisdiction: &str,
    seat: Option<&Seat>,
    formation_uid: Option<&str>,
    office_uid: Option<&str>,
) -> Option<SeatParts> {
    let chamber = seat.and_then(Seat::title_label);
    let axes = lj_core::titles::seat_axes(
        jurisdiction,
        chamber.as_deref(),
        formation_uid.and_then(lj_core::titles::formation_label),
        office_uid.and_then(lj_core::titles::office_label),
    );
    let mut parts = match (axes.chamber, seat) {
        (Some(_), Some(seat)) => room_parts(seat),
        _ => SeatParts::default(),
    };
    parts.formation = axes.formation.and(formation_uid).map(key);
    parts.office = axes.office.and(office_uid).map(key);
    (parts != SeatParts::default()).then_some(parts)
}

/// The seat in French, composed from its parts: « pôle 4, 1re chambre · juge unique ».
pub fn french_seat(parts: &SeatParts) -> Option<String> {
    let chamber = room_seat(parts).title_label();
    lj_core::titles::SeatAxes {
        chamber: chamber.as_deref(),
        formation: parts
            .formation
            .as_deref()
            .and_then(|f| lj_core::titles::formation_label(&format!("formation:{f}"))),
        office: parts
            .office
            .as_deref()
            .and_then(|o| lj_core::titles::office_label(&format!("office:{o}"))),
    }
    .display()
}

/// A referential uid's key, without its facet: `formation:A_TROIS` is `A_TROIS`.
fn key(uid: &str) -> String {
    uid.split_once(':').map_or(uid, |(_, key)| key).to_string()
}

fn room_parts(seat: &Seat) -> SeatParts {
    SeatParts {
        container: seat.container.map(|container| match container {
            Container::Pole(n) => SeatContainer::Pole(n),
            Container::Section(n) => SeatContainer::Section(n),
            Container::Tribunal => SeatContainer::Tribunal,
            Container::TribunalFonctionPublique => SeatContainer::TribunalFonctionPublique,
        }),
        chamber: seat.chamber.map(|chamber| match chamber {
            ChamberId::Number(n) => SeatChamber::Number(n),
            ChamberId::Letter(l) => SeatChamber::Letter(l),
            ChamberId::Composed(a, b) => SeatChamber::Composed(a, b),
        }),
        solemn: seat.solemn.map(|solemn| solemn.token().to_string()),
        matters: seat.specialty.iter().map(|uid| key(uid)).collect(),
        section: seat.section,
        letter: seat.letter,
        joined: seat.joined.clone().map(|joined| match joined {
            Joined::SubSections(v) => SeatJoined::SubSections(v),
            Joined::Chambers(v) => SeatJoined::Chambers(v),
        }),
        formation: None,
        office: None,
    }
}

/// The parts read back into the stored seat they came from.
fn room_seat(parts: &SeatParts) -> Seat {
    Seat {
        container: parts.container.map(|container| match container {
            SeatContainer::Pole(n) => Container::Pole(n),
            SeatContainer::Section(n) => Container::Section(n),
            SeatContainer::Tribunal => Container::Tribunal,
            SeatContainer::TribunalFonctionPublique => Container::TribunalFonctionPublique,
        }),
        chamber: parts.chamber.map(|chamber| match chamber {
            SeatChamber::Number(n) => ChamberId::Number(n),
            SeatChamber::Letter(l) => ChamberId::Letter(l),
            SeatChamber::Composed(a, b) => ChamberId::Composed(a, b),
        }),
        solemn: parts.solemn.as_deref().map(|token| {
            Solemn::from_token(token).expect("a seat part's solemn comes from a stored seat")
        }),
        specialty: parts
            .matters
            .iter()
            .map(|matter| {
                lj_core::chamber::static_specialty(matter)
                    .expect("a seat part's matter comes from a stored seat")
            })
            .collect(),
        section: parts.section,
        letter: parts.letter,
        joined: parts.joined.clone().map(|joined| match joined {
            SeatJoined::SubSections(v) => Joined::SubSections(v),
            SeatJoined::Chambers(v) => Joined::Chambers(v),
        }),
        ..Seat::default()
    }
}

/// The « Formation » keys of a decision as tags: the path the `formation` filter
/// takes and its label, the composition first; the order and the type are left out.
pub fn formation_tags(paths: &[String]) -> Vec<lj_dtos::FacetTag> {
    paths
        .iter()
        .filter_map(|path| {
            Some(lj_dtos::FacetTag {
                key: path.clone(),
                label: lj_core::formation::formation_label(path)?.to_string(),
            })
        })
        .collect()
}

/// Seat tag: the deepest path, as the `seat` filter expects it, and the label
/// of its last segment. Paths come out unfolded from shortest to longest, so
/// the last one is the room.
pub fn seat_path_tag(paths: &[String]) -> Option<lj_dtos::FacetTag> {
    let path = paths.last()?;
    Some(lj_dtos::FacetTag {
        key: path.clone(),
        label: lj_core::chamber::seat_path_label(path)?,
    })
}

/// Readable title « <court>[, <seat>], <French date>, <number> », the last three
/// optional. `seat` is already composed, [`french_seat`], never a source string.
pub fn decision_title(
    type_label: &str,
    jurisdiction_name: Option<&str>,
    seat: Option<&str>,
    date_lecture: Option<&str>,
    nom_usage: Option<&str>,
    docket_numbers: Option<&[String]>,
) -> String {
    let jurisdiction = decision_jurisdiction(type_label, jurisdiction_name);
    lj_core::titles::decision_title(
        &jurisdiction,
        seat,
        date_lecture,
        nom_usage,
        docket_numbers
            .and_then(lj_core::titles::docket_display)
            .as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_url_names_the_order_its_seat_starts_at() {
        for kind in lj_dtos::JurisdictionType::ALL {
            let code = serde_json::to_value(kind).unwrap();
            let code = code.as_str().unwrap();
            let court = lj_vocab::courts::is_foreign(code)
                .then(|| lj_vocab::courts::foreign_court_code("sn", code, &[]));
            let order = lj_core::chamber::jurisdiction_order(code, court.as_deref());
            assert_eq!(
                order.to_ascii_lowercase(),
                kind.url_segment(court.as_deref()),
                "{code}"
            );
        }
    }

    #[test]
    fn full_title_with_date_and_docket() {
        let dockets = vec!["24-17.384".to_string()];
        let title = decision_title(
            "Cour de cassation",
            None,
            None,
            Some("2026-05-29"),
            None,
            Some(&dockets),
        );
        assert_eq!(title, "Cour de cassation, 29 mai 2026, 24-17.384");
    }

    #[test]
    fn seat_sits_between_jurisdiction_and_date() {
        let dockets = vec!["499381".to_string()];
        let title = decision_title(
            "Conseil d'État",
            None,
            Some("chambres 5/6 réunies"),
            Some("2026-02-25"),
            None,
            Some(&dockets),
        );
        assert_eq!(
            title,
            "Conseil d'État, chambres 5/6 réunies, 25 février 2026, 499381"
        );
    }

    fn seat(
        jurisdiction: &str,
        seat: Seat,
        formation: Option<&str>,
        office: Option<&str>,
    ) -> Option<String> {
        seat_parts(jurisdiction, Some(&seat), formation, office)
            .as_ref()
            .and_then(french_seat)
    }

    #[test]
    fn seat_cc_deja_dans_la_juridiction() {
        // Le label cass_civ2 porte déjà la chambre : le siège composé tombe.
        let room = Seat {
            chamber: Some(ChamberId::Number(2)),
            specialty: vec!["matiere:CIVILE"],
            ..Default::default()
        };
        let parts = seat_parts(
            "Cour de cassation, 2e chambre civile",
            Some(&room),
            Some("formation:RESTREINTE"),
            None,
        );
        assert_eq!(
            parts,
            Some(SeatParts {
                formation: Some("RESTREINTE".to_string()),
                ..Default::default()
            })
        );
        assert_eq!(
            parts.as_ref().and_then(french_seat).as_deref(),
            Some("formation restreinte")
        );
    }

    #[test]
    fn seat_office_depuis_les_axes() {
        // A room nothing qualifies: the label is the number alone.
        let room = Seat {
            chamber: Some(ChamberId::Number(7)),
            ..Default::default()
        };
        assert_eq!(
            seat(
                "Tribunal judiciaire de Nanterre",
                room,
                None,
                Some("office:JLD")
            )
            .as_deref(),
            Some("7e chambre · juge des libertés et de la détention")
        );
    }

    /// The French seat composed from the parts is the one composed from the stored seat.
    #[test]
    fn the_french_seat_is_composed_from_its_parts() {
        let cases = [
            (
                "Cour d'appel de Paris",
                Seat {
                    container: Some(Container::Pole(4)),
                    chamber: Some(ChamberId::Number(1)),
                    ..Default::default()
                },
                None,
                "pôle 4, 1re chambre",
            ),
            (
                "Cour d'appel de Lyon",
                Seat {
                    chamber: Some(ChamberId::Number(3)),
                    letter: Some('A'),
                    ..Default::default()
                },
                None,
                "3e chambre, section A",
            ),
            (
                "Cour d'appel de Douai",
                Seat {
                    specialty: vec!["matiere:SOCIALE"],
                    ..Default::default()
                },
                Some("formation:JUGE_UNIQUE"),
                "chambre sociale · juge unique",
            ),
        ];
        for (jurisdiction, room, formation, french) in cases {
            let stored = lj_core::titles::seat_display(
                jurisdiction,
                room.title_label().as_deref(),
                formation.and_then(lj_core::titles::formation_label),
                None,
            );
            assert_eq!(stored.as_deref(), Some(french));
            assert_eq!(
                seat(jurisdiction, room, formation, None).as_deref(),
                Some(french)
            );
        }
    }

    #[test]
    fn jurisdiction_name_wins_over_type_label() {
        let title = decision_title(
            "Tribunal administratif",
            Some("Tribunal administratif de Paris"),
            None,
            Some("2024-02-13"),
            None,
            None,
        );
        assert_eq!(title, "Tribunal administratif de Paris, 13 février 2024");
    }

    #[test]
    fn blank_jurisdiction_name_falls_back_to_label() {
        let title = decision_title("Conseil d'État", Some("   "), None, None, None, None);
        assert_eq!(title, "Conseil d'État");
    }

    #[test]
    fn space_comma_is_collapsed() {
        let title = decision_title(
            "Cour d'appel",
            Some("Cour d'appel de Paris , chambre 1"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(title, "Cour d'appel de Paris, chambre 1");
    }
}
