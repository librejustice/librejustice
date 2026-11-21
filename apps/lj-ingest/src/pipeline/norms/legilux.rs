//! Luxembourg's acts in force on Legilux: the consolidated works state by state, every other act
//! as published, the individual measures left out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use lj_norm::text_aliases;
use lj_sources::legilux::LegiluxWork;
use lj_sources::publisher::Consolidation;
use regex::Regex;

use super::{Target, LUXEMBOURG};

/// An act's states, each stamped with its file and the day it was last modified.
pub(super) struct Act {
    pub(super) adopted: Option<String>,
    pub(super) states: Vec<Consolidation>,
}

pub(super) type Index = HashMap<String, Act>;

/// The title's opening up to the act's year: `Loi du 4 février 2005`.
static LEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^.*?\b\d{4}\b[,.]?\s*(?:-\s*)?").unwrap());
static KEPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:portant\s+)?(?:modifi|complét|abrogeant|portant (?:modification|révision|abrogation)|modification|approbation|approuvant)|budget|crédits?\b|règlement des comptes|portant règlement du compte",
    )
    .unwrap()
});
static INDIVIDUAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)naturalisation|nomination|nommant|démission|agrément|agréant|accordant|qui accorde|autorisant (?:la vente|l'aliénation|l’aliénation|l'acquisition|le gouvernement à (?:vendre|acquérir|céder|aliéner|participer))|aliénation|utilité publique|désignation|portant reconnaissance|reconnaissance légale|convocation|concession|délégation|tracé|^[^,]*\bsieur\b",
    )
    .unwrap()
});

/// Whether an act's title names a measure about given persons, goods or places rather than a rule.
/// An act amending, approving or funding another stays a rule whatever it names.
fn individual(title: &str) -> bool {
    let rest = LEAD.replace(title, "");
    !KEPT.is_match(&rest) && INDIVIDUAL.is_match(&rest)
}

fn nature(kind: &str, key: &str) -> &'static str {
    match kind {
        _ if key.starts_with("CODE-") => "CODE",
        "LOI" => "LOI",
        "Constitution" => "CONSTITUTION",
        "RGD" | "RMIN" => "REGLEMENT",
        _ => "ARRETE",
    }
}

/// Where a file fetched ahead of the lane waits: under its own name.
pub(super) fn cached(cache: &Path, url: &str) -> PathBuf {
    let name = url.rsplit('/').next().unwrap_or(url);
    cache.join("legilux").join(format!("{name}.zst"))
}

/// The lane's targets, consolidated works first then the newest acts, those cached ahead of the
/// others, and each one's states. A code [`LUXEMBOURG`] follows is read from its own listing.
pub(super) fn targets(
    works: Vec<LegiluxWork>,
    cache: &Path,
    selected: &dyn Fn(&str) -> bool,
) -> (Vec<Target>, Index) {
    let (mut scanned, mut individuals) = (0usize, 0usize);
    let mut ordered = Vec::new();
    let mut index = Index::new();
    for w in works {
        let text_uid = format!("LU/{}", w.key);
        if LUXEMBOURG.iter().any(|f| f.text_uid == text_uid) || !selected(&w.key) {
            continue;
        }
        if w.states.is_empty() {
            scanned += 1;
            continue;
        }
        if !w.consolidated && individual(&w.title) {
            individuals += 1;
            continue;
        }
        let states: Vec<Consolidation> = w
            .states
            .into_iter()
            .map(|c| Consolidation {
                token: Some(xxhash_rust::xxh3::xxh3_64(
                    format!("{}{:?}", c.xml_url, c.modified).as_bytes(),
                )),
                ..c
            })
            .collect();
        let current = states
            .iter()
            .fold(String::new(), |all, c| format!("{all}{:?}", c.token));
        let uncached = states.iter().any(|c| !cached(cache, &c.xml_url).exists());
        let path = match w.key.strip_prefix("CODE-") {
            Some(code) => format!("leg/code/{}", code.to_lowercase().replace('-', "_")),
            None if w.key == "CONSTITUTION" => "leg/constitution/1868/10/17/n1".to_string(),
            None if w.key.starts_with("adm/") => format!("{}/jo", w.key),
            None if w.consolidated => format!("leg/{}", w.key),
            None => format!("leg/{}/jo", w.key),
        };
        let title = match (w.key.starts_with("CODE-"), w.title.contains("Luxembourg")) {
            (true, _) => format!("{} luxembourgeois", w.title),
            (false, true) => w.title.clone(),
            (false, false) => format!("{} (Luxembourg)", w.title),
        };
        let target = Target {
            url: format!("https://legilux.public.lu/eli/etat/{path}"),
            text_uid,
            title,
            title_key: None,
            nature: nature(&w.kind, &w.key).to_string(),
            current: Some(xxhash_rust::xxh3::xxh3_64(current.as_bytes())),
            justel: None,
            aliases: text_aliases::in_title("fr", &w.title),
            key: w.key.clone(),
        };
        index.insert(
            w.key,
            Act {
                adopted: w.adopted.map(|d| d.to_string()),
                states,
            },
        );
        ordered.push((
            (uncached, !w.consolidated, std::cmp::Reverse(w.adopted)),
            target,
        ));
    }
    ordered.sort_by_key(|(order, _)| *order);
    tracing::info!(
        acts = ordered.len(),
        scanned,
        individuals,
        "Legilux index read"
    );
    (ordered.into_iter().map(|(_, t)| t).collect(), index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measure_about_given_persons_or_goods_is_individual_and_a_rule_is_not() {
        for title in [
            "Loi du 12 mars 1903 accordant la naturalisation à M. Nicolas Kettenhofer, industriel à Echternach",
            "Arrêté ministériel du 3 mai 2019 portant agrément de la société ProActif SARL en tant que société d’impact sociétal",
            "Arrêté grand-ducal du 2 février 1999 - Administration des Douanes et Accises- Nomination",
            "Loi du 20 juin 2001 autorisant l'aliénation d'un immeuble domanial sis à Eischen",
            "Arrêté royal grand-ducal du 4 juin 1860 qui démet le sieur Kinckels de ses fonctions de conducteur",
        ] {
            assert!(individual(title), "{title}");
        }
        for title in [
            "Loi du 27 juin 2018 instituant le juge aux affaires familiales, portant réforme du divorce et de l’autorité parentale et portant modification de la pension alimentaire",
            "Règlement ministériel du 2 mars 2015 portant publication de l'arrêté ministériel belge du 2 mars 2015 relatif au régime fiscal des tabacs manufacturés",
            "Loi du 27 juillet 1993 portant habilitation pour le Grand-Duc de réglementer certaines matières",
            "Règlement grand-ducal du 20 mai 2021 déclarant zone protégée d’intérêt national sous forme de réserve naturelle la zone « Nommerlayen »",
            "Loi du 15 décembre 2023 portant modification de la loi du 8 mars 2017 portant nomination des membres du Conseil",
            "Loi du 20 décembre 2024 concernant le budget des recettes et des dépenses de l’État pour l’exercice 2025",
        ] {
            assert!(!individual(title), "{title}");
        }
    }
}
