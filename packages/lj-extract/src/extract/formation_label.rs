//! Chambre lue dans le BANDEAU d'en-tête (zone par tokens du scan, ADR 0157),
//! consommée par le parse structuré de la formation (ADR 0170).
//!
//! Le crate `regex` ne supporte pas les lookaround : les patterns Python qui
//! en utilisent sont compilés sans l'assertion puis revérifiés côté Rust
//! (cf. [`trim_named_chamber`]).

use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

fn ci(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("static formation regex must compile")
}

// ───────────────────────── chambre lue au bandeau ───────────────────────────

fn re_body_pole() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| ci(r"\bp[ôo]le\s*(\d+)\s*-\s*chambre\s*(\d+)\b"))
}
fn re_body_conseil() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| ci(r"\bchambre\s+du\s+conseil\s*\("))
}
fn re_body_named_chamber() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // _RE_BODY_NAMED_CHAMBER : lookahead négatif interne `(?!(?:stop)\b)`
    // retiré, rejoué par `trim_named_chamber`.
    RE.get_or_init(|| {
        ci(r"cour\s+d['\x{2019}]appel\s+de\s+[\w'\x{2019}.\- ]+?\s+(chambre(?:\s+(?:des|du|de\s+la|d['\x{2019}]))?(?:\s+[a-zà-ÿ]+){1,3})\s+(?:arr[êe]t|audience|ordonnance|jugement|du|le|n[o°])\b")
    })
}
fn re_body_chamber_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Titre de greffe « COUR D'APPEL DE/D' <ville> » suivi — à travers les
    // sauts de ligne — du libellé de chambre en LIGNE de bandeau : ordinal
    // optionnel (« 2ème chambre ») + « chambre » + traîne de même ligne,
    // épluchée token à token par `chamber_line_label` (le crate `regex` n'a
    // pas de lookahead pour borner aux stopwords).
    RE.get_or_init(|| {
        ci(r"cour\s+d['\x{2019}]appel\s+d(?:e\s+|['\x{2019}]\s*)[\w'\x{2019}.\- ]+?\s+((?:\d{1,2}\s*(?:ème|eme|ère|ere|er|re|è|e)?\s+)?chambre\b[^\n]{0,80})")
    })
}

// Stopwords d'acte bornant le nom de chambre (cf. `_CHAMBER_STOP`,
// judilibre.py:367). Le regex `body_named_chamber` les exige en BORNE
// (trailing `\s+(?:STOP)\b`) mais — le crate `regex` n'ayant pas de lookahead
// — n'embarque PAS le lookahead négatif interne `(?!(?:STOP)\b)` qui, côté
// Python, empêche un stopword d'être avalé comme MOT du libellé. On reproduit
// ce lookahead a posteriori sur la capture brute via `trim_named_chamber`.
const CHAMBER_BODY_STOP: &[&str] = &[
    "arret",
    "arrêt",
    "audience",
    "ordonnance",
    "jugement",
    "du",
    "le",
    "no",
    "n°",
    "rg",
    "numero",
    "numéro",
];

fn is_body_stop(tok: &str) -> bool {
    let low = tok.to_lowercase();
    CHAMBER_BODY_STOP.iter().any(|s| *s == low)
}

/// Reproduit le lookahead négatif interne de `_RE_BODY_NAMED_CHAMBER` :
/// la capture greedy `chambre(?: connecteur)?(?:\s+\w+){1,3}` (sans lookahead)
/// peut avaler un stopword d'acte comme mot du libellé ; Python ne le ferait
/// jamais (le lookahead `(?!(?:STOP)\b)` borne la liste de mots au premier
/// stopword). On rejoue donc : `chambre` + connecteur optionnel (`des|du|de
/// la|d'`) + 1..3 mots arrêtés au premier stopword. Si zéro mot valide ne
/// suit (premier mot = stopword), Python renvoie None ({1,3} exige >= 1 mot).
fn trim_named_chamber(label: &str) -> Option<String> {
    let toks: Vec<&str> = label.split(' ').collect();
    let mut out: Vec<&str> = vec![toks[0]]; // "chambre"
    let mut i = 1;
    // Connecteur optionnel : des | du | d' | « de la ».
    if i < toks.len() {
        let low = toks[i].to_lowercase();
        if low == "des" || low == "du" || low == "d'" || low == "d\u{2019}" {
            out.push(toks[i]);
            i += 1;
        } else if low == "de" && i + 1 < toks.len() && toks[i + 1].to_lowercase() == "la" {
            out.push(toks[i]);
            out.push(toks[i + 1]);
            i += 2;
        }
    }
    // 1..3 mots, bornés au premier stopword (lookahead Python) ou token
    // non-mot (saut de ligne avalé par `\s+` dans la capture).
    let mut nwords = 0;
    while i < toks.len() && nwords < 3 {
        if is_body_stop(toks[i]) || !is_label_word(toks[i]) {
            break;
        }
        out.push(toks[i]);
        nwords += 1;
        i += 1;
    }
    if nwords < 1 {
        return None;
    }
    Some(out.join(" "))
}

/// Un token de libellé de chambre : mots (accents, apostrophes, points
/// d'abréviation « P.P. », virgule de liste en traîne), jamais de chiffre.
fn is_label_word(tok: &str) -> bool {
    let core = tok.trim_end_matches(',');
    !core.is_empty()
        && core
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, '\'' | '\u{2019}' | '.' | '-'))
        && core.chars().any(|c| c.is_alphabetic())
}

/// Épluche la traîne de ligne capturée par [`re_body_chamber_line`] :
/// ordinal optionnel + « chambre » + connecteur optionnel + 0..3 mots de
/// libellé (« et » de liste non compté), bornés au premier stopword d'acte
/// ou token non-mot (RG, dates, séparateurs). Une « chambre » nue sans
/// ordinal ni mot ne fait pas un libellé.
fn chamber_line_label(tail: &str) -> Option<String> {
    let toks: Vec<&str> = tail.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    let ordinal = toks
        .first()
        .is_some_and(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()));
    if ordinal {
        out.push(toks[0]);
        i += 1;
    }
    if toks.get(i).map(|t| fold_ascii(t)) != Some("chambre".to_string()) {
        return None;
    }
    out.push(toks[i]);
    i += 1;
    // Connecteur optionnel : des | du | d' | « de la ».
    if let Some(t) = toks.get(i) {
        let low = t.to_lowercase();
        if low == "des" || low == "du" || low == "d'" || low == "d\u{2019}" {
            out.push(t);
            i += 1;
        } else if low == "de" && toks.get(i + 1).map(|n| n.to_lowercase()).as_deref() == Some("la")
        {
            out.push(toks[i]);
            out.push(toks[i + 1]);
            i += 2;
        }
    }
    let mut nwords = 0;
    while i < toks.len() && nwords < 3 {
        let t = toks[i];
        if is_body_stop(t) || !is_label_word(t) {
            break;
        }
        if t.to_lowercase() == "et" {
            // « et » de liste : gardé seulement si un mot suit.
            if toks
                .get(i + 1)
                .is_some_and(|n| is_label_word(n) && !is_body_stop(n) && n.to_lowercase() != "et")
            {
                out.push(t);
                i += 1;
                continue;
            }
            break;
        }
        out.push(t);
        nwords += 1;
        i += 1;
    }
    if nwords < 1 && !ordinal {
        return None;
    }
    Some(out.join(" ").trim_end_matches(',').to_string())
}

/// Repli casse+accents ASCII d'un token (comparaison de mot-clé).
fn fold_ascii(tok: &str) -> String {
    tok.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            _ => c,
        })
        .collect()
}

/// Chambre lue dans le BANDEAU d'en-tête (zone par tokens du scan, ADR 0157
/// — remplace le `head(1500)`) : regex sur petit span positionné. La lecture
/// en ligne prime (bandeaux de greffe CA : chambre en ligne autonome après le
/// titre) ; l'ancien regex borné par stopword reste en repli pour les
/// libellés éclatés sur plusieurs lignes.
pub(super) fn chamber_from_body(bandeau: &str) -> Option<String> {
    if let Some(c) = re_body_pole().captures(bandeau) {
        return Some(format!("Pôle {} - Chambre {}", &c[1], &c[2]));
    }
    if re_body_conseil().is_match(bandeau) {
        return Some("Chambre du conseil".to_string());
    }
    for c in re_body_chamber_line().captures_iter(bandeau) {
        if let Some(label) = chamber_line_label(&c[1]) {
            return Some(label);
        }
    }
    if let Some(c) = re_body_named_chamber().captures(bandeau) {
        return trim_named_chamber(&c[1]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::chamber_from_body;

    #[test]
    fn chamber_en_ligne_de_bandeau() {
        // Chambre numérotée en ligne autonome après le titre de greffe.
        assert_eq!(
            chamber_from_body(
                "R. G : 10/ 07139\n\nCOUR D'APPEL DE LYON\n\n2ème chambre\n\nARRET DU 21 Mars 2011"
            )
            .as_deref(),
            Some("2ème chambre")
        );
        // Ville en « d' » + libellé borné par la traîne RG de même ligne.
        assert_eq!(
            chamber_from_body("COUR D'APPEL D'ANGERS\nChambre Sociale RG N : 10/ 03036").as_deref(),
            Some("Chambre Sociale")
        );
        // Liste à virgule avec « et », bornée en fin de ligne.
        assert_eq!(
            chamber_from_body(
                "COUR D'APPEL D'ORLÉANS\n\nCHAMBRE COMMERCIALE, ÉCONOMIQUE ET FINANCIÈRE\n\nGROSSES + EXPÉDITIONS"
            )
            .as_deref(),
            Some("CHAMBRE COMMERCIALE, ÉCONOMIQUE ET FINANCIÈRE")
        );
        // Tokens d'abréviation à points.
        assert_eq!(
            chamber_from_body(
                "COUR D'APPEL\n\nDE SAINT-DENIS\n\nChambre P.P. Référés\n\nRG N : 07/00050"
            )
            .as_deref(),
            Some("Chambre P.P. Référés")
        );
        // « chambre » nue sans ordinal ni mot : pas un libellé.
        assert_eq!(
            chamber_from_body("COUR D'APPEL DE PARIS\nchambre\n12 janvier"),
            None
        );
        // Connecteur « DES » conservé ; borne au stopword d'acte.
        assert_eq!(
            chamber_from_body(
                "COUR D'APPEL DE GRENOBLE \n CHAMBRE DES EXPROPRIATIONS \n DU 5 MARS"
            )
            .as_deref(),
            Some("CHAMBRE DES EXPROPRIATIONS")
        );
        // Lettre de section « B » préservée avant la borne d'acte.
        assert_eq!(
            chamber_from_body("COUR D'APPEL DE BASTIA \n CHAMBRE CIVILE B \n ARRET DU").as_deref(),
            Some("CHAMBRE CIVILE B")
        );
        // Premier mot après « chambre » = stopword d'acte → pas de capture.
        assert_eq!(
            chamber_from_body("COUR D'APPEL DE X \n CHAMBRE \n ARRET no 12"),
            None
        );
    }
}
