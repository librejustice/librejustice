//! Carte canonique des codes de publication → libellé, par ordre.

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

/// Valeurs de la facette `publication:*` et les codes source de chacune, dans
/// l'ordre du menu : les quatre catégories que la Cour sert elle-même, l'inédit
/// judiciaire, puis les trois niveaux du Lebon.
pub fn publication_groups() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("PUBLIE_BULLETIN", &["b"]),
        ("PUBLIE_RAPPORT", &["r"]),
        ("LETTRE_CHAMBRE", &["l"]),
        ("COMMUNIQUE", &["c"]),
        ("INEDIT_BULLETIN", &["n"]),
        ("PUBLIE_LEBON", &["A"]),
        ("MENTIONNE_LEBON", &["B", "C+", "R"]),
        ("INEDIT_LEBON", &["C", "D", "Z"]),
    ]
}

/// Paliers d'autorité, du plus fort au plus faible, pour trier une liste de
/// décisions quand la pertinence ne tranche pas. Clé de tri interne, jamais un
/// libellé : le lecteur voit la publication réelle, pas un rang.
pub fn authority_tiers() -> &'static [&'static [&'static str]] {
    &[
        &["r", "A"],
        &["b", "l", "c", "B", "C+", "R"],
        &["n", "C", "D", "Z"],
    ]
}

/// Codes source d'une valeur de facette, vide si la clé est inconnue.
pub fn publication_codes_of(key: &str) -> &'static [&'static str] {
    publication_groups()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, codes)| *codes)
        .unwrap_or(&[])
}

/// `publication:*` keys of a decision. Membership is **multiple**: a decision can
/// carry several codes, which keeping only the strongest rank would hide. No
/// source code gives an empty list, with no fallback value.
pub fn publication_keys(codes: &[String]) -> Vec<&'static str> {
    publication_groups()
        .iter()
        .filter(|(_, group)| codes.iter().any(|c| group.contains(&c.as_str())))
        .map(|(key, _)| *key)
        .collect()
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
        let only_unknown = codes(&["xyz", "???"]);
        assert_eq!(publication_label(Some(&only_unknown)), None);
    }

    #[test]
    fn publication_groups_cover_every_labelled_code() {
        let grouped: Vec<&str> = publication_groups()
            .iter()
            .flat_map(|(_, codes)| codes.iter().copied())
            .collect();
        for (code, _) in labels_by_code() {
            assert!(grouped.contains(code), "code {code} sans valeur de facette");
        }
        assert_eq!(publication_codes_of("MENTIONNE_LEBON"), &["B", "C+", "R"]);
        assert_eq!(publication_codes_of("inconnu"), &[] as &[&str]);
    }

    /// Miroir exact du référentiel : toute dérive entre les groupes compilés et
    /// les valeurs semées est un build rouge.
    #[test]
    fn groups_mirror_the_seeded_referential() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../lj-store/migrations");
        let mut seeded: std::collections::BTreeSet<String> = Default::default();
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for path in files {
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                let line = line.trim_start();
                if let Some(rest) = line.strip_prefix("('publication:") {
                    seeded.insert(rest[..rest.find('\'').unwrap()].to_string());
                } else if let Some(rest) =
                    line.strip_prefix("DELETE FROM facet_value WHERE uid = 'publication:")
                {
                    seeded.remove(&rest[..rest.find('\'').unwrap()]);
                }
            }
        }
        let compiled: std::collections::BTreeSet<String> = publication_groups()
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        assert_eq!(compiled, seeded);
    }

    /// Une décision porte toutes les valeurs dont elle a un code, pas la plus
    /// forte : `{b, l}` se trouve sous « bulletin » ET sous « lettre ».
    #[test]
    fn publication_keys_are_multiple() {
        assert_eq!(
            publication_keys(&codes(&["b", "l"])),
            vec!["PUBLIE_BULLETIN", "LETTRE_CHAMBRE"]
        );
        assert_eq!(publication_keys(&codes(&["r"])), vec!["PUBLIE_RAPPORT"]);
        assert_eq!(
            publication_keys(&codes(&["C", "D", "Z"])),
            vec!["INEDIT_LEBON"]
        );
        assert!(publication_keys(&[]).is_empty());
        assert!(publication_keys(&codes(&["xyz"])).is_empty());
    }
}
