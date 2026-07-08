//! Nettoyage texte + normalisation des références juridiques
//! (port de `parsing/normalizer.py`).
//!
//! Produit la liste des tokens d'articles cités (`L_611_1`, `CESEDA_L_611_1`…)
//! ajoutés à l'index FTS. Volontairement non exhaustif : on préfère le silence
//! au bruit (regex strict > regex laxiste).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

// ── 1. Nettoyage texte ──────────────────────────────────────────────────────

static BR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[ \t\u{a0}\u{202f}]+").unwrap());
static BLANKLINES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Transforme le texte XML brut en texte lisible (`<br/>` → `\n`, décodage HTML,
/// suppression NUL, espaces insécables → espace, collapse, ≤ 2 sauts de ligne).
pub fn clean_texte(raw: &str) -> String {
    let txt = BR_RE.replace_all(raw, "\n");
    let txt = html_escape::decode_html_entities(&txt);
    let txt = txt.replace('\u{0}', "");
    let txt = txt.replace(['\u{a0}', '\u{202f}'], " ");
    let txt = WS_RE.replace_all(&txt, " ");
    let joined = txt
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    let collapsed = BLANKLINES_RE.replace_all(&joined, "\n\n");
    collapsed.trim().to_string()
}

// ── 2. Normalisation des références ─────────────────────────────────────────

/// Mapping libellé → code canonique. Ordonné du plus long au plus court pour
/// éviter que « code de la santé publique » soit avalé par « code ».
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

// Articles : L / R / D / A, éventuellement pointés et espacés, suivi d'un numéro
// de type 611-1, 1142-1, 521-1. On exige au moins 2 chiffres. Les tirets « - » et
// « ‑ » (insécable) sont acceptés. La capture `suffix` est greedy.
static ART_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?P<prefix>[LRDA])\s*\.\s*(?P<num>\d{2,}(?:[-‑]\d+)*)(?:\s+du\s+(?P<suffix>code\s+[^\n,.;:()]{3,120}|ceseda|cedh|cgct|cgi|cja))?",
    )
    .unwrap()
});

static APOSTROPHE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[’'`]").unwrap());
static SUFFIX_WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Strip les diacritiques combinables des lettres latines (équivalent
/// fonctionnel de `unicodedata.normalize("NFKD")` + filtrage `combining` sur
/// la plage utile aux libellés de codes français).
fn strip_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'ç' => 'c',
            'Ç' => 'C',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'ñ' => 'n',
            'Ñ' => 'N',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'ý' | 'ÿ' => 'y',
            'Ý' => 'Y',
            other => other,
        })
        .collect()
}

/// Associe un libellé libre (« code de la santé publique ») à un sigle canonique.
fn norm_suffix(raw_suffix: &str) -> Option<String> {
    let s = strip_accents(&raw_suffix.to_lowercase());
    let s = APOSTROPHE_RE.replace_all(&s, " ");
    let s = SUFFIX_WS_RE.replace_all(&s, " ");
    let s = s.trim_matches(|c| c == ' ' || c == '.' || c == ',' || c == ';' || c == ':');
    // Cherche la plus longue correspondance dans la table.
    for (keyword, canonical) in CODE_MAP {
        if s.starts_with(keyword) {
            return Some((*canonical).to_string());
        }
    }
    None
}

/// Référence d'article extraite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    /// L / R / D / A.
    pub prefix: String,
    /// `611_1`.
    pub num: String,
    /// CESEDA, CJA… (`None` si non résolu).
    pub code: Option<String>,
}

impl Ref {
    /// `{prefix}_{num}` (ex. `L_611_1`).
    pub fn article_token(&self) -> String {
        format!("{}_{}", self.prefix, self.num)
    }

    /// `{code}_{article_token}` ou `None` si le code n'est pas résolu.
    pub fn compound_token(&self) -> Option<String> {
        self.code
            .as_ref()
            .map(|c| format!("{}_{}", c, self.article_token()))
    }
}

/// Extrait les références d'articles, dédupliquées dans l'ordre du texte.
pub fn extract_refs(texte: &str) -> Vec<Ref> {
    let mut seen: HashSet<(String, String, Option<String>)> = HashSet::new();
    let mut out: Vec<Ref> = Vec::new();
    for caps in ART_RE.captures_iter(texte) {
        let prefix = caps.name("prefix").unwrap().as_str().to_uppercase();
        let num = caps
            .name("num")
            .unwrap()
            .as_str()
            .replace(['\u{2011}', '-'], "_");
        let code = caps.name("suffix").and_then(|m| norm_suffix(m.as_str()));
        let key = (prefix.clone(), num.clone(), code.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        out.push(Ref { prefix, num, code });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_texte_br_and_entities() {
        let raw = "Vu&#160;la requête<br/>présentée<br />par&nbsp;M.&#39;X&amp;Y";
        let out = clean_texte(raw);
        assert_eq!(out, "Vu la requête\nprésentée\npar M.'X&Y");
    }

    #[test]
    fn clean_texte_collapses_blanklines_and_trims_lines() {
        let raw = "  ligne 1  \n\n\n\n  ligne 2  ";
        let out = clean_texte(raw);
        assert_eq!(out, "ligne 1\n\nligne 2");
    }

    #[test]
    fn clean_texte_removes_nul_and_nbsp() {
        let raw = "a\u{0}b\u{a0}c\u{202f}d\te";
        let out = clean_texte(raw);
        assert_eq!(out, "ab c d e");
    }

    #[test]
    fn extract_refs_with_code_and_dedup() {
        // L. 611-1 du CESEDA, puis répétition (dédupliquée), puis R.521-1 du CJA.
        let texte = "Vu l'article L. 611-1 du code de l'entrée et du séjour des étrangers et du droit d'asile ; \
                     l'article L. 611-1 du CESEDA ; et l'article R.521-1 du code de justice administrative.";
        let refs = extract_refs(texte);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].prefix, "L");
        assert_eq!(refs[0].num, "611_1");
        assert_eq!(refs[0].code.as_deref(), Some("CESEDA"));
        assert_eq!(refs[0].article_token(), "L_611_1");
        assert_eq!(refs[0].compound_token().as_deref(), Some("CESEDA_L_611_1"));
        assert_eq!(refs[1].prefix, "R");
        assert_eq!(refs[1].num, "521_1");
        assert_eq!(refs[1].code.as_deref(), Some("CJA"));
    }

    #[test]
    fn extract_refs_unresolved_code() {
        let refs = extract_refs("article L. 1142-1");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].num, "1142_1");
        assert_eq!(refs[0].code, None);
        assert_eq!(refs[0].compound_token(), None);
    }

    #[test]
    fn extract_refs_requires_two_digits() {
        // « L. 1 » trop peu discriminant : pas de match.
        assert!(extract_refs("article L. 1 du code civil").is_empty());
    }
}
