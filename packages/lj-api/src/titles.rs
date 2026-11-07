//! Titre lisible d'une décision côté API : résolution de la
//! juridiction (nom réel assaini, sinon libellé référentiel du type fourni
//! par l'appelant), siège composé du libellé de chambre du référentiel et des
//! uids `formation:*` / `office:*`, puis au
//! composeur canonique [`lj_core::titles`] — le même qui écrit
//! `search_title` à l'ingest.

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

/// Siège composé depuis les axes structurés : délègue à
/// [`lj_core::titles::seat_display`] avec les labels référentiels des uids.
pub fn decision_seat(
    jurisdiction: &str,
    chamber_label: Option<&str>,
    formation_uid: Option<&str>,
    office_uid: Option<&str>,
) -> Option<String> {
    lj_core::titles::seat_display(
        jurisdiction,
        chamber_label,
        formation_uid.and_then(lj_core::titles::formation_label),
        office_uid.and_then(lj_core::titles::office_label),
    )
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

/// Titre lisible « <juridiction>[, <siège>], <date FR>, <numéro> »
/// (siège/date/numéro optionnels). `seat` est le siège DÉJÀ composé
/// ([`decision_seat`]) — jamais une chaîne source.
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

    #[test]
    fn seat_cc_deja_dans_la_juridiction() {
        // Le label cass_civ2 porte déjà la chambre : le siège composé tombe.
        let jur = "Cour de cassation, 2e chambre civile";
        let seat = decision_seat(
            jur,
            Some("2e chambre civile"),
            Some("formation:RESTREINTE"),
            None,
        );
        assert_eq!(seat.as_deref(), Some("formation restreinte"));
    }

    #[test]
    fn seat_office_depuis_les_axes() {
        // A room nothing qualifies: the label is the number alone.
        let label = lj_core::chamber::Seat {
            chamber: Some(lj_core::chamber::ChamberId::Number(7)),
            ..Default::default()
        }
        .label();
        let seat = decision_seat(
            "Tribunal judiciaire de Nanterre",
            label.as_deref(),
            None,
            Some("office:JLD"),
        );
        assert_eq!(
            seat.as_deref(),
            Some("7e chambre · juge des libertés et de la détention")
        );
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
