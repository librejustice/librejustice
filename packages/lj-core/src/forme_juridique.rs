//! Catégories juridiques INSEE (niveau III) → libellé d'affichage.
//!
//! Le registre SIRENE porte la catégorie juridique en code brut (`5710`,
//! `5485`) ; les surfaces produit (fiche entité, annuaire — ADR 0189/0192)
//! affichent un libellé lisible. Nomenclature officielle INSEE embarquée
//! (PUR, `include_str!` — règle #17, exception données du code) sous
//! `data/insee_cj_niveau3.tsv`, libellés curés badge (sigle usuel : « SAS »,
//! « SELARL », « Commune »… quand il existe).

use std::collections::HashMap;
use std::sync::LazyLock;

/// TSV brut embarqué (`code TAB libellé`, commentaires `#`).
const INSEE_CJ_TSV: &str = include_str!("../data/insee_cj_niveau3.tsv");

/// Table code → libellé, parsée une fois. Une ligne malformée du dataset
/// embarqué est un bug de curation : erreur franche (règle #12).
static LABELS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    INSEE_CJ_TSV
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_once('\t')
                .unwrap_or_else(|| panic!("insee_cj_niveau3.tsv : ligne sans TAB : {l:?}"))
        })
        .collect()
});

/// Libellé d'affichage d'une catégorie juridique INSEE niveau III (`5710` →
/// « SAS »). `None` = code hors nomenclature — l'appelant retombe sur la
/// valeur brute (les `forme` non-INSEE du référentiel d'entités : «
/// association », « avocat (paris) »… passent telles quelles).
pub fn forme_juridique_label(code: &str) -> Option<&'static str> {
    LABELS.get(code).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_cles_et_fallback() {
        // Sigles usuels curés (spec coordinateur : affichage badge).
        assert_eq!(forme_juridique_label("5710"), Some("SAS"));
        assert_eq!(forme_juridique_label("5485"), Some("SELARL"));
        assert_eq!(forme_juridique_label("5499"), Some("SARL"));
        assert_eq!(
            forme_juridique_label("5599"),
            Some("SA à conseil d'administration")
        );
        assert_eq!(forme_juridique_label("7210"), Some("Commune"));
        // Libellé INSEE verbatim (pas de sigle usuel).
        assert_eq!(forme_juridique_label("9220"), Some("Association déclarée"));
        // Hors nomenclature : None — l'appelant garde la valeur brute.
        assert_eq!(forme_juridique_label("association"), None);
        assert_eq!(forme_juridique_label(""), None);
    }

    #[test]
    fn nomenclature_complete() {
        // La nomenclature INSEE niveau III (sept. 2022) compte 260 codes.
        assert_eq!(LABELS.len(), 260);
    }
}
