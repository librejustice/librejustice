//! Extraction d'articles normalisés à partir du texte nettoyé
//! (port de `articles.py`). Helper pur conservé pour les tests/offline.
//! Volet CITATIONS (ADR 0116) — hors périmètre du scan de marqueurs ADR 0157.
//!
//! Reprend 1:1 le pipeline regex de `parsing/normalizer.py` (`_ART_RE` +
//! `_norm_suffix`) : on capture la mention brute (`m.group(0)`) pour l'affichage
//! et on dérive un `article_norm` minuscule, déterministe. Dédup sur
//! `article_norm` (première occurrence gagne).

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Référence d'article normalisée + mention brute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    /// `cja_l_761_1` | `l_761_1`.
    pub article_norm: String,
    /// `L. 761-1 du code de justice administrative`.
    pub raw_mention: String,
}

/// Mapping libellé → code canonique (port de `_CODE_MAP`). Ordonné du plus long
/// au plus court pour éviter qu'un libellé long soit avalé par `code`.
const CODE_MAP: &[(&str, &str)] = &[
    (
        "code de l entree et du sejour des etrangers et du droit d asile",
        "CESEDA",
    ),
    ("code general des collectivites territoriales", "CGCT"),
    ("code general de la fonction publique", "CGFP"),
    (
        "code des relations entre le public et l administration",
        "CRPA",
    ),
    ("code de procedure civile d execution", "CPCE"),
    ("code de la construction et de l habitation", "CCH"),
    ("code de procedure civile", "CPC"),
    ("code de procedure penale", "CPP"),
    ("code de justice administrative", "CJA"),
    ("code de la sante publique", "CSP"),
    ("code de la securite sociale", "CSS"),
    ("code general des impots", "CGI"),
    ("code de l urbanisme", "CURB"),
    ("code de l environnement", "CENV"),
    ("code de l education", "CEDU"),
    ("code du travail", "CT"),
    ("code civil", "CC"),
    ("code penal", "CP"),
    // Acronymes déjà présents tels quels dans le texte :
    ("ceseda", "CESEDA"),
    ("cedh", "CEDH"),
    ("cgct", "CGCT"),
    ("cgi", "CGI"),
    ("cja", "CJA"),
];

/// `_ART_RE` : préfixe L/R/D/A, point optionnellement espacé, numéro (≥ 2
/// chiffres) avec tirets `-`/`‑` (insécable), suffixe `du <code>` optionnel.
/// `(?i)` = `re.IGNORECASE` côté Python.
fn art_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)\b(?P<prefix>[LRDA])\s*\.\s*(?P<num>\d{2,}(?:[-\x{2011}]\d+)*)",
            r"(?:\s+du\s+(?P<suffix>code\s+[^\n,.;:()]{3,120}|ceseda|cedh|cgct|cgi|cja))?",
        ))
        .expect("static _ART_RE is valid")
    })
}

/// Supprime les diacritiques (NFKD + drop combining). Implémentation minimale
/// couvrant les accents français rencontrés dans les libellés de codes.
fn strip_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'û' | 'ü' | 'ù' | 'ú' => 'u',
            'ÿ' | 'ý' => 'y',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// Associe un libellé libre (« code de la santé publique ») à un sigle canonique
/// (port de `_norm_suffix`). Normalise casse/accents/apostrophes/espaces puis
/// cherche le préfixe le plus long de la table.
fn norm_suffix(raw_suffix: &str) -> Option<&'static str> {
    let lowered = strip_accents(&raw_suffix.to_lowercase());
    // re.sub(r"[’'`]", " ", s) puis collapse espaces.
    let replaced: String = lowered
        .chars()
        .map(|c| {
            if c == '’' || c == '\'' || c == '`' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = collapse_ws(&replaced);
    let s = collapsed.trim_matches([' ', '.', ',', ';', ':']);
    for (keyword, canonical) in CODE_MAP {
        if s.starts_with(keyword) {
            return Some(canonical);
        }
    }
    None
}

/// `re.sub(r"\s+", " ", s)` — collapse toute suite de blancs en un espace.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Extrait les articles cités, dans l'ordre du texte, dédupliqués sur `article_norm`.
pub fn extract_articles(cleaned_text: &str) -> Vec<Article> {
    let re = art_re();
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Article> = Vec::new();
    for caps in re.captures_iter(cleaned_text) {
        let prefix = caps
            .name("prefix")
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_uppercase();
        let num = caps
            .name("num")
            .map(|m| m.as_str())
            .unwrap_or("")
            .replace(['\u{2011}', '-'], "_");
        let code = caps.name("suffix").and_then(|m| norm_suffix(m.as_str()));

        let article_token = format!("{prefix}_{num}").to_lowercase();
        let article_norm = match code {
            Some(c) => format!("{}_{}", c.to_lowercase(), article_token),
            None => article_token,
        };

        if seen.contains(&article_norm) {
            continue;
        }
        seen.push(article_norm.clone());
        let raw_mention = caps
            .get(0)
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(Article {
            article_norm,
            raw_mention,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_code_to_compound_norm() {
        let arts = extract_articles("Vu l'article L. 761-1 du code de justice administrative ;");
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].article_norm, "cja_l_761_1");
        assert_eq!(
            arts[0].raw_mention,
            "L. 761-1 du code de justice administrative"
        );
    }

    #[test]
    fn orphan_article_without_code() {
        let arts = extract_articles("au visa de l'article L. 521-1, et plus loin.");
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].article_norm, "l_521_1");
        assert_eq!(arts[0].raw_mention, "L. 521-1");
    }

    #[test]
    fn requires_at_least_two_digits() {
        // « L. 1 » trop peu discriminant : pas de match.
        let arts = extract_articles("article L. 1 du code civil");
        assert!(arts.is_empty());
    }

    #[test]
    fn dedup_first_occurrence_wins() {
        let arts = extract_articles(
            "L. 761-1 du code de justice administrative et encore L. 761-1 du code de justice administrative",
        );
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].article_norm, "cja_l_761_1");
    }

    #[test]
    fn order_is_preserved() {
        let arts = extract_articles("L. 521-1 puis R. 421-1 du code de l'urbanisme");
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].article_norm, "l_521_1");
        assert_eq!(arts[1].article_norm, "curb_r_421_1");
    }

    #[test]
    fn nonbreaking_hyphen_in_number() {
        // tiret insécable U+2011 dans le numéro.
        let arts = extract_articles("article L. 761\u{2011}1 du code de justice administrative");
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].article_norm, "cja_l_761_1");
    }

    #[test]
    fn acronym_suffix_resolves() {
        let arts = extract_articles("au visa de l'article L. 511-1 du ceseda");
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].article_norm, "ceseda_l_511_1");
    }
}
