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

/// Synonymes concept→corps : un terme doctrinal (« frais irrépétibles ») → la
/// formule **statutaire** qu'emploie la disposition source (« frais exposés non
/// compris dans les dépens », art. 700 CPC). Contrairement aux alias de textes
/// (titre), ceux-ci enrichissent la jambe **corps** de la recherche d'articles :
/// le terme doctrinal n'a aucune assise lexicale dans le texte du code, donc
/// sans ce pont l'article gouvernant n'est jamais candidat (ADR 0241).
const LEGAL_CONCEPT_SYNONYMS_JSON: &str = include_str!("../data/legal_concept_synonyms.json");

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

/// Une entrée de synonyme concept→corps curée (`why` ignoré à la désérialisation).
#[derive(Debug, Clone, Deserialize)]
struct ConceptSynonym {
    /// Terme doctrinal tapé par l'utilisateur.
    trigger: String,
    /// Formule statutaire à OR-er dans la jambe corps.
    expansion: String,
}

/// Synonymes concept chargés : `(trigger foldé, expansion)`, ordre du dataset.
static CONCEPT_SYNONYMS: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    let entries: Vec<ConceptSynonym> = serde_json::from_str(LEGAL_CONCEPT_SYNONYMS_JSON)
        .expect("legal_concept_synonyms.json embarqué valide");
    entries
        .into_iter()
        .map(|e| (fold(&e.trigger), e.expansion))
        .collect()
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

/// Expansions concept→corps déclenchées par un terme doctrinal présent dans
/// `query` (borné par mot, comme [`expand_query`]). Renvoie les formules
/// statutaires à OR-er dans la jambe **corps** de la recherche d'articles,
/// dédupliquées, plafonnées à [`MAX_EXPANSIONS`] (ADR 0241). Pur, sans I/O.
pub fn concept_expansions(query: &str) -> Vec<String> {
    let folded = fold(query);
    if folded.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for (trigger, expansion) in CONCEPT_SYNONYMS.iter() {
        if out.len() >= MAX_EXPANSIONS {
            break;
        }
        if contains_word(&folded, trigger) && !out.contains(expansion) {
            out.push(expansion.clone());
        }
    }
    out
}

/// Expansions dont l'alias couvre la requête **entière** (foldée) — la
/// requête EST le nom usuel d'un texte (« code civil du sénégal »), pas une
/// requête qui le contient (« article L442-1 du code de commerce »). Seules
/// ces expansions ont droit à la jambe conteneurs du ranking (ADR 0238) :
/// une expansion embarquée dans une requête plus large faisait voler le
/// rang 1 par le conteneur sur les requêtes d'article nommé (ADR 0234).
pub fn whole_query_expansions(query: &str) -> Vec<String> {
    let folded = fold(query);
    if folded.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for (alias, entry) in ALIASES.iter() {
        if *alias == folded && !out.contains(&entry.expansion) {
            out.push(entry.expansion.clone());
        }
    }
    out
}

/// Forme d'une requête pour un match **conjonctif** sur le titre formé
/// `search_title` (ADR 0114) : tous les tokens retournés doivent exister dans
/// le titre, donc on élimine les tokens que le titre ne porte jamais et on
/// recolle les numéros d'article à la forme indexée.
///
/// - « article(s) » / « art » sautent : le mot est exclu de `search_title`
///   (posting-list de 3 M de lignes qui ne discrimine rien, 0079) — le laisser
///   ferait échouer TOUTE conjonction « article L442-1 du code de commerce ».
/// - « L. 442-1 » / « R 600-2 » se recollent en « L442-1 » / « R600-2 » : le
///   tokenizer `[\p{L}\p{N}-]+` indexe le `num` en un seul token collé, la
///   forme pointée usuelle en produit deux qui ne matchent jamais.
/// - Les stopwords FR (« du », « de la »…) restent : le tokenizer de l'index
///   les élimine des deux côtés.
///
/// Renvoie `None` si rien ne subsiste (requête réduite à « article »…) —
/// l'appelant n'émet alors pas de clause conjonctive. Pur, sans I/O.
pub fn conj_title_query(query: &str) -> Option<String> {
    let tokens: Vec<&str> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| !t.is_empty())
        .filter(|t| {
            let t = fold(t);
            t != "article" && t != "articles" && t != "art"
        })
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut iter = tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        let is_ref_letter = tok.len() == 1 && matches!(fold(tok).as_str(), "l" | "r" | "d");
        let next_is_num = iter
            .peek()
            .is_some_and(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit()));
        if is_ref_letter && next_is_num {
            out.push(format!("{tok}{}", iter.next().expect("peeked")));
        } else {
            out.push(tok.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// `needle` apparaît dans `haystack` borné par des non-alphanumériques (ou les
/// bords). `haystack` et `needle` sont déjà foldés (minuscule, espaces collapsés).
pub(crate) fn contains_word(haystack: &str, needle: &str) -> bool {
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
    use super::{conj_title_query, contains_word, expand_query};

    #[test]
    fn conj_query_drops_article_and_glues_num() {
        assert_eq!(
            conj_title_query("article L. 442-1 du code de commerce").as_deref(),
            Some("L442-1 du code de commerce")
        );
        assert_eq!(
            conj_title_query("article 15 de la loi du 6 juillet 1989").as_deref(),
            Some("15 de la loi du 6 juillet 1989")
        );
        // Forme déjà collée : inchangée.
        assert_eq!(
            conj_title_query("R600-2 urbanisme").as_deref(),
            Some("R600-2 urbanisme")
        );
        // « d' » élidé ne se recolle pas sur un nombre qui suit.
        assert_eq!(
            conj_title_query("délai d'un mois").as_deref(),
            Some("délai d un mois")
        );
    }

    #[test]
    fn conj_query_empty_when_nothing_left() {
        assert_eq!(conj_title_query("article"), None);
        assert_eq!(conj_title_query(""), None);
    }

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
    fn whole_query_alias_expands_embedded_does_not() {
        use super::whole_query_expansions;
        // La requête EST l'alias (accents tolérés par le fold).
        assert_eq!(
            whole_query_expansions("code civil du sénégal"),
            vec!["code de la famille sénégalais".to_string()]
        );
        // Alias embarqué dans une requête plus large : rien.
        assert!(whole_query_expansions("article 700 cpc").is_empty());
        assert!(whole_query_expansions("").is_empty());
    }

    #[test]
    fn concept_synonym_bridges_doctrinal_to_statutory() {
        use super::concept_expansions;
        // Terme doctrinal (± accents) → formule statutaire du corps de l'article.
        assert_eq!(
            concept_expansions("frais irrépétibles"),
            vec!["frais exposés non compris dans les dépens".to_string()]
        );
        assert_eq!(
            concept_expansions("condamnation aux frais irrepetibles"),
            vec!["frais exposés non compris dans les dépens".to_string()]
        );
        // Requête thématique sans terme doctrinal connu : rien.
        assert!(concept_expansions("responsabilité du fait des choses").is_empty());
        assert!(concept_expansions("").is_empty());
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
