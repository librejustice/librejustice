//! Signal d'usage des citations (ADR 0248) : normalisation partagée entre le
//! prédicat de recherche (grammes de la requête) et le job `lj-ingest
//! usage-terms` (sacs) — les deux côtés doivent produire les mêmes tokens
//! pour que l'index `legal_article_usage_bm25` matche.

/// Grammes d'une chaîne : unigrammes (> 1 char) + bigrammes joints par `_`
/// (le tokenizer du champ garde `_` dans ses tokens : un bigramme ne matche
/// qu'un bigramme). Minuscules, lettres et tirets seuls.
pub fn usage_grams(text: &str) -> String {
    let lower = text.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| {
            if c.is_alphabetic() || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut toks: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() > 1)
        .map(|w| (*w).to_string())
        .collect();
    toks.extend(words.windows(2).map(|p| format!("{}_{}", p[0], p[1])));
    toks.join(" ")
}

/// Garde structurelle de la clause usage : les requêtes-référence (un token
/// porteur de chiffre — « L442-1 », « article 145 du CPC ») et
/// navigationnelles (« code … ») visent une entité exacte déjà servie par les
/// clauses titre / la jambe conteneurs — le signal d'usage n'y apporte que du
/// bruit de co-citation. Validée au banc (parité exacte sur 10 GT
/// référence/nav, working-note 2026-07-20).
pub fn usage_reference_or_nav_query(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.split_whitespace()
        .any(|t| t.chars().any(|c| c.is_ascii_digit()))
        || q.starts_with("code ")
        || q == "code"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_unigrams_and_joined_bigrams() {
        assert_eq!(
            usage_grams("frais irrépétibles"),
            "frais irrépétibles frais_irrépétibles"
        );
        assert_eq!(usage_grams("l'article 700"), "article l_article");
    }

    #[test]
    fn guard_reference_and_nav() {
        assert!(usage_reference_or_nav_query(
            "article L442-1 du code de commerce"
        ));
        assert!(usage_reference_or_nav_query("code civil du sénégal"));
        assert!(!usage_reference_or_nav_query("frais irrépétibles"));
        assert!(!usage_reference_or_nav_query(
            "déplafonnement du loyer commercial"
        ));
    }
}
