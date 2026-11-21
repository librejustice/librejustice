//! The Swiss cantons as a cantonal act's stored title names them.

use std::ops::Range;

/// Each canton's code, its French name and the abbreviation of its systematic collection.
const CANTONS: [(&str, &str, &str); 26] = [
    ("AG", "Argovie", "SAR"),
    ("AI", "Appenzell Rhodes-Intérieures", "GS"),
    ("AR", "Appenzell Rhodes-Extérieures", "bGS"),
    ("BE", "Berne", "RSB"),
    ("BL", "Bâle-Campagne", "SGS"),
    ("BS", "Bâle-Ville", "SG"),
    ("FR", "Fribourg", "RSF"),
    ("GE", "Genève", "RSG"),
    ("GL", "Glaris", "GS"),
    ("GR", "Grisons", "BR"),
    ("JU", "Jura", "RSJU"),
    ("LU", "Lucerne", "SRL"),
    ("NE", "Neuchâtel", "RSN"),
    ("NW", "Nidwald", "NG"),
    ("OW", "Obwald", "GDB"),
    ("SG", "Saint-Gall", "sGS"),
    ("SH", "Schaffhouse", "SHR"),
    ("SO", "Soleure", "BGS"),
    ("SZ", "Schwyz", "SRSZ"),
    ("TG", "Thurgovie", "RB"),
    ("TI", "Tessin", "RL"),
    ("UR", "Uri", "RB"),
    ("VD", "Vaud", "BLV"),
    ("VS", "Valais", "RSVS"),
    ("ZG", "Zoug", "BGS"),
    ("ZH", "Zurich", "LS"),
];

/// A cantonal act's title as stored, `Finanzausgleichsgesetz (Zurich), LS 132.1`; panics on a
/// code that names no canton.
pub fn cantonal_title(title: &str, canton: &str, number: &str) -> String {
    let (_, name, collection) = CANTONS
        .iter()
        .find(|(code, ..)| *code == canton)
        .unwrap_or_else(|| panic!("no Swiss canton {canton}"));
    format!("{title} ({name}), {collection} {number}")
}

/// The canton a title written by [`cantonal_title`] names: its code, and where its French name
/// sits in `title`.
pub fn titled_canton(title: &str) -> Option<(&'static str, Range<usize>)> {
    CANTONS.iter().find_map(|(code, name, collection)| {
        let at = title.rfind(&format!(" ({name}), {collection} "))? + 2;
        Some((*code, at..at + name.len()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_title_gives_back_its_canton_and_nothing_else() {
        let title = cantonal_title(
            "Gesetz über Lebensmittel (Lebensmittelgesetz, LVG)",
            "NW",
            "717.1",
        );
        assert_eq!(
            title,
            "Gesetz über Lebensmittel (Lebensmittelgesetz, LVG) (Nidwald), NG 717.1"
        );
        let (code, at) = titled_canton(&title).unwrap();
        assert_eq!((code, &title[at]), ("NW", "Nidwald"));
        assert_eq!(titled_canton("Accord avec Genève (Genève), LS 1"), None);
        assert_eq!(titled_canton("Code de procédure civile suisse"), None);
    }
}
