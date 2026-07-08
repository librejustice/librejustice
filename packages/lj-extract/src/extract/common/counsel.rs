//! Dédup des listes NER (avocats, cabinets, parties) — les valeurs restent
//! des tranches VERBATIM du texte (ADR 0157), seule la redondance se réduit.

use std::collections::HashSet;

/// `_unique_nonempty`.
pub(crate) fn unique_nonempty(values: &[String]) -> Option<Vec<String>> {
    let mut deduped: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for value in values {
        let compact = value.trim();
        if compact.is_empty() {
            continue;
        }
        let key = compact.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        deduped.push(compact.to_string());
    }
    if deduped.is_empty() {
        None
    } else {
        Some(deduped)
    }
}

/// Réduit les variantes-préfixes d'un même nom (casse ignorée) : quand un
/// candidat est le préfixe strict d'un autre (« SAS X » / « SAS X conclut… »),
/// seul le plus court survit — le plus long traîne de la prose échappée au
/// trim. Ordre d'apparition préservé.
pub(crate) fn dedupe_prefix_variants(values: Vec<String>) -> Vec<String> {
    let lowers: Vec<String> = values.iter().map(|v| v.to_lowercase()).collect();
    values
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !lowers.iter().enumerate().any(|(j, other)| {
                // Préfixe strict borné à un mot entier (le char suivant est un
                // espace) : « SAS Xy » ne consomme pas « SAS X ».
                j != *i
                    && lowers[*i].len() > other.len()
                    && lowers[*i].starts_with(other.as_str())
                    && lowers[*i].as_bytes().get(other.len()) == Some(&b' ')
            })
        })
        .map(|(_, v)| v.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_nonempty_dedupes_casefold() {
        let out = unique_nonempty(&[
            "Dupont".to_string(),
            "dupont".to_string(),
            "  ".to_string(),
            "Martin".to_string(),
        ]);
        assert_eq!(out, Some(vec!["Dupont".to_string(), "Martin".to_string()]));
        assert_eq!(unique_nonempty(&[]), None);
    }
}
