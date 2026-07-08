//! Libellés des codes de publication bruts (`publication_codes`) et de la
//! portée dérivée (groupes au rang le plus fort — facette depuis l'ADR 0167,
//! qui supersède le « affichage seulement » de l'ADR 0146 §2). Les autres
//! libellés métier (solution, voie, office, domaine, juridictions) arrivent
//! résolus par l'API en `FacetTag` (ADR 0146).

/// Table `code → libellé` des publications. Port de `PUBLICATION_LABELS`
/// (= `publication.py` / `labels_by_code`).
const PUBLICATION_LABELS: &[(&str, &str)] = &[
    ("b", "Publié au bulletin"),
    ("r", "Publié au rapport"),
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
];

fn publication_code_label(code: &str) -> Option<&'static str> {
    PUBLICATION_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, label)| *label)
}

/// Libellé joint depuis les codes (« Publié au bulletin · Rapport »), dédupliqué,
/// ordre préservé, codes inconnus ignorés. Port de `publicationLabel`. `None` si
/// vide / aucun code connu.
pub fn publication_label(codes: &[String]) -> Option<String> {
    if codes.is_empty() {
        return None;
    }
    let mut labels: Vec<&'static str> = Vec::new();
    for code in codes {
        if let Some(label) = publication_code_label(code) {
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

/// Libellé des seules publications notables (exclut les « Inédit… »). Port de
/// `publicationBadge` : filtre les codes dont le label commence par `"Inédit"`,
/// puis applique `publication_label`.
pub fn publication_badge(codes: &[String]) -> Option<String> {
    let notable: Vec<String> = codes
        .iter()
        .filter(|c| {
            publication_code_label(c)
                .map(|label| !label.starts_with("Inédit"))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    publication_label(&notable)
}

/// Groupes de portée, du rang le plus fort au plus faible — miroir de
/// `portee_codes` de lj-core (ADR 0167), même servitude de duplication que
/// `PUBLICATION_LABELS` (lj-web ne dépend pas de lj-core).
const PORTEE_GROUPS: &[(&str, &[&str])] = &[
    ("Majeure", &["r", "A"]),
    ("Importante", &["b", "l", "c", "B", "C+", "R"]),
    ("Limitée", &["n", "C", "D", "Z"]),
];

/// Libellé de portée au rang le plus fort (`{b,r}` → « Majeure »). `None` =
/// indéterminée (aucun code classant) — on ne l'affiche pas.
pub fn portee_label(codes: &[String]) -> Option<&'static str> {
    PORTEE_GROUPS.iter().find_map(|(label, group)| {
        codes
            .iter()
            .any(|c| group.contains(&c.as_str()))
            .then_some(*label)
    })
}

/// Badge de portée des cartes résultat : majeure/importante seulement (la
/// portée limitée, 85 % du corpus classé, serait du bruit) — remplace l'ancien
/// badge publication brut, dont il est la lecture normalisée inter-ordres.
pub fn portee_badge(codes: &[String]) -> Option<String> {
    portee_label(codes)
        .filter(|l| *l != "Limitée")
        .map(|l| format!("Portée {}", l.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_label_joins_and_dedups() {
        let c = vec!["b".to_string(), "r".to_string()];
        assert_eq!(
            publication_label(&c).as_deref(),
            Some("Publié au bulletin · Publié au rapport")
        );
        // C/D/Z partagent un libellé : dédup.
        let c = vec!["C".to_string(), "D".to_string(), "Z".to_string()];
        assert_eq!(
            publication_label(&c).as_deref(),
            Some("Inédit au recueil Lebon")
        );
        assert_eq!(publication_label(&[]), None);
    }

    #[test]
    fn publication_badge_drops_inedit() {
        // "n" = "Inédit", "b" = notable.
        let c = vec!["n".to_string(), "b".to_string()];
        assert_eq!(publication_badge(&c).as_deref(), Some("Publié au bulletin"));
        // Que des inédits → None.
        let c = vec!["n".to_string(), "C".to_string()];
        assert_eq!(publication_badge(&c), None);
    }

    #[test]
    fn portee_takes_strongest_rank_and_badge_hides_limitee() {
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(portee_label(&c(&["b", "r"])), Some("Majeure"));
        assert_eq!(portee_label(&c(&["l"])), Some("Importante"));
        assert_eq!(portee_label(&c(&["n"])), Some("Limitée"));
        assert_eq!(portee_label(&[]), None);
        assert_eq!(portee_badge(&c(&["A"])).as_deref(), Some("Portée majeure"));
        assert_eq!(
            portee_badge(&c(&["B"])).as_deref(),
            Some("Portée importante")
        );
        assert_eq!(portee_badge(&c(&["C", "D"])), None);
        assert_eq!(portee_badge(&[]), None);
    }
}
