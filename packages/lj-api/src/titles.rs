//! Construction du titre lisible d'une décision — port de `titles.py` (ADR 0049).
//!
//! Format : `<juridiction>[, <formation>], <date FR>, <premier numéro de
//! rôle>` — p. ex. « Conseil d'État, 5ème et 6ème chambres réunies,
//! 25 février 2026, 499381 ». La juridiction est le `jurisdiction_name` réel
//! (assaini) ou, à défaut, le libellé de repli fourni par l'appelant — résolu
//! depuis le référentiel `juridiction:*`
//! ([`crate::referential::Referential::juridiction_type_label`], ADR 0146 §4 :
//! les labels sont de la donnée, pas des tables compilées). La formation est
//! la valeur DÉJÀ assainie par l'appelant (`formation_display`) — jamais la
//! colonne source brute. Date rendue en français, mono-locale.

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// « 2026-05-29 » → « 29 mai 2026 ». Passe-plat si pas une date ISO complète.
fn format_fr_date(date_lecture: &str) -> String {
    let parts: Vec<&str> = date_lecture.split('-').collect();
    if parts.len() != 3 {
        return date_lecture.to_string();
    }
    match (
        parts[0].parse::<i32>(),
        parts[1].parse::<usize>(),
        parts[2].parse::<i32>(),
    ) {
        (Ok(year), Ok(month), Ok(day)) if (1..=12).contains(&month) => {
            format!("{} {} {}", day, FR_MONTHS[month - 1], year)
        }
        _ => date_lecture.to_string(),
    }
}

/// Nom de juridiction affichable : `jurisdiction_name` assaini, sinon
/// `type_label` — le libellé du `juridiction_type` déjà résolu par l'appelant
/// (référentiel `juridiction:*`), ou le code brut faute de référentiel chargé.
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

/// Titre lisible « <juridiction>[, <formation>], <date FR>, <numéro> »
/// (formation/date/numéro optionnels).
pub fn decision_title(
    type_label: &str,
    jurisdiction_name: Option<&str>,
    formation: Option<&str>,
    date_lecture: Option<&str>,
    docket_numbers: Option<&[String]>,
) -> String {
    let jurisdiction = decision_jurisdiction(type_label, jurisdiction_name);
    let formation = formation.and_then(|f| formation_deduped(&jurisdiction, f));
    let mut parts = vec![jurisdiction];
    if let Some(f) = formation {
        parts.push(f);
    }
    if let Some(date) = date_lecture.filter(|d| !d.is_empty()) {
        parts.push(format_fr_date(date));
    }
    if let Some(first) = docket_numbers.and_then(|d| d.first()) {
        parts.push(first.clone());
    }
    parts.join(", ")
}

/// Formation débarrassée de ce que la juridiction porte déjà. Les labels de
/// juridiction de la Cour de cassation embarquent la chambre (« Cour de
/// cassation, deuxième chambre civile ») et le champ `formation` Judilibre la
/// répète en préfixe (« Deuxième chambre civile — Formation de section ») :
/// on retire ce préfixe (comparaison insensible à la casse) et on garde le
/// reste — rien à garder ⇒ pas de formation.
fn formation_deduped(jurisdiction: &str, formation: &str) -> Option<String> {
    let formation = formation.trim();
    if formation.is_empty() {
        return None;
    }
    let jur_lower = jurisdiction.to_lowercase();
    let mut rest = formation;
    for sep in ["—", "-", "–"] {
        if let Some((head, tail)) = formation.split_once(sep) {
            if jur_lower.contains(&head.trim().to_lowercase()) {
                rest = tail.trim_start_matches(['-', '–', '—', ' ']);
                break;
            }
        }
    }
    let rest = rest.trim();
    if rest.is_empty() || jur_lower.contains(&rest.to_lowercase()) {
        return None;
    }
    Some(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_title_with_date_and_docket() {
        let dockets = vec!["24-17.384".to_string()];
        // `type_label` déjà résolu par l'appelant (référentiel `juridiction:*`).
        let title = decision_title(
            "Cour de cassation",
            None,
            None,
            Some("2026-05-29"),
            Some(&dockets),
        );
        assert_eq!(title, "Cour de cassation, 29 mai 2026, 24-17.384");
    }

    #[test]
    fn formation_sits_between_jurisdiction_and_date() {
        let dockets = vec!["499381".to_string()];
        let title = decision_title(
            "Conseil d'État",
            None,
            Some("5ème et 6ème chambres réunies"),
            Some("2026-02-25"),
            Some(&dockets),
        );
        assert_eq!(
            title,
            "Conseil d'État, 5ème et 6ème chambres réunies, 25 février 2026, 499381"
        );
    }

    #[test]
    fn formation_already_in_jurisdiction_is_not_repeated() {
        let dockets = vec!["04-10.362".to_string()];
        let title = decision_title(
            "Cour de cassation",
            Some("Cour de cassation, deuxième chambre civile"),
            Some("Deuxième chambre civile"),
            Some("2005-02-24"),
            Some(&dockets),
        );
        assert_eq!(
            title,
            "Cour de cassation, deuxième chambre civile, 24 février 2005, 04-10.362"
        );
    }

    #[test]
    fn formation_chamber_prefix_is_stripped_keeping_the_rest() {
        let title = decision_title(
            "Cour de cassation",
            Some("Cour de cassation, première chambre civile"),
            Some("Première chambre civile — Formation de section"),
            Some("2016-09-28"),
            None,
        );
        assert_eq!(
            title,
            "Cour de cassation, première chambre civile, Formation de section, 28 septembre 2016"
        );
        // Variante tiret simple (données sources hétérogènes).
        assert_eq!(
            formation_deduped(
                "Cour de cassation, première chambre civile",
                "Première chambre civile - Formation restreinte RNSM"
            )
            .as_deref(),
            Some("Formation restreinte RNSM")
        );
        // Préfixe étranger à la juridiction : formation intacte.
        assert_eq!(
            formation_deduped("Conseil d'État", "5ème et 6ème chambres réunies").as_deref(),
            Some("5ème et 6ème chambres réunies")
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
        );
        assert_eq!(title, "Tribunal administratif de Paris, 13 février 2024");
    }

    #[test]
    fn blank_jurisdiction_name_falls_back_to_label() {
        let title = decision_title("Conseil d'État", Some("   "), None, None, None);
        assert_eq!(title, "Conseil d'État");
    }

    #[test]
    fn non_iso_date_passes_through() {
        assert_eq!(format_fr_date("2024"), "2024");
        assert_eq!(format_fr_date("2024-13-01"), "2024-13-01");
    }

    #[test]
    fn space_comma_is_collapsed() {
        let title = decision_title(
            "Cour d'appel",
            Some("Cour d'appel de Paris , chambre 1"),
            None,
            None,
            None,
        );
        assert_eq!(title, "Cour d'appel de Paris, chambre 1");
    }
}
