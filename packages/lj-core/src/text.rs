//! Primitives de chaînes pures partagées (folding, recasse, collapse d'espaces,
//! lookahead manuel). Noyau `lj-core` : consommé côté serve par `aliases` (fold
//! des requêtes) et côté ingest par le recognizer de `lj-extract` — d'où leur
//! résidence dans l'ancêtre commun (ADR 0123).

use std::sync::LazyLock;

use regex::Regex;

/// `_fold` : minuscule, accents supprimés (NFD + drop Mn), apostrophes typo
/// normalisées, espaces collapsés. Clé de comparaison uniquement.
pub fn fold(text: &str) -> String {
    let lower = text.to_lowercase();
    let decomposed = nfd::decompose(&lower);
    let stripped: String = decomposed
        .chars()
        .filter(|c| !nfd::is_combining_mark(*c))
        .collect();
    let stripped = stripped.replace('\u{2019}', "'");
    static RE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    RE_WS.replace_all(&stripped, " ").trim().to_string()
}

pub fn uppercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `tok[0].upper() + tok[1:].lower()`.
pub fn capitalize_only(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Émule `re.finditer` avec borne droite *lookahead* consommée. Le crate `regex`
/// n'a pas de lookahead zéro-largeur (`(?=…)`) : nos patterns (citations
/// d'articles côté opendata, cabinets chaînés des observations côté judilibre)
/// consomment la borne droite, ce qui avancerait le curseur trop loin et
/// perdrait le match suivant. On rejoue donc `finditer` à la main en avançant à
/// la fin du groupe *borne* renvoyée par `advance` (== `match.end()` côté
/// Python), pas à la fin du match consommé ; garde-fou anti-boucle si la borne
/// est zéro-largeur. Partagé : opendata passe `group(2)`, judilibre `group(name)`.
pub fn captures_iter_lookahead<'t>(
    re: &Regex,
    text: &'t str,
    advance: impl Fn(&regex::Captures) -> Option<usize>,
) -> Vec<regex::Captures<'t>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start <= text.len() {
        match re.captures_at(text, start) {
            Some(c) => {
                let full_end = c.get(0).unwrap().end();
                let next = advance(&c).unwrap_or(full_end);
                let next = if next > start {
                    next
                } else {
                    full_end.max(start + 1)
                };
                out.push(c);
                start = next;
            }
            None => break,
        }
    }
    out
}

static RE_WS_GLOBAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// `_normalize_spaces`.
pub fn normalize_spaces(value: &str) -> String {
    RE_WS_GLOBAL
        .replace_all(value.trim(), " ")
        .trim()
        .to_string()
}

/// Mini-décomposition NFD ciblée (le crate `unicode-normalization` n'est pas
/// au manifest de `lj-core`). Suffisant pour `_fold` sur le corpus juridique
/// FR : retire les diacritiques latins courants.
mod nfd {
    pub fn decompose(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => {
                    out.push('a');
                    out.push('\u{0301}');
                }
                'è' | 'é' | 'ê' | 'ë' => {
                    out.push('e');
                    out.push('\u{0301}');
                }
                'ì' | 'í' | 'î' | 'ï' => {
                    out.push('i');
                    out.push('\u{0301}');
                }
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' => {
                    out.push('o');
                    out.push('\u{0301}');
                }
                'ù' | 'ú' | 'û' | 'ü' => {
                    out.push('u');
                    out.push('\u{0301}');
                }
                'ý' | 'ÿ' => {
                    out.push('y');
                    out.push('\u{0301}');
                }
                'ç' => {
                    out.push('c');
                    out.push('\u{0327}');
                }
                'ñ' => {
                    out.push('n');
                    out.push('\u{0303}');
                }
                other => out.push(other),
            }
        }
        out
    }

    pub fn is_combining_mark(c: char) -> bool {
        ('\u{0300}'..='\u{036f}').contains(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_strips_accents_case_and_apostrophes() {
        assert_eq!(fold("CODE CIVIL"), "code civil");
        assert_eq!(fold("Code de l\u{2019}entrée"), "code de l'entree");
        assert_eq!(fold("  Décret   du  "), "decret du");
    }
}
