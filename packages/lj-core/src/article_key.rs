//! Clé publique d'un numéro d'article — segment d'URL `/texte/{code}/{clé}` et
//! alphabet des colonnes `num_key` (ADR 0209, remplace la forme « L. 761-1 »
//! de l'ADR 0123 §2).
//!
//! Alphabet : minuscules ASCII, chiffres, `-`, plus `*` et `()` qui portent un
//! sens juridique (`R*212-4` = décret en conseil des ministres, distinct de
//! `R. 212-4`). Le préfixe de partie (`L`, `R`, `D`, `A`, `LO`, `LP`,
//! précédé d'au plus deux `*`) est collé au numéro : « L. 761-1 » → `l761-1`.
//! Tout autre séparateur se plie en `-` : « 106 Bis » → `106-bis`,
//! « Annexe 1 » → `annexe-1`, « 100/1 » → `100-1`.
//!
//! La fonction est un point fixe (`article_key(article_key(x)) ==
//! article_key(x)`) : les backfills se rejouent, et toute forme citée d'un
//! même numéro (« L761-1 », « l. 761-1 », « L 761-1 ») tombe sur la même clé.

/// Préfixes de partie collés au numéro (minuscules, après pliage).
const GLUED_PREFIXES: [&str; 6] = ["l", "r", "d", "a", "lo", "lp"];

/// Clé publique d'un numéro d'article (voir doc de module).
pub fn article_key(num: &str) -> String {
    // Pliage : minuscules, accents ASCII, alphabet restreint, séparateurs en `-`.
    let mut folded = String::with_capacity(num.len());
    for c in num.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'î' | 'ï' => 'i',
            'ó' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'œ' => {
                folded.push('o');
                'e'
            }
            c => c,
        };
        if c.is_ascii_alphanumeric() || matches!(c, '*' | '(' | ')') {
            folded.push(c);
        } else if !folded.is_empty() && !folded.ends_with('-') {
            folded.push('-');
        }
    }
    let folded = folded.trim_end_matches('-');

    // Collage du préfixe de partie : `[**]l-761-1` → `[**]l761-1`.
    let stars = folded.chars().take_while(|c| *c == '*').count();
    let rest = &folded[stars..];
    for prefix in GLUED_PREFIXES {
        if let Some(tail) = rest.strip_prefix(prefix) {
            if let Some(tail) = tail.strip_prefix('-') {
                if tail.starts_with(|c: char| c.is_ascii_digit()) {
                    return format!("{}{prefix}{tail}", &folded[..stars]);
                }
            }
        }
    }
    folded.to_string()
}

/// Clé d'IDENTITÉ d'un numéro d'article servi intégral — la valeur de
/// `legal_article.num_key` et du segment d'URL `/texte/{code}/{clé}` (ADR 0236).
///
/// Distincte de la clé de CITATION (`normalize_article` de `lj-extract`, pliée
/// par [`article_key`]) : celle-ci tronque volontairement au cœur de
/// l'identifiant (ordinaux hors vocabulaire, discriminants « -0 W », numéros
/// pointés « 1.01 ») — parfait pour apparier des citations bruitées,
/// catastrophique comme identité : 4 508 groupes d'articles distincts
/// confondus sous une même clé (audit 2026-07-19). L'identité, elle, plie le
/// numéro DILA **sans perte** ; sa seule normalisation propre est le marqueur
/// ordinal de tête (« 1er » ≡ « 1 », « 1er bis » ≡ « 1 bis », « premier » ≡
/// « 1 ») — un marqueur, pas un discriminant. Point fixe comme `article_key`.
pub fn identity_key(num: &str) -> String {
    let t = num.trim();
    if t.eq_ignore_ascii_case("premier") {
        return "1".to_string();
    }
    let nd = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if nd > 0 {
        let rest = &t[nd..];
        let low = rest.to_lowercase();
        for marker in ["ère", "ere", "er"] {
            if low.starts_with(marker)
                && low[marker.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                return article_key(&format!("{}{}", &t[..nd], &rest[marker.len()..]));
            }
        }
    }
    article_key(t)
}

/// Forme d'affichage d'une clé publique à préfixe de partie simple :
/// `l761-1` → « L. 761-1 », `lo6213-1` → « LO. 6213-1 ». Hors de ce
/// sous-ensemble (numérique nu, étoiles, annexes…), la clé se rend telle
/// quelle — le libellé autoritaire reste `legal_article.num` quand on l'a.
pub fn display(key: &str) -> String {
    let stars = key.chars().take_while(|c| *c == '*').count();
    let rest = &key[stars..];
    for prefix in ["lo", "lp", "l", "r", "d", "a"] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            if tail.starts_with(|c: char| c.is_ascii_digit()) {
                return format!("{}{}. {tail}", &key[..stars], prefix.to_uppercase());
            }
        }
    }
    key.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_inverse_les_prefixes_simples() {
        assert_eq!(display("l761-1"), "L. 761-1");
        assert_eq!(display("lo6213-1"), "LO. 6213-1");
        assert_eq!(display("r*212-4"), "r*212-4");
        assert_eq!(display("1240"), "1240");
        assert_eq!(display("106-bis"), "106-bis");
        assert_eq!(display("annexe-1"), "annexe-1");
    }

    #[test]
    fn prefixe_de_partie_colle_au_numero() {
        assert_eq!(article_key("L. 761-1"), "l761-1");
        assert_eq!(article_key("L761-1"), "l761-1");
        assert_eq!(article_key("l. 761-1"), "l761-1");
        assert_eq!(article_key("L 761-1"), "l761-1");
        assert_eq!(article_key("R. 313-4"), "r313-4");
        assert_eq!(article_key("LO. 6213-1"), "lo6213-1");
        assert_eq!(article_key("1240"), "1240");
    }

    #[test]
    fn etoiles_et_parentheses_conservees() {
        assert_eq!(article_key("R*212-4"), "r*212-4");
        assert_eq!(article_key("**R. 111-1"), "**r111-1");
        assert_eq!(article_key("(1)"), "(1)");
        assert_eq!(article_key("ANNEXE*"), "annexe*");
    }

    #[test]
    fn separateurs_plies_en_tiret() {
        assert_eq!(article_key("106 Bis"), "106-bis");
        assert_eq!(article_key("11 BIS"), "11-bis");
        assert_eq!(article_key("100/1"), "100-1");
        assert_eq!(article_key("Annexe 1"), "annexe-1");
        assert_eq!(article_key("annexe I"), "annexe-i");
        assert_eq!(article_key("ANNEXE, 2"), "annexe-2");
        assert_eq!(article_key("Préambule."), "preambule");
    }

    #[test]
    fn identity_preserve_les_discriminants_que_la_citation_tronque() {
        // CGI : ordinaux hors vocabulaire citation, discriminants « -0 X ».
        assert_eq!(identity_key("199 duovicies"), "199-duovicies");
        assert_eq!(identity_key("1609 quatertricies"), "1609-quatertricies");
        assert_eq!(identity_key("46 quater-0 W"), "46-quater-0-w");
        assert_eq!(identity_key("41 duovicies-0 H bis"), "41-duovicies-0-h-bis");
        // KALI/JORF : numéros pointés et composés.
        assert_eq!(identity_key("1.01"), "1-01");
        assert_eq!(identity_key("213-6.02 bis"), "213-6-02-bis");
        assert_eq!(identity_key("221-II-1/13"), "221-ii-1-13");
        // LPF : lettres de division + sous-numéros.
        assert_eq!(identity_key("A80 CB-3-1"), "a80-cb-3-1");
    }

    #[test]
    fn identity_plie_le_marqueur_ordinal_de_tete() {
        assert_eq!(identity_key("1er"), "1");
        assert_eq!(identity_key("1ER"), "1");
        assert_eq!(identity_key("1ère"), "1");
        assert_eq!(identity_key("premier"), "1");
        assert_eq!(identity_key("PREMIER"), "1");
        assert_eq!(identity_key("1er bis"), "1-bis");
        assert_eq!(identity_key("1er-1"), "1-1");
        assert_eq!(identity_key("1er (1)"), "1-(1)");
        // Lettre de division ≠ marqueur : « 124 E » et « 2e » restent intacts.
        assert_eq!(identity_key("124 E"), "124-e");
        assert_eq!(identity_key("2e"), "2e");
        // « ermite »-like : marqueur suivi d'alphanumérique = pas un marqueur.
        assert_eq!(identity_key("1ers"), "1ers");
    }

    #[test]
    fn identity_point_fixe() {
        for raw in [
            "199 duovicies",
            "46 quater-0 W",
            "1.01",
            "1er bis",
            "premier",
            "L. 761-1",
            "Annexe 1",
        ] {
            let key = identity_key(raw);
            assert_eq!(identity_key(&key), key, "raw={raw}");
        }
    }

    #[test]
    fn point_fixe() {
        for raw in ["L. 761-1", "R*212-4", "106 Bis", "(1)", "Annexe 1"] {
            let key = article_key(raw);
            assert_eq!(article_key(&key), key);
        }
    }
}
