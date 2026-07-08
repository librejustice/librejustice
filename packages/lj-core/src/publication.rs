//! Carte canonique des codes de publication → libellé, par ordre
//! (port de `publication.py`, cf. ADR 0054).

/// `Some(libellé)` joint depuis les codes (« Publié au bulletin · Rapport »),
/// dédupliqué, ordre préservé, codes inconnus ignorés. `None` si vide.
pub fn publication_label(codes: Option<&[String]>) -> Option<String> {
    let codes = codes?;
    if codes.is_empty() {
        return None;
    }
    let table = labels_by_code();
    let mut labels: Vec<&'static str> = Vec::new();
    for code in codes {
        if let Some(&(_, label)) = table.iter().find(|(c, _)| *c == code.as_str()) {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join(" · "))
    }
}

/// Code → libellé d'affichage (judiciaire minuscules `b r l c n` ;
/// administratif majuscules `A B C C+ D Z R`).
pub fn labels_by_code() -> &'static [(&'static str, &'static str)] {
    &[
        ("r", "Publié au rapport"),
        ("b", "Publié au bulletin"),
        ("l", "Lettre de chambre"),
        ("c", "Communiqué"),
        ("n", "Inédit"),
        ("A", "Recueil Lebon"),
        ("B", "Tables du recueil Lebon"),
        ("C+", "Signalée"),
        ("R", "Signalée"),
        ("C", "Inédit au recueil Lebon"),
        ("D", "Inédit au recueil Lebon"),
        ("Z", "Inédit au recueil Lebon"),
    ]
}

/// Groupes de portée (IN-list dérivées des codes) : `majeure` / `importante` / `limitee`.
pub fn portee_codes(group: &str) -> &'static [&'static str] {
    match group {
        "majeure" => &["r", "A"],
        "importante" => &["b", "l", "c", "B", "C+", "R"],
        "limitee" => &["n", "C", "D", "Z"],
        _ => &[],
    }
}

/// Clé `portee:*` d'une décision : groupe du code au rang le plus fort
/// (`{b,r}` → `MAJEURE`), `INDETERMINEE` sans code classant. Mapping total —
/// toute décision est facettable (ADR 0167).
pub fn portee_key(codes: &[String]) -> &'static str {
    for (group, key) in [
        ("majeure", "MAJEURE"),
        ("importante", "IMPORTANTE"),
        ("limitee", "LIMITEE"),
    ] {
        if codes
            .iter()
            .any(|c| portee_codes(group).contains(&c.as_str()))
        {
            return key;
        }
    }
    "INDETERMINEE"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn none_when_empty_or_missing() {
        assert_eq!(publication_label(None), None);
        assert_eq!(publication_label(Some(&[])), None);
    }

    #[test]
    fn joins_with_middle_dot() {
        let c = codes(&["b", "r"]);
        assert_eq!(
            publication_label(Some(&c)).as_deref(),
            Some("Publié au bulletin · Publié au rapport")
        );
    }

    #[test]
    fn dedups_labels_preserving_order() {
        // C, D, Z partagent « Inédit au recueil Lebon » : un seul libellé.
        let c = codes(&["C", "D", "Z"]);
        assert_eq!(
            publication_label(Some(&c)).as_deref(),
            Some("Inédit au recueil Lebon")
        );
    }

    #[test]
    fn unknown_codes_ignored() {
        let c = codes(&["xyz", "b"]);
        assert_eq!(
            publication_label(Some(&c)).as_deref(),
            Some("Publié au bulletin")
        );
        // Que des codes inconnus → None.
        let only_unknown = codes(&["xyz", "???"]);
        assert_eq!(publication_label(Some(&only_unknown)), None);
    }

    #[test]
    fn portee_groups() {
        assert_eq!(portee_codes("majeure"), &["r", "A"]);
        assert_eq!(portee_codes("importante"), &["b", "l", "c", "B", "C+", "R"]);
        assert_eq!(portee_codes("limitee"), &["n", "C", "D", "Z"]);
        assert_eq!(portee_codes("inconnu"), &[] as &[&str]);
    }

    #[test]
    fn portee_key_takes_strongest_rank() {
        // `r` (rapport) l'emporte sur `b` (bulletin).
        assert_eq!(portee_key(&codes(&["b", "r", "c"])), "MAJEURE");
        assert_eq!(portee_key(&codes(&["A"])), "MAJEURE");
        assert_eq!(portee_key(&codes(&["b", "l"])), "IMPORTANTE");
        // `l` seul (lettre de chambre) classe importante même sans `b`.
        assert_eq!(portee_key(&codes(&["l"])), "IMPORTANTE");
        assert_eq!(portee_key(&codes(&["c", "n"])), "IMPORTANTE");
        assert_eq!(portee_key(&codes(&["n"])), "LIMITEE");
        assert_eq!(portee_key(&codes(&["C", "D"])), "LIMITEE");
        // Sans code classant (vide ou inconnu) : indéterminée.
        assert_eq!(portee_key(&[]), "INDETERMINEE");
        assert_eq!(portee_key(&codes(&["xyz"])), "INDETERMINEE");
    }
}
