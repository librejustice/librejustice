//! Usage names of decisions, « Danthony », « Duvignères », which pseudonymisation strips from the
//! decision's own text: the pure part, the fold that keys them and the n-grams a query offers.

use std::collections::HashMap;

use crate::text::normalize_spaces;
use lj_fold::fold;

/// Le nom d'usage le plus long qu'une requête puisse porter, en mots :
/// « Section française de l'Observatoire international des prisons » en fait
/// sept.
pub const MAX_NAME_WORDS: usize = 8;

/// Clé de comparaison d'un nom d'usage : accents, casse et ponctuation repliés,
/// pour que « Association AC ! » et « Association AC! » soient un seul nom. La
/// projection et la résolution passent toutes deux par elle.
pub fn case_name_key(nom: &str) -> String {
    let sans_ponctuation: String = fold(nom)
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    normalize_spaces(&sans_ponctuation).trim().to_string()
}

/// Rarity threshold of a short key, a place or a company written far more than
/// cited.
pub const SHORT_NAME_RARITY: i64 = 20_000;

/// The heads a named party carries before what distinguishes it. The corpus
/// cites the remainder alone more often: « jurisprudence Béziers », « Béziers II ».
const GENERIC_HEADS: &[&str] = &[
    "commune",
    "ville",
    "departement",
    "region",
    "societe",
    "ste",
    "sa",
    "sarl",
    "sas",
    "sci",
];

const HEAD_ARTICLES: &[&str] = &["de", "du", "des", "d", "la", "le", "les", "l"];

/// The short key a full key derives: what follows a generic head, articles
/// dropped. `commune de beziers` gives `beziers`; a patronym gives nothing.
pub fn short_case_name_key(key: &str) -> Option<String> {
    let mut words = key.split(' ');
    if !GENERIC_HEADS.contains(&words.next()?) {
        return None;
    }
    let short = words
        .skip_while(|w| HEAD_ARTICLES.contains(w))
        .collect::<Vec<_>>()
        .join(" ");
    (short.chars().count() >= 3).then_some(short)
}

/// Context a name supports around it, in words: a query naming its decision
/// asks a question around it, past twenty words it is a pasted paragraph.
const MAX_CONTEXT_WORDS: usize = 20;

/// Les clés que la requête peut désigner, la plus longue d'abord : tous ses
/// n-grammes contigus jusqu'à [`MAX_NAME_WORDS`] mots. Vide au-delà du contexte
/// que [`MAX_CONTEXT_WORDS`] autorise.
pub fn candidate_keys(query: &str) -> Vec<String> {
    let cle = case_name_key(query);
    let mots: Vec<&str> = cle.split_whitespace().collect();
    if mots.is_empty() || mots.len() > MAX_NAME_WORDS + MAX_CONTEXT_WORDS {
        return Vec::new();
    }
    let mut cles = Vec::new();
    for taille in (1..=mots.len().min(MAX_NAME_WORDS)).rev() {
        if mots.len() - taille > MAX_CONTEXT_WORDS {
            continue;
        }
        for debut in 0..=mots.len() - taille {
            cles.push(mots[debut..debut + taille].join(" "));
        }
    }
    cles
}

/// How many decisions a name may designate by its words; the card shows the most cited.
/// « Simmenthal » names 7 decisions of the CJUE, « Bosman » 3.
pub const NAME_BEARERS_MAX: usize = 10;

/// The words of the named decisions' keys, each to the named decisions whose keys hold it. A
/// named decision is its rank in the order the index was built from.
#[derive(Debug, Default, Clone)]
pub struct NameIndex {
    words: HashMap<String, Vec<u32>>,
    keys: Vec<Vec<String>>,
}

impl NameIndex {
    pub fn new(named: impl IntoIterator<Item = Vec<String>>) -> Self {
        let mut index = NameIndex::default();
        for keys in named {
            let rank = index.keys.len() as u32;
            let mut words: Vec<&str> = keys
                .iter()
                .flat_map(|k| k.split(' '))
                .filter(|w| !w.is_empty())
                .collect();
            words.sort_unstable();
            words.dedup();
            for word in words {
                index.words.entry(word.to_string()).or_default().push(rank);
            }
            index.keys.push(keys);
        }
        index
    }

    pub fn keys(&self, rank: usize) -> &[String] {
        &self.keys[rank]
    }

    /// The named decisions whose keys hold every word.
    pub fn holding(&self, words: &[&str]) -> Vec<usize> {
        let mut lists: Vec<&Vec<u32>> = Vec::with_capacity(words.len());
        for word in words {
            match self.words.get(*word) {
                Some(ranks) => lists.push(ranks),
                None => return Vec::new(),
            }
        }
        lists.sort_unstable_by_key(|ranks| ranks.len());
        let Some((first, rest)) = lists.split_first() else {
            return Vec::new();
        };
        first
            .iter()
            .filter(|rank| rest.iter().all(|ranks| ranks.binary_search(rank).is_ok()))
            .map(|&rank| rank as usize)
            .collect()
    }

    /// The named decisions whose keys hold every word, or only those whose key the words
    /// write whole when some do: « Nicolo » is not Nicolò Santilli's name. Empty past `max`.
    pub fn bearers(&self, words: &[&str], max: usize) -> Vec<usize> {
        let holding = self.holding(words);
        let whole: Vec<usize> = holding
            .iter()
            .copied()
            .filter(|&rank| {
                self.keys[rank].iter().any(|k| {
                    crate::citation::name_words(k)
                        .iter()
                        .all(|w| words.contains(w))
                })
            })
            .collect();
        let bearers = match whole.is_empty() {
            true => holding,
            false => whole,
        };
        match bearers.len() <= max {
            true => bearers,
            false => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_bears_every_word_and_too_many_bearers_name_nothing() {
        let index = NameIndex::new(
            [
                "Commune de Cassis",
                "Rewe-Zentral AG c. Bundesmonopolverwaltung (Cassis de Dijon)",
                "Commune de Dijon",
                "Nicolo",
                "Nicolò Santilli c. Italie",
            ]
            .map(|name| vec![case_name_key(name)]),
        );
        assert_eq!(index.bearers(&["cassis", "dijon"], 10), vec![1]);
        assert_eq!(index.bearers(&["commune"], 10), vec![0, 2]);
        assert!(index.bearers(&["commune"], 1).is_empty());
        assert!(index.bearers(&["cassis", "blanco"], 10).is_empty());
        assert!(index.bearers(&[], 10).is_empty());
        assert_eq!(index.bearers(&["nicolo"], 10), vec![3]);
        assert_eq!(index.bearers(&["santilli"], 10), vec![4]);
    }

    #[test]
    fn la_ponctuation_du_nom_ne_compte_pas() {
        assert_eq!(case_name_key("Association AC !"), "association ac");
        assert_eq!(
            case_name_key("Département de Tarn-et-Garonne"),
            case_name_key("departement de tarn et garonne")
        );
    }

    #[test]
    fn le_nom_court_suit_la_tete_generique() {
        for (cle, court) in [
            ("commune de beziers", Some("beziers")),
            ("commune de la garde", Some("garde")),
            ("departement de l eure", Some("eure")),
            ("ville de paris", Some("paris")),
            (
                "societe tropic travaux signalisation",
                Some("tropic travaux signalisation"),
            ),
            ("sa x", None),
            ("danthony", None),
            ("commission c italie", None),
            ("commune", None),
        ] {
            assert_eq!(short_case_name_key(cle).as_deref(), court, "clé {cle:?}");
        }
    }

    #[test]
    fn le_nom_seul_ou_avec_son_contexte_est_candidat() {
        for q in [
            "danthony",
            "arrêt Danthony",
            "CE 2011 Danthony consultation irrégulière",
        ] {
            assert!(
                candidate_keys(q).contains(&"danthony".to_string()),
                "requête {q:?}"
            );
        }
    }

    #[test]
    fn le_nom_survit_a_une_question_de_quinze_mots() {
        for (q, nom) in [
            (
                "jurisprudence Danthony irrégularité de la consultation préalable effet sur le \
                 sens de la décision prise",
                "danthony",
            ),
            (
                "Czabaj recours contre une décision individuelle sans mention des voies de \
                 recours délai d'un an opposable",
                "czabaj",
            ),
            (
                "GISTI circulaire impérative interprétation du droit positif recours pour \
                 excès de pouvoir recevabilité contre les lignes directrices",
                "gisti",
            ),
        ] {
            assert!(
                candidate_keys(q).contains(&nom.to_string()),
                "requête {q:?}"
            );
        }
    }

    /// Un paragraphe collé n'est pas une demande d'arrêt : le nom qu'il contient
    /// y est de passage.
    #[test]
    fn le_paragraphe_colle_ne_nomme_rien() {
        let colle = "le juge des référés peut ordonner toutes mesures utiles sans faire obstacle \
                     à l'exécution d'aucune décision administrative, lorsque la demande \
                     présente un caractère d'urgence et que la mesure sollicitée est utile au \
                     litige principal";
        assert!(candidate_keys(colle).is_empty());
    }

    #[test]
    fn le_nom_le_plus_long_est_propose_avant_son_raccourci() {
        let cles = candidate_keys("société tropic travaux signalisation");
        let rang = |c: &str| cles.iter().position(|k| k == c).expect(c);
        assert!(rang("societe tropic travaux signalisation") < rang("tropic"));
    }

    /// A common surname inside a question competes, and length does not rule it
    /// out: « Marie » is in so many decisions that the projection leaves it no key
    /// to designate.
    #[test]
    fn le_patronyme_courant_reste_a_la_projection() {
        assert!(candidate_keys(
            "un détenu nommé Marie conteste la sanction disciplinaire que le directeur \
             lui a infligée sans consulter la commission"
        )
        .contains(&"marie".to_string()));
    }
}
