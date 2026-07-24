//! Dédup des listes NER (avocats, cabinets, parties) — les valeurs restent
//! des tranches VERBATIM du texte (ADR 0157), seule la redondance se réduit.

use std::collections::HashMap;

/// `_unique_nonempty`. À doublons (clé casse-insensible), la variante la
/// mieux cassée remplace celle déjà retenue (cf. [`better_cased`]) : les deux
/// sont des tranches verbatim (en-tête parties « Jean DUVAL » vs corps
/// « Jean Duval »), mais la casse mixte porte l'information que les
/// capitales ont détruite.
pub(crate) fn unique_nonempty(values: &[String]) -> Option<Vec<String>> {
    let mut deduped: Vec<String> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for value in values {
        let compact = value.trim();
        if compact.is_empty() {
            continue;
        }
        let key = compact.to_lowercase();
        match seen.get(&key) {
            Some(&i) => {
                if better_cased(&deduped[i], compact) {
                    deduped[i] = compact.to_string();
                }
            }
            None => {
                seen.insert(key, deduped.len());
                deduped.push(compact.to_string());
            }
        }
    }
    if deduped.is_empty() {
        None
    } else {
        Some(deduped)
    }
}

/// `new` est-il mieux cassé que `old` ? Comparaison mot à mot (même nombre de
/// mots) : gain = un mot tout-CAPS (≥ 2 lettres — la convention en-tête met
/// le patronyme seul en capitales) rendu en casse mixte en face ; jamais de
/// gain si l'inverse existe aussi (on ne dégrade aucun mot).
pub(crate) fn better_cased(old: &str, new: &str) -> bool {
    let ow: Vec<&str> = old.split_whitespace().collect();
    let nw: Vec<&str> = new.split_whitespace().collect();
    if ow.len() != nw.len() {
        return false;
    }
    let destroyed = |w: &str| {
        w.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !w.chars().any(char::is_lowercase)
    };
    let mixed = |w: &str| w.chars().any(char::is_lowercase);
    let gain = ow.iter().zip(&nw).any(|(o, n)| destroyed(o) && mixed(n));
    let loss = ow.iter().zip(&nw).any(|(o, n)| destroyed(n) && mixed(o));
    gain && !loss
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

/// La valeur contient un token placeholder d'anonymisation : 1-2 capitales
/// suivies de « ... » en tête de mot (« X... », « Bruno Y... », le composant
/// « M... » de « EUGENE-M... », la queue collée « B...pour » — l'anonymisation
/// source avale parfois l'espace suivante).
fn has_placeholder(value: &str) -> bool {
    let chars: Vec<char> = value.chars().collect();
    for i in 0..chars.len() {
        if !chars[i].is_ascii_uppercase() {
            continue;
        }
        // borne gauche : début de mot (jamais au milieu d'un nom réel tronqué)
        if i > 0 && chars[i - 1].is_alphanumeric() {
            continue;
        }
        let mut j = i + 1;
        if j < chars.len() && chars[j].is_ascii_uppercase() {
            j += 1;
        }
        if chars[j..].starts_with(&['.', '.', '.']) {
            return true;
        }
    }
    false
}

/// École « noms anonymisés » (2026-07-19) — PERSONNE : un nom dont le
/// patronyme (n'importe quel composant) est un placeholder ne s'émet pas ;
/// le prénom réel ne le sauve pas (« Bruno Y... »). Jamais résolvable (pas
/// de résolution CNB sur un prénom), aucune valeur produit. La mention reste
/// extraite en interne (attribution des côtés) — le filtre ne vit qu'à
/// l'émission.
pub(crate) fn is_anonymized_person(value: &str) -> bool {
    has_placeholder(value)
}

/// Mots non identifiants d'un nom de cabinet : formes sociales et génériques.
const FIRM_NOISE: &[&str] = &[
    "SCP",
    "SELARL",
    "SELEURL",
    "SELAS",
    "SELASU",
    "SELAFA",
    "AARPI",
    "SAS",
    "SARL",
    "SA",
    "SCM",
    "EARL",
    "CABINET",
    "AVOCAT",
    "AVOCATS",
    "ASSOCIES",
    "ASSOCIÉS",
    "D'AVOCATS",
    "SOCIETE",
    "SOCIÉTÉ",
    "ET",
    "DE",
    "LA",
    "LE",
    "DU",
    "DES",
    "&",
];

/// École « noms anonymisés » — CABINET : ne s'émet pas SEULEMENT si
/// tout-placeholder (« SCP F... N... G... », « A... D'AVOCATS ») ; un token
/// identifiant réel le sauve (« SELARL FONTENEAU - B... - MARCHAND »).
pub(crate) fn is_anonymized_firm(value: &str) -> bool {
    if !has_placeholder(value) {
        return false;
    }
    !value
        .split(|c: char| c.is_whitespace() || c == ',' || c == '-' || c == '/')
        .filter(|t| !t.is_empty())
        .any(|t| {
            let up = t
                .trim_matches(|c: char| c == '.' || c == ',')
                .to_uppercase();
            !FIRM_NOISE.contains(&up.as_str())
                && !has_placeholder(t)
                && t.chars().filter(|c| c.is_alphabetic()).count() >= 2
        })
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

    // Spec : école « noms anonymisés » (2026-07-19) — personne au patronyme
    // placeholder = pas d'émission, le prénom réel ne sauve pas.
    #[test]
    fn anonymized_person_placeholder_patronyme() {
        for v in [
            "X...",
            "Bruno Y...",
            "B... D...",
            "B...pour",
            "Johann EUGENE-M...",
        ] {
            assert!(is_anonymized_person(v), "{v:?} devrait être filtré");
        }
        for v in [
            "Moustardier",
            "Le Mière",
            "Benjamin PEYRELEVADE",
            "S.C.P. Lyon-Caen",
        ] {
            assert!(!is_anonymized_person(v), "{v:?} devrait passer");
        }
    }

    // Spec : cabinet filtré SEULEMENT si tout-placeholder — un token
    // identifiant réel le sauve.
    #[test]
    fn anonymized_firm_tout_placeholder_seulement() {
        for v in ["SCP F... N... G...", "A... D'AVOCATS", "SCP Z...-A..."] {
            assert!(is_anonymized_firm(v), "{v:?} devrait être filtré");
        }
        for v in [
            "SELARL FONTENEAU - B... - MARCHAND",
            "SCP FRANCOIS-CARREAU FRANCOIS TRAMIER Z...",
            "B... - FIRKOWSKI",
            "SCP Jeanne K...",
            "OSBORNE CLARKE",
        ] {
            assert!(!is_anonymized_firm(v), "{v:?} devrait passer");
        }
    }

    #[test]
    fn unique_nonempty_prefers_mixed_case_over_all_caps() {
        // En-tête parties en capitales puis corps en casse mixte : la tranche
        // mixte remplace la CAPS (position conservée), jamais l'inverse.
        let out = unique_nonempty(&[
            "Jean DUVAL".to_string(),
            "Martin".to_string(),
            "Jean Duval".to_string(),
            "MARTIN".to_string(),
        ]);
        assert_eq!(
            out,
            Some(vec!["Jean Duval".to_string(), "Martin".to_string()])
        );
    }
}
