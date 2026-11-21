//! The labels of the raw publication codes (`publication_codes`), from the catalogue. The other
//! labels of a decision (solution, office, domain, courts) arrive resolved by the API as `FacetTag`.

use crate::i18n::DecisionUi;

/// The codes of a decision left out of every collection.
const UNPUBLISHED: [&str; 4] = ["n", "C", "D", "Z"];

/// The labels of `codes` joined, « Publié au bulletin · Publié au rapport », each once, in
/// order, unknown codes left out; `None` when none is known.
pub fn publication_label(codes: &[String], ui: &DecisionUi) -> Option<String> {
    let mut labels: Vec<&'static str> = Vec::new();
    for label in codes.iter().filter_map(|code| (ui.publication_label)(code)) {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    (!labels.is_empty()).then(|| labels.join(" · "))
}

/// The label of the notable publications alone, the unpublished codes left out.
pub fn publication_badge(codes: &[String], ui: &DecisionUi) -> Option<String> {
    let notable: Vec<String> = codes
        .iter()
        .filter(|c| !UNPUBLISHED.contains(&c.as_str()))
        .cloned()
        .collect();
    publication_label(&notable, ui)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn publication_label_joins_and_dedups() {
        let ui = &Lang::Fr.ui().decision;
        let c = vec!["b".to_string(), "r".to_string()];
        assert_eq!(
            publication_label(&c, ui).as_deref(),
            Some("Publié au bulletin · Publié au rapport")
        );
        // C/D/Z partagent un libellé : dédup.
        let c = vec!["C".to_string(), "D".to_string(), "Z".to_string()];
        assert_eq!(
            publication_label(&c, ui).as_deref(),
            Some("Inédit au recueil Lebon")
        );
        assert_eq!(publication_label(&[], ui), None);
    }

    #[test]
    fn publication_badge_drops_inedit() {
        let ui = &Lang::Fr.ui().decision;
        // "n" = "Inédit", "b" = notable.
        let c = vec!["n".to_string(), "b".to_string()];
        assert_eq!(
            publication_badge(&c, ui).as_deref(),
            Some("Publié au bulletin")
        );
        // Que des inédits → None.
        let c = vec!["n".to_string(), "C".to_string()];
        assert_eq!(publication_badge(&c, ui), None);
    }
}
