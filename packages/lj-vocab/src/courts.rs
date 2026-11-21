//! The court types of the foreign orders, whose code starts with their place.

/// The closed list of families a foreign court's type is one of.
pub const FOREIGN_COURT_TYPES: [&str; 13] = [
    "COUR_SUPREME",
    "COUR_CASSATION",
    "COUR_CONSTITUTIONNELLE",
    "COUR_APPEL",
    "COUR_ADMINISTRATIVE",
    "TRIBUNAL_ADMINISTRATIF",
    "TRIBUNAL_PREMIERE_INSTANCE",
    "TRIBUNAL_COMMERCE",
    "COUR_CRIMINELLE",
    "COUR_JUSTICE",
    "CCJA",
    "COUR_DROITS_HOMME",
    "AUTORITE_ADMINISTRATIVE",
];

pub fn is_foreign(jurisdiction_type: &str) -> bool {
    FOREIGN_COURT_TYPES.contains(&jurisdiction_type)
}

/// Place, type, then the words naming the court within its type: `sn_cour_appel_dakar`.
/// A court code carries no hyphen, a compound place name taking an underscore for it.
pub fn foreign_court_code(place: &str, jurisdiction_type: &str, name: &[&str]) -> String {
    let mut code = format!(
        "{}_{}",
        place.to_ascii_lowercase(),
        jurisdiction_type.to_ascii_lowercase()
    );
    for word in name {
        code.push('_');
        code.push_str(&word.to_ascii_lowercase().replace('-', "_"));
    }
    code
}

/// The place heading a foreign court's code: `sn_cour_supreme` sits in `SN`.
pub fn foreign_order(jurisdiction_code: &str) -> String {
    let (place, _) = jurisdiction_code.split_once('_').unwrap_or_else(|| {
        panic!("a foreign court's code starts with its place: {jurisdiction_code}")
    });
    place.to_ascii_uppercase()
}

/// What a foreign court's code says after its place, `cour_appel_dakar`.
pub fn foreign_court_rest(jurisdiction_code: &str) -> &str {
    jurisdiction_code
        .split_once('_')
        .map_or(jurisdiction_code, |(_, rest)| rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_foreign_code_names_its_place_then_its_court() {
        let code = foreign_court_code("sn", "COUR_APPEL", &["dakar"]);
        assert_eq!(code, "sn_cour_appel_dakar");
        assert_eq!(foreign_order(&code), "SN");
        assert_eq!(foreign_court_rest(&code), "cour_appel_dakar");
        assert_eq!(
            foreign_court_code("cg", "COUR_APPEL", &["pointe-noire"]),
            "cg_cour_appel_pointe_noire"
        );
        assert_eq!(
            foreign_order(&foreign_court_code("ohada", "CCJA", &[])),
            "OHADA"
        );
    }
}
