//! Aliases de textes juridiques : acronymes & noms usuels → forme développée
//! (ADR 0114). Substitut **lexical** au sémantique pour la recherche par titre :
//! l'utilisateur tape « CPC », « RGPD », « loi Badinter » ; on OR-e la forme
//! développée (« code de procédure civile »…) dans la requête BM25 sur le titre
//! formé `legal_article.search_title`.
//!
//! Données curées embarquées (PUR, `include_str!` — règle #17, exception données
//! du code) sous `data/legal_aliases.json`, ancrées sur le catalogue `legal_text`.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::text::fold;

/// JSON brut embarqué. Désérialisé une fois via [`ALIASES`].
const LEGAL_ALIASES_JSON: &str = include_str!("../data/legal_aliases.json");

/// Une entrée d'alias curée. Les autres champs du dataset (`canonical_slug`,
/// `canonical_text_uid`, `kind`, `catalog_status`) sont ignorés à la
/// désérialisation (serde tolère les champs JSON absents du struct).
#[derive(Debug, Clone, Deserialize)]
struct AliasEntry {
    /// Forme tapée par l'utilisateur (déjà minuscule/sans accent côté dataset ;
    /// re-`fold`ée au chargement par sûreté).
    alias: String,
    /// Forme développée à OR-er dans la requête (texte naturel ; le tokenizer
    /// ParadeDB fait l'ascii_folding).
    expansion: String,
}

/// Alias chargés : `(alias foldé, entrée)`, dans l'ordre du dataset.
static ALIASES: LazyLock<Vec<(String, AliasEntry)>> = LazyLock::new(|| {
    let entries: Vec<AliasEntry> =
        serde_json::from_str(LEGAL_ALIASES_JSON).expect("legal_aliases.json embarqué valide");
    entries.into_iter().map(|e| (fold(&e.alias), e)).collect()
});

/// Nombre maximal d'expansions retournées (borne anti-bloat de requête).
const MAX_EXPANSIONS: usize = 4;

/// Expansions d'alias présents dans `query` (ADR 0114). Un alias matche s'il
/// apparaît **borné par mot** dans la requête foldée (évite « css » dans
/// « accès »). Renvoie les formes développées, dédupliquées, dans l'ordre du
/// dataset, plafonnées à [`MAX_EXPANSIONS`]. Pur, sans I/O.
pub fn expand_query(query: &str) -> Vec<String> {
    let folded = fold(query);
    if folded.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for (alias, entry) in ALIASES.iter() {
        if out.len() >= MAX_EXPANSIONS {
            break;
        }
        if contains_word(&folded, alias) && !out.contains(&entry.expansion) {
            out.push(entry.expansion.clone());
        }
    }
    out
}

/// `needle` apparaît dans `haystack` borné par des non-alphanumériques (ou les
/// bords). `haystack` et `needle` sont déjà foldés (minuscule, espaces collapsés).
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{contains_word, expand_query};

    #[test]
    fn acronym_expands_to_full_name() {
        // « cpc » seul → « code de procédure civile ».
        let exp = expand_query("cpc");
        assert!(
            exp.iter().any(|e| e == "code de procédure civile"),
            "{exp:?}"
        );
    }

    #[test]
    fn acronym_in_phrase_expands() {
        // « article 700 cpc » → l'acronyme est détecté borné par mot.
        let exp = expand_query("article 700 cpc");
        assert!(
            exp.iter().any(|e| e == "code de procédure civile"),
            "{exp:?}"
        );
    }

    #[test]
    fn usual_name_expands() {
        // « loi badinter » (multi-mots, sans accent) est reconnu.
        let exp = expand_query("loi badinter");
        assert!(!exp.is_empty(), "loi badinter doit s'étendre");
    }

    #[test]
    fn accented_query_matches_unaccented_alias() {
        // L'utilisateur tape avec accent ; le fold rapproche de l'alias dataset.
        let exp = expand_query("RGPD");
        assert!(!exp.is_empty(), "rgpd doit s'étendre");
    }

    #[test]
    fn plain_query_yields_no_expansion() {
        // Une requête thématique sans acronyme ne déclenche rien.
        assert!(expand_query("responsabilité du fait des choses").is_empty());
    }

    #[test]
    fn word_boundary_prevents_substring_false_positive() {
        // « css » ne doit pas matcher à l'intérieur de « accès » (foldé « acces »)
        // ni « classe ».
        assert!(!contains_word("acces", "css"));
        assert!(!contains_word("classe interne", "css"));
        assert!(contains_word("le css ouvre", "css"));
    }
}
