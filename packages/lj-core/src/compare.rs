//! Diff de deux rédactions d'un article (comparateur de versions, ADR 0193).
//!
//! Deux passes : diff **ligne** (alinéa) pour ancrer la structure du texte,
//! puis diff **mot Unicode** (UAX #29) à l'intérieur des blocs remplacés —
//! plus fin que le grain alinéa de Légifrance. Le texte se reconstruit en
//! concaténant les segments d'un même côté (Equal+Delete = ancien,
//! Equal+Insert = nouveau) ; les sauts de ligne restent dans les segments
//! (rendu `whitespace-pre-line`).

use similar::{ChangeTag, DiffTag, TextDiff};

/// Opération d'un segment du diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,
    Insert,
    Delete,
}

/// Tronçon contigu du diff : un texte et l'opération qui le porte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareSegment {
    pub op: CompareOp,
    pub text: String,
}

/// Diff des deux rédactions, en segments contigus (adjacents de même op
/// fusionnés, aucun segment vide).
pub fn compare_texts(old: &str, new: &str) -> Vec<CompareSegment> {
    let line_diff = TextDiff::from_lines(old, new);
    let mut segments: Vec<CompareSegment> = Vec::new();
    for op in line_diff.ops() {
        match op.tag() {
            DiffTag::Equal | DiffTag::Delete | DiffTag::Insert => {
                let text: String = line_diff.iter_changes(op).map(|c| c.value()).collect();
                let seg_op = match op.tag() {
                    DiffTag::Equal => CompareOp::Equal,
                    DiffTag::Delete => CompareOp::Delete,
                    _ => CompareOp::Insert,
                };
                push(&mut segments, seg_op, &text);
            }
            // Bloc remplacé : les deux côtés existent, on descend au mot.
            DiffTag::Replace => {
                let old_block: String = line_diff
                    .iter_changes(op)
                    .filter(|c| c.tag() == ChangeTag::Delete)
                    .map(|c| c.value())
                    .collect();
                let new_block: String = line_diff
                    .iter_changes(op)
                    .filter(|c| c.tag() == ChangeTag::Insert)
                    .map(|c| c.value())
                    .collect();
                let word_diff = TextDiff::from_unicode_words(&old_block, &new_block);
                for change in word_diff.iter_all_changes() {
                    let seg_op = match change.tag() {
                        ChangeTag::Equal => CompareOp::Equal,
                        ChangeTag::Delete => CompareOp::Delete,
                        ChangeTag::Insert => CompareOp::Insert,
                    };
                    push(&mut segments, seg_op, change.value());
                }
            }
        }
    }
    segments
}

/// Ajoute un tronçon en fusionnant avec le précédent s'il porte la même op.
fn push(segments: &mut Vec<CompareSegment>, op: CompareOp, text: &str) {
    if text.is_empty() {
        return;
    }
    match segments.last_mut() {
        Some(last) if last.op == op => last.text.push_str(text),
        _ => segments.push(CompareSegment {
            op,
            text: text.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_texts, CompareOp};

    fn old_side(segments: &[super::CompareSegment]) -> String {
        segments
            .iter()
            .filter(|s| s.op != CompareOp::Insert)
            .map(|s| s.text.as_str())
            .collect()
    }

    fn new_side(segments: &[super::CompareSegment]) -> String {
        segments
            .iter()
            .filter(|s| s.op != CompareOp::Delete)
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn identical_texts_yield_single_equal_segment() {
        let text = "Chacun a droit au respect de sa vie privée.\nLes juges peuvent prescrire toutes mesures.";
        let segments = compare_texts(text, text);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].op, CompareOp::Equal);
        assert_eq!(segments[0].text, text);
    }

    #[test]
    fn word_change_is_diffed_inside_the_alinea() {
        // Cas 16-13 : rédaction 2002 → 2004 (« de ses caractéristiques
        // génétiques » inchangé, le début de phrase remplacé au mot).
        let old = "Nul ne peut faire l'objet de discriminations en raison de ses caractéristiques génétiques.";
        let new = "Nul ne peut faire l'objet d'une discrimination en raison de ses caractéristiques génétiques.";
        let segments = compare_texts(old, new);
        assert_eq!(old_side(&segments), old);
        assert_eq!(new_side(&segments), new);
        // Le tronc commun reste en Equal — le diff n'a pas dégénéré en
        // suppression/réinsertion du texte entier.
        let equal_len: usize = segments
            .iter()
            .filter(|s| s.op == CompareOp::Equal)
            .map(|s| s.text.len())
            .sum();
        assert!(equal_len > old.len() / 2, "segments: {segments:?}");
        assert!(segments.iter().any(|s| s.op == CompareOp::Delete));
        assert!(segments.iter().any(|s| s.op == CompareOp::Insert));
    }

    #[test]
    fn inserted_alinea_is_a_single_insert_block() {
        let old = "Premier alinéa.\nDernier alinéa.";
        let new = "Premier alinéa.\nAlinéa nouveau.\nDernier alinéa.";
        let segments = compare_texts(old, new);
        assert_eq!(old_side(&segments), old);
        assert_eq!(new_side(&segments), new);
        let inserts: Vec<_> = segments
            .iter()
            .filter(|s| s.op == CompareOp::Insert)
            .collect();
        assert_eq!(inserts.len(), 1);
        assert_eq!(inserts[0].text, "Alinéa nouveau.\n");
    }

    #[test]
    fn deleted_alinea_is_a_single_delete_block() {
        let old = "Premier alinéa.\nAlinéa condamné.\nDernier alinéa.";
        let new = "Premier alinéa.\nDernier alinéa.";
        let segments = compare_texts(old, new);
        assert_eq!(old_side(&segments), old);
        assert_eq!(new_side(&segments), new);
        let deletes: Vec<_> = segments
            .iter()
            .filter(|s| s.op == CompareOp::Delete)
            .collect();
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0].text, "Alinéa condamné.\n");
    }

    #[test]
    fn empty_old_text_is_a_full_insert() {
        let segments = compare_texts("", "Texte créé.");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].op, CompareOp::Insert);
    }
}
