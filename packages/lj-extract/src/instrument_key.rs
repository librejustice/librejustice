//! Clé d'identité canonique d'un instrument normatif, **indépendante du diffuseur**
//! (ADR 0115). Filet de la cascade d'identité `ELI → NOR → instrument_key` : quand un
//! acte n'a ni ELI ni NOR (vieux fonds, sources hors DILA), on le ré-identifie par
//! `nature | date | numéro` — la forme par laquelle il est *cité* (« loi n° 65-557 du
//! 10 juillet 1965 »). Deux manifestations diffuseur d'un même acte (LEGITEXT/JORFTEXT)
//! produisent la **même** clé → collapse en une identité.
//!
//! Pur (#1) : aucune I/O. Le numéro est fourni par l'appelant (lu dans `<NUM>` ou parsé
//! du titre à la frontière de parsing). La clé n'est émise que pour un acte **numéroté**
//! et **daté** : `nature|date` seul collisionne (mille arrêtés le même jour) — pour ces
//! actes non numérotés (longue traîne JO), l'identité reste l'uid-diffuseur (pas de
//! collapse à faire, ADR 0115 §5 : ce résidu est purgé, pas dédupliqué).

use crate::extract::common::fold;

/// Construit la clé canonique `nature|date|num` d'un instrument, ou `None` si la donnée
/// est insuffisante pour une identité **sans collision** (numéro ou date manquant).
///
/// - `nature` : `<NATURE>` DILA (LOI/DECRET/ORDONNANCE…), replié (minuscule, sans accent).
/// - `date_texte` : date du texte en ISO `YYYY-MM-DD` (la date par laquelle on cite).
/// - `num` : numéro propre de l'acte (`65-557`, `2008-496`), libellé brut accepté
///   (`n° 65-557`, `65‑557`, `65 557`) → normalisé en `65-557`.
pub fn instrument_key(nature: &str, date_texte: Option<&str>, num: Option<&str>) -> Option<String> {
    let date = date_texte?.trim();
    let num = normalize_num(num?)?;
    let nature = fold(nature);
    if nature.is_empty() || date.is_empty() {
        return None;
    }
    Some(format!("{nature}|{date}|{num}"))
}

/// Normalise un numéro d'acte vers sa forme canonique `AA-NNN` (ex. `65-557`).
/// Retire le préfixe `n°`/`no`/`numéro`, replie les espaces/points internes autour du
/// tiret, unifie les variantes de tiret. `None` si rien d'exploitable ne reste.
fn normalize_num(raw: &str) -> Option<String> {
    // Replie tirets typographiques (‑ ‐ – —) en `-`, retire espaces fines et points.
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '\u{2011}' | '\u{2010}' | '\u{2013}' | '\u{2014}' => '-',
            _ => c,
        })
        .collect();
    s = fold(&s); // minuscule + sans accent + espaces compactés
                  // Retire un préfixe numéro éventuel.
    for p in ["numero ", "n° ", "n°", "no ", "n ", "num "] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.to_string();
            break;
        }
    }
    // Compacte les espaces résiduels autour du tiret/chiffres : « 65 - 557 » → « 65-557 ».
    let compact: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '.')
        .collect();
    if compact.is_empty() || !compact.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loi_numerotee_datee() {
        assert_eq!(
            instrument_key("LOI", Some("1965-07-10"), Some("65-557")),
            Some("loi|1965-07-10|65-557".to_string())
        );
    }

    #[test]
    fn collapse_variantes_de_numero_vers_meme_cle() {
        // Toutes les formes citées d'un même acte doivent produire la même clé.
        let canon = Some("loi|1983-07-13|83-634".to_string());
        for n in [
            "83-634",
            "n° 83-634",
            "n°83-634",
            "83‑634",
            "83 - 634",
            "numéro 83-634",
        ] {
            assert_eq!(
                instrument_key("LOI", Some("1983-07-13"), Some(n)),
                canon,
                "num={n:?}"
            );
        }
    }

    #[test]
    fn nature_repliee_insensible_casse_accent() {
        // « DÉCRET » et « decret » → même nature dans la clé.
        assert_eq!(
            instrument_key("DÉCRET", Some("2009-01-07"), Some("2009-14")),
            instrument_key("decret", Some("2009-01-07"), Some("2009-14"))
        );
    }

    #[test]
    fn sans_numero_pas_de_cle() {
        // Acte non numéroté (arrêté « du X ») : nature|date collisionne → pas de clé.
        assert_eq!(instrument_key("ARRETE", Some("2018-05-09"), None), None);
    }

    #[test]
    fn sans_date_pas_de_cle() {
        assert_eq!(instrument_key("LOI", None, Some("65-557")), None);
    }

    #[test]
    fn numero_vide_ou_non_numerique_rejete() {
        assert_eq!(instrument_key("LOI", Some("1965-07-10"), Some("n°")), None);
        assert_eq!(instrument_key("LOI", Some("1965-07-10"), Some("   ")), None);
    }
}
