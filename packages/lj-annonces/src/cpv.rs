//! CPV division inferred from a market's title, PURE.
//!
//! Naive Bayes, table trained offline on the markets their sources labelled and
//! embedded by `include_str!`. A code the source declared always wins over it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::NatureMarche;
use lj_fold::fold;

const TABLE_TSV: &str = include_str!("../data/cpv_divisions_fr.tsv");

/// Lengths training kept. A token outside them carries no weight in the table
/// and would skew the score by the per-division default alone.
const LONGUEUR: std::ops::RangeInclusive<usize> = 4..=24;

struct Modele {
    vides: HashSet<&'static str>,
    /// Log-prior and default log-weight of a division.
    divisions: Vec<(&'static str, f32, f32)>,
    /// Per token, its gap to the default of each division it was seen in.
    poids: HashMap<&'static str, Vec<(usize, f32)>>,
}

static MODELE: LazyLock<Modele> = LazyLock::new(|| {
    let mut vides = HashSet::new();
    let mut divisions: Vec<(&str, f32, f32)> = Vec::new();
    let mut rang: HashMap<&str, usize> = HashMap::new();
    let mut poids: HashMap<&str, Vec<(usize, f32)>> = HashMap::new();

    for ligne in TABLE_TSV.lines() {
        if ligne.starts_with('#') || ligne.is_empty() {
            continue;
        }
        let mut champs = ligne.split('\t');
        let tete = champs.next().unwrap_or_default();
        match tete {
            "!" => vides.extend(champs.next().unwrap_or_default().split_whitespace()),
            "@" => {
                let division = champs.next().expect("division");
                let prior: f32 = champs.next().expect("prior").parse().expect("prior");
                let defaut: f32 = champs.next().expect("defaut").parse().expect("defaut");
                rang.insert(division, divisions.len());
                divisions.push((division, prior / 100.0, defaut / 100.0));
            }
            jeton => {
                let ecarts = champs
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|p| {
                        let (d, v) = p.split_once(':')?;
                        Some((*rang.get(d)?, v.parse::<f32>().ok()? / 100.0))
                    })
                    .collect();
                poids.insert(jeton, ecarts);
            }
        }
    }
    Modele {
        vides,
        divisions,
        poids,
    }
});

/// Division to market nature, kept where one nature holds at least 85 % of the
/// markets sources labelled both ways. `41` is dropped, split between works and
/// services.
const NATURE_PAR_DIVISION: [(&str, NatureMarche); 38] = [
    ("03", NatureMarche::Fournitures),
    ("09", NatureMarche::Fournitures),
    ("14", NatureMarche::Fournitures),
    ("15", NatureMarche::Fournitures),
    ("16", NatureMarche::Fournitures),
    ("18", NatureMarche::Fournitures),
    ("19", NatureMarche::Fournitures),
    ("22", NatureMarche::Fournitures),
    ("24", NatureMarche::Fournitures),
    ("30", NatureMarche::Fournitures),
    ("33", NatureMarche::Fournitures),
    ("34", NatureMarche::Fournitures),
    ("37", NatureMarche::Fournitures),
    ("38", NatureMarche::Fournitures),
    ("39", NatureMarche::Fournitures),
    ("43", NatureMarche::Fournitures),
    ("45", NatureMarche::Travaux),
    ("50", NatureMarche::Services),
    ("51", NatureMarche::Services),
    ("55", NatureMarche::Services),
    ("60", NatureMarche::Services),
    ("63", NatureMarche::Services),
    ("64", NatureMarche::Services),
    ("65", NatureMarche::Services),
    ("66", NatureMarche::Services),
    ("70", NatureMarche::Services),
    ("71", NatureMarche::Services),
    ("72", NatureMarche::Services),
    ("73", NatureMarche::Services),
    ("75", NatureMarche::Services),
    ("76", NatureMarche::Services),
    ("77", NatureMarche::Services),
    ("79", NatureMarche::Services),
    ("80", NatureMarche::Services),
    ("85", NatureMarche::Services),
    ("90", NatureMarche::Services),
    ("92", NatureMarche::Services),
    ("98", NatureMarche::Services),
];

/// The same mapping as division and facet uid, for a caller applying it in bulk.
pub fn natures_par_division() -> impl Iterator<Item = (&'static str, &'static str)> {
    NATURE_PAR_DIVISION.iter().map(|(d, n)| (*d, n.facet_uid()))
}

/// The nature a `cpv:NN` implies, `None` where the division does not settle it.
pub fn nature_probable(cpv_uid: &str) -> Option<NatureMarche> {
    let division = cpv_uid.strip_prefix("cpv:")?;
    NATURE_PAR_DIVISION
        .iter()
        .find(|(d, _)| *d == division)
        .map(|(_, n)| *n)
}

fn jetons(titre: &str) -> HashSet<String> {
    fold(titre)
        .split(|c: char| !c.is_alphabetic())
        .filter(|m| LONGUEUR.contains(&m.chars().count()) && !MODELE.vides.contains(m))
        .map(str::to_string)
        .collect()
}

/// `cpv:NN` inferred from the title, `None` only when no token of the title is
/// in the table. Never abstains on a thin margin: answering always puts more
/// markets in their true division, a wrong division and an empty one both being
/// a market its buyer never finds.
pub fn division_probable(titre: &str) -> Option<String> {
    let m = &*MODELE;
    let mut scores: Vec<f32> = m.divisions.iter().map(|(_, prior, _)| *prior).collect();
    let mut vu = false;
    for mot in jetons(titre) {
        let Some(ecarts) = m.poids.get(mot.as_str()) else {
            continue;
        };
        vu = true;
        for (i, (_, _, defaut)) in m.divisions.iter().enumerate() {
            scores[i] += defaut;
        }
        for (i, ecart) in ecarts {
            scores[*i] += ecart;
        }
    }
    if !vu {
        return None;
    }
    let meilleur = (0..scores.len()).max_by(|a, b| scores[*a].total_cmp(&scores[*b]))?;
    Some(format!("cpv:{}", m.divisions[meilleur].0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_table_embarquee_se_lit_entierement() {
        let m = &*MODELE;
        assert_eq!(m.divisions.len(), 45);
        assert!(m.poids.len() > 15_000, "jetons : {}", m.poids.len());
        assert!(m.vides.contains("commune"));
    }

    #[test]
    fn un_titre_sans_aucun_jeton_connu_ne_rend_rien() {
        assert_eq!(division_probable(""), None);
        assert_eq!(division_probable("zzzzqqqq wwwwxxxx"), None);
    }

    #[test]
    fn les_intitules_francs_tombent_sur_leur_division() {
        for (titre, attendu) in [
            (
                "Travaux de réfection de la toiture du groupe scolaire",
                "cpv:45",
            ),
            (
                "Mission de maîtrise d'oeuvre pour la réhabilitation du gymnase",
                "cpv:71",
            ),
            (
                "Fourniture de denrées alimentaires pour la restauration scolaire",
                "cpv:15",
            ),
            ("Nettoyage des locaux administratifs", "cpv:90"),
            ("Contrats d'assurance de la collectivité", "cpv:66"),
        ] {
            assert_eq!(
                division_probable(titre).as_deref(),
                Some(attendu),
                "titre : {titre}"
            );
        }
    }

    /// A title split between two domains gets the better of them rather than
    /// nothing: an empty division is a market its buyer never finds.
    #[test]
    fn un_intitule_qui_hesite_recoit_quand_meme_sa_division() {
        for titre in [
            "Marché de services de nettoyage et d'entretien des locaux",
            "Marché subséquent numéro 3",
        ] {
            assert!(division_probable(titre).is_some(), "titre : {titre}");
        }
    }

    #[test]
    fn la_nature_suit_la_division_quand_la_division_tranche() {
        assert_eq!(nature_probable("cpv:45"), Some(NatureMarche::Travaux));
        assert_eq!(nature_probable("cpv:71"), Some(NatureMarche::Services));
        assert_eq!(nature_probable("cpv:15"), Some(NatureMarche::Fournitures));
        assert_eq!(
            nature_probable("cpv:41"),
            None,
            "water splits between works and services"
        );
        assert_eq!(nature_probable("45"), None, "sans le préfixe");
    }
}
