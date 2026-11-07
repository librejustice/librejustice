//! The « Formation » filter: under a decision's order and court type, the bench's
//! composition and the judge sitting by function. Solemn formations stay in the seat.

/// Keys in display order, the composition first, with their label.
const KEYS: &[(&str, &str)] = &[
    ("PLENIERE", "Formation plénière"),
    ("SECTION", "Formation de section"),
    ("A_CINQ", "Formation à cinq"),
    ("A_TROIS", "Formation à trois"),
    ("RESTREINTE", "Formation restreinte"),
    ("CHAMBRE_SEULE", "Chambre jugeant seule"),
    ("JUGE_UNIQUE", "Juge unique"),
    ("PREMIER_PRESIDENT", "Premier président"),
    ("JUGE_REFERES", "Juge des référés"),
    ("MISE_EN_ETAT", "Juge de la mise en état"),
    ("JEX", "Juge de l'exécution"),
    ("JAF", "Juge aux affaires familiales"),
    ("JLD", "Juge des libertés et de la détention"),
    ("JCP", "Juge des contentieux de la protection"),
    ("JUGE_ENFANTS", "Juge des enfants"),
    ("JUGE_EXPROPRIATION", "Juge de l'expropriation"),
];

/// The Cour de cassation's formations, with the codes its decisions print.
const CC_LABELS: &[(&str, &str)] = &[
    ("PLENIERE", "Formation plénière de chambre (FP)"),
    ("SECTION", "Formation de section (FS)"),
    ("RESTREINTE", "Formation restreinte (F)"),
];

/// The court types the filter covers.
const COURTS: &[&str] = &["CC", "CE", "CAA", "TA", "CA", "TJ", "TCOM", "CPH"];

/// The paths `ORDER.TYPE.KEY` of a decision with their ancestors, as the seat
/// unfolds its own, from the trunk of its seat paths and the three axes the
/// extraction reads in a registry formation. No key, no ancestor.
pub fn formation_paths(
    seat_paths: &[String],
    formation_uid: Option<&str>,
    office_uid: Option<&str>,
    procedure_uid: Option<&str>,
) -> Vec<String> {
    let Some(trunk) = seat_paths.get(1) else {
        return Vec::new();
    };
    let Some((order, court_type)) = trunk.split_once('.') else {
        return Vec::new();
    };
    if !COURTS.contains(&court_type) {
        return Vec::new();
    }
    let office = office_uid.and_then(|u| u.strip_prefix("office:"));
    let composition = formation_uid
        .and_then(|u| u.strip_prefix("formation:"))
        .and_then(|k| composition(court_type, k))
        .or((office == Some("MAGISTRAT_DESIGNE")).then_some("JUGE_UNIQUE"));
    let referes = procedure_uid.is_some_and(|u| u.starts_with("procedure:REFERE_"))
        && referes(court_type, composition);
    let functions: Vec<&str> = [
        office.and_then(|k| function(court_type, k)),
        referes.then_some("JUGE_REFERES"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let composition =
        composition.or((!functions.is_empty() && court_type != "CPH").then_some("JUGE_UNIQUE"));
    let mut keys: Vec<&str> = composition.into_iter().chain(functions).collect();
    if keys.is_empty() {
        return Vec::new();
    }
    keys.sort_by_key(|k| rank(k));
    keys.dedup();
    [order.to_string(), trunk.clone()]
        .into_iter()
        .chain(keys.into_iter().map(|k| format!("{trunk}.{k}")))
        .collect()
}

/// The label of a path's key, `None` outside the vocabulary.
pub fn formation_label(path: &str) -> Option<&'static str> {
    let (trunk, key) = path.rsplit_once('.')?;
    if trunk.ends_with(".CC") {
        if let Some((_, label)) = CC_LABELS.iter().find(|(k, _)| *k == key) {
            return Some(label);
        }
    }
    match (trunk.rsplit('.').next(), key) {
        (Some("CPH"), "JUGE_REFERES") => return Some("Formation de référé"),
        (Some("CA"), "MISE_EN_ETAT") => return Some("Conseiller de la mise en état"),
        _ => {}
    }
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l)
}

/// Whether a filter value names a node of the tree: the order, a court type the
/// filter covers, or a key that type has.
pub fn is_formation_path(path: &str) -> bool {
    let mut segments = path.split('.');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("FR"), None, _, _) => true,
        (Some("FR"), Some(court_type), None, _) => COURTS.contains(&court_type),
        (Some("FR"), Some(court_type), Some(key), None) => {
            composition(court_type, key).is_some()
                || function(court_type, key) == Some(key)
                || (key == "JUGE_REFERES" && referes(court_type, Some("JUGE_UNIQUE")))
        }
        _ => false,
    }
}

/// Display rank of a path's key, its place in the vocabulary.
pub fn formation_rank(path: &str) -> usize {
    rank(path.rsplit('.').next().unwrap_or(path))
}

fn rank(key: &str) -> usize {
    KEYS.iter()
        .position(|(k, _)| *k == key)
        .unwrap_or(KEYS.len())
}

/// The Conseil d'État's section stays in the seat, with its other solemn formations.
fn composition<'a>(court_type: &str, key: &'a str) -> Option<&'a str> {
    let courts: &[&str] = match key {
        "PLENIERE" => &["CC", "CAA", "TA"],
        "SECTION" | "RESTREINTE" => &["CC"],
        "A_CINQ" => &["CAA", "TA"],
        "A_TROIS" => &["CAA", "TA", "CE"],
        "CHAMBRE_SEULE" => &["CE"],
        "JUGE_UNIQUE" => COURTS,
        _ => &[],
    };
    courts.contains(&court_type).then_some(key)
}

/// At the cour d'appel a specialised judge's label names the chamber hearing its
/// appeals, and a JLD order is appealed to the premier président or a delegate.
fn function<'a>(court_type: &str, key: &'a str) -> Option<&'a str> {
    match (court_type, key) {
        ("CA", "JLD") => Some("PREMIER_PRESIDENT"),
        ("CA" | "CC", "PREMIER_PRESIDENT") => Some(key),
        ("CA" | "TJ", "MISE_EN_ETAT") => Some(key),
        ("TJ", "JEX" | "JAF" | "JLD" | "JCP" | "JUGE_ENFANTS" | "JUGE_EXPROPRIATION") => Some(key),
        _ => None,
    }
}

/// The juge des référés sits alone, or three together at the Conseil d'État. A
/// chamber or a collegial bench ruling on a référé is not that judge, nor is the
/// cour d'appel or the Cour de cassation hearing its appeal or its review.
fn referes(court_type: &str, composition: Option<&str>) -> bool {
    match court_type {
        "TJ" | "TCOM" | "CPH" => true,
        "TA" | "CAA" => matches!(composition, None | Some("JUGE_UNIQUE")),
        "CE" => matches!(composition, Some("JUGE_UNIQUE" | "A_TROIS")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_path_names_a_key_its_court_has() {
        for path in [
            "FR",
            "FR.TJ",
            "FR.TJ.JEX",
            "FR.CC.RESTREINTE",
            "FR.CE.JUGE_REFERES",
            "FR.CA.MISE_EN_ETAT",
        ] {
            assert!(is_formation_path(path), "{path}");
        }
        for path in [
            "EU",
            "FR.CEDH",
            "FR.CA.JEX",
            "FR.CC.JUGE_REFERES",
            "FR.TA.CHAMBRE_SEULE",
            "FR.TJ.JEX.X",
        ] {
            assert!(!is_formation_path(path), "{path}");
        }
    }

    fn seat(trunk: &str) -> Vec<String> {
        let order = trunk.split('.').next().unwrap();
        vec![order.to_string(), trunk.to_string()]
    }

    #[test]
    fn a_tribunal_judiciaire_names_its_judge_and_its_composition() {
        assert_eq!(
            formation_paths(
                &seat("FR.TJ"),
                Some("formation:JUGE_UNIQUE"),
                Some("office:JEX"),
                None
            ),
            ["FR", "FR.TJ", "FR.TJ.JUGE_UNIQUE", "FR.TJ.JEX"]
        );
    }

    #[test]
    fn a_designated_magistrate_is_a_single_judge() {
        assert_eq!(
            formation_paths(
                &seat("FR.TA"),
                Some("formation:JUGE_UNIQUE"),
                Some("office:MAGISTRAT_DESIGNE"),
                Some("procedure:REFERE_SUSPENSION")
            ),
            ["FR", "FR.TA", "FR.TA.JUGE_UNIQUE", "FR.TA.JUGE_REFERES"]
        );
    }

    #[test]
    fn the_cour_d_appel_names_no_first_instance_judge() {
        assert_eq!(
            formation_paths(&seat("FR.CA"), None, Some("office:JLD"), None),
            [
                "FR",
                "FR.CA",
                "FR.CA.JUGE_UNIQUE",
                "FR.CA.PREMIER_PRESIDENT"
            ]
        );
        assert!(formation_paths(&seat("FR.CA"), None, Some("office:JEX"), None).is_empty());
        assert!(
            formation_paths(&seat("FR.CA"), None, None, Some("procedure:REFERE_CIVIL")).is_empty()
        );
    }

    #[test]
    fn a_function_judge_sits_alone_but_the_prud_hommes_referes() {
        assert_eq!(
            formation_paths(&seat("FR.TJ"), None, Some("office:MISE_EN_ETAT"), None),
            ["FR", "FR.TJ", "FR.TJ.JUGE_UNIQUE", "FR.TJ.MISE_EN_ETAT"]
        );
        assert_eq!(
            formation_paths(&seat("FR.CPH"), None, None, Some("procedure:REFERE_CIVIL")),
            ["FR", "FR.CPH", "FR.CPH.JUGE_REFERES"]
        );
    }

    #[test]
    fn a_collegial_bench_ruling_on_a_refere_is_not_the_juge_des_referes() {
        let refere = Some("procedure:REFERE_SUSPENSION");
        assert_eq!(
            formation_paths(&seat("FR.CAA"), Some("formation:A_TROIS"), None, refere),
            ["FR", "FR.CAA", "FR.CAA.A_TROIS"]
        );
        assert_eq!(
            formation_paths(
                &seat("FR.CE"),
                Some("formation:CHAMBRE_SEULE"),
                None,
                refere
            ),
            ["FR", "FR.CE", "FR.CE.CHAMBRE_SEULE"]
        );
        assert!(formation_paths(&seat("FR.CE"), None, None, refere).is_empty());
        assert_eq!(
            formation_paths(&seat("FR.CE"), Some("formation:A_TROIS"), None, refere),
            ["FR", "FR.CE", "FR.CE.A_TROIS", "FR.CE.JUGE_REFERES"]
        );
    }

    #[test]
    fn a_composition_holds_in_its_courts_alone() {
        assert!(formation_paths(&seat("FR.CA"), Some("formation:A_TROIS"), None, None).is_empty());
        assert!(
            formation_paths(&seat("EU.CJUE"), Some("formation:JUGE_UNIQUE"), None, None).is_empty()
        );
    }

    #[test]
    fn the_solemn_formations_stay_in_the_seat() {
        assert!(formation_paths(
            &seat("FR.CE"),
            Some("formation:SECTION"),
            None,
            Some("procedure:QPC")
        )
        .is_empty());
        assert!(formation_paths(
            &seat("FR.CE"),
            Some("formation:CHAMBRES_REUNIES"),
            None,
            None
        )
        .is_empty());
        assert_eq!(
            formation_paths(&seat("FR.CC"), Some("formation:SECTION"), None, None),
            ["FR", "FR.CC", "FR.CC.SECTION"]
        );
    }

    #[test]
    fn no_seat_no_trunk() {
        assert!(formation_paths(&[], Some("formation:JUGE_UNIQUE"), None, None).is_empty());
    }

    #[test]
    fn the_cour_de_cassation_prints_its_codes() {
        assert_eq!(
            formation_label("FR.CC.RESTREINTE"),
            Some("Formation restreinte (F)")
        );
        assert_eq!(
            formation_label("FR.CAA.PLENIERE"),
            Some("Formation plénière")
        );
        assert_eq!(
            formation_label("FR.CPH.JUGE_REFERES"),
            Some("Formation de référé")
        );
        assert_eq!(
            formation_label("FR.CA.MISE_EN_ETAT"),
            Some("Conseiller de la mise en état")
        );
    }
}
