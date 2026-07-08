//! Références d'une décision (titre complet / abrégé / nom de fichier). Port de
//! `apps/web/src/lib/decision-reference.ts`. Pur.

use lj_dtos::DecisionDetail;

use crate::helpers::{format_iso_date, format_short_decision_jurisdiction};

/// Parties de référence d'une décision. Port de `DecisionReferenceParts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionReferenceParts {
    pub full: String,
    pub short: String,
    pub filename: String,
}

/// Construit les références. Port de `buildDecisionReferences` :
/// - `full` = `detail.title` (titre canonique servi par l'API) ;
/// - `short`/`filename` = `[juridiction courte, date FR, n° dossier]` joints `, `.
pub fn build_decision_references(detail: &DecisionDetail) -> DecisionReferenceParts {
    let date = detail
        .date_lecture
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let docket = detail
        .docket_numbers
        .as_ref()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_else(|| detail.id.clone());
    let jurisdiction = format_short_decision_jurisdiction(
        detail.juridiction_type,
        detail.jurisdiction_name.as_deref(),
    );
    let parts: Vec<String> = [Some(jurisdiction), date, Some(docket)]
        .into_iter()
        .flatten()
        .filter(|p| !p.trim().is_empty())
        .collect();
    let short = parts.join(", ");
    DecisionReferenceParts {
        full: detail.title.clone(),
        short: short.clone(),
        filename: short,
    }
}

/// Normalise un texte de sélection en **préservant les sauts de paragraphe** :
/// espaces et tabulations intra-ligne réduits à une espace, chaque ligne trimée,
/// lignes vides de tête/queue supprimées, runs de lignes vides réduits à une
/// seule (séparateur de paragraphe). `Selection.toString()` insère un `\n` entre
/// blocs `<p>` ; les écraser collait tous les paragraphes sur une ligne (port
/// enrichi de `normalizeSelectionText`, qui aplatissait tout `\s+`).
pub fn normalize_selection_text(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut pending_blank = false;
    for raw in text.lines() {
        let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            pending_blank = !lines.is_empty();
        } else {
            if pending_blank {
                lines.push(String::new());
                pending_blank = false;
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Aplatit la sélection sur une seule ligne (tout `\s+` → une espace) : pour la
/// requête de recherche, où les retours de ligne n'ont pas de sens.
pub fn flatten_selection_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `"<texte> (<référence>)"`, paragraphes du texte préservés. Port enrichi de
/// `formatSelectionWithReference`.
pub fn format_selection_with_reference(text: &str, reference: &str) -> String {
    format!("{} ({reference})", normalize_selection_text(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_preserves_paragraph_breaks() {
        // Sélection multi-paragraphes telle que `Selection.toString()` la rend.
        assert_eq!(
            normalize_selection_text("Premier paragraphe.\n\nSecond paragraphe."),
            "Premier paragraphe.\n\nSecond paragraphe."
        );
    }

    #[test]
    fn normalize_collapses_intraline_whitespace_and_trims() {
        assert_eq!(normalize_selection_text("  a   b \n\t c  "), "a b\nc");
    }

    #[test]
    fn normalize_squeezes_blank_runs_and_strips_edges() {
        assert_eq!(normalize_selection_text("\n\na\n\n\n\nb\n\n"), "a\n\nb");
    }

    #[test]
    fn flatten_drops_all_line_breaks() {
        assert_eq!(flatten_selection_text("a\n\nb  c"), "a b c");
    }

    #[test]
    fn format_with_reference_keeps_paragraphs() {
        assert_eq!(
            format_selection_with_reference("A\n\nB", "TA Paris, 1 janvier 2024, 12-345"),
            "A\n\nB (TA Paris, 1 janvier 2024, 12-345)"
        );
    }
}
