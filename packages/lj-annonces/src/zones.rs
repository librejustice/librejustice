//! Geographic scope of a public-procurement notice, at one grain: the
//! département. A région is exactly a set of départements, so it is expanded
//! into them here and never offered as a checkbox; what is left of it groups
//! the list under a heading.
//!
//! [`Zone::FranceEntiere`] and [`Zone::UnionEuropeenne`] sit outside the grain
//! because they are not places: a national framework agreement has no
//! département, and dropping it from a departmental filter would be the
//! symmetric mistake of keeping a single département out of several.
//!
//! `data/zones_fr.tsv` comes from the Eurostat NUTS 2024 classification, where
//! NUTS3 is the département and NUTS1 the région.

use std::collections::HashMap;
use std::sync::LazyLock;

use lj_fold::slug;

/// `departement TAB nuts3 TAB region`, header on the first line. Overseas
/// collectivities carry neither NUTS nor région.
const ZONES_TSV: &str = include_str!("../data/zones_fr.tsv");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Zone {
    /// Canonical département code: `75`, `2A`, `973`.
    Departement(String),
    FranceEntiere,
    UnionEuropeenne,
}

use lj_vocab::{ZONE_EU, ZONE_FRANCE};

impl Zone {
    pub fn code(&self) -> &str {
        match self {
            Zone::Departement(d) => d,
            Zone::FranceEntiere => ZONE_FRANCE,
            Zone::UnionEuropeenne => ZONE_EU,
        }
    }

    pub fn parse(code: &str) -> Option<Zone> {
        match code {
            ZONE_FRANCE => Some(Zone::FranceEntiere),
            ZONE_EU => Some(Zone::UnionEuropeenne),
            d if REGION_DU_DEP.contains_key(d) => Some(Zone::Departement(d.to_string())),
            _ => None,
        }
    }

    /// True for the two values that are not places: they widen a departmental
    /// filter instead of narrowing it.
    pub fn est_sans_lieu(&self) -> bool {
        !matches!(self, Zone::Departement(_))
    }
}

struct Ligne {
    departement: &'static str,
    nuts3: &'static str,
    region: &'static str,
}

/// A malformed line in the embedded TSV is a curation bug: hard error.
static TABLE: LazyLock<Vec<Ligne>> = LazyLock::new(|| {
    ZONES_TSV
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut c = l.split('\t');
            match (c.next(), c.next(), c.next()) {
                (Some(d), Some(n), Some(r)) => Ligne {
                    departement: d,
                    nuts3: n,
                    region: r,
                },
                _ => panic!("zones_fr.tsv: line with fewer than three columns: {l:?}"),
            }
        })
        .collect()
});

static PAR_NUTS3: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    TABLE
        .iter()
        .filter(|l| !l.nuts3.is_empty())
        .map(|l| (l.nuts3, l.departement))
        .collect()
});

static REGION_DU_DEP: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| TABLE.iter().map(|l| (l.departement, l.region)).collect());

static DEPS_DE_REGION: LazyLock<HashMap<String, Vec<&'static str>>> = LazyLock::new(|| {
    let mut m: HashMap<String, Vec<&'static str>> = HashMap::new();
    for l in TABLE.iter().filter(|l| !l.region.is_empty()) {
        m.entry(slug(l.region, '-'))
            .or_default()
            .push(l.departement);
    }
    m
});

/// Longest name first, so a label never matches two régions by inclusion.
static REGIONS_REPLIEES: LazyLock<Vec<(String, &'static str)>> = LazyLock::new(|| {
    let mut v: Vec<(String, &'static str)> = TABLE
        .iter()
        .filter(|l| !l.region.is_empty())
        .map(|l| (slug(l.region, '-'), l.region))
        .collect();
    v.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
    v.dedup_by(|a, b| a.0 == b.0);
    v
});

/// The stored form of a set of zones, which is what the column carries and what
/// a facet key is.
pub fn codes(zones: &[Zone]) -> Vec<String> {
    zones.iter().map(|z| z.code().to_string()).collect()
}

/// Normalises raw département codes and keeps the ones the referential knows,
/// sorted and deduplicated. This is how a connector turns what its source
/// publishes, one code or a list of them, into zones.
pub fn zones_departements<S: AsRef<str>>(codes: impl IntoIterator<Item = S>) -> Vec<Zone> {
    let mut v: Vec<Zone> = codes
        .into_iter()
        .filter_map(|c| crate::normalise_departement(c.as_ref()))
        .map(Zone::Departement)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `FRY30` gives `973`. `None` for a foreign NUTS, for `FRZZZ`, and for any
/// level other than 3.
pub fn departement_depuis_nuts3(code: &str) -> Option<&'static str> {
    PAR_NUTS3.get(code.trim().to_uppercase().as_str()).copied()
}

/// `None` for an overseas collectivity, which belongs to no région.
pub fn region_du_departement(code: &str) -> Option<&'static str> {
    REGION_DU_DEP
        .get(code)
        .copied()
        .filter(|r: &&str| !r.is_empty())
}

/// Accents, case and separators are indifferent. Empty when the name is not a
/// région.
pub fn departements_de_region(nom: &str) -> &'static [&'static str] {
    DEPS_DE_REGION
        .get(&slug(nom, '-'))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The département a label names, when it names one. Longest name first, so
/// « Charente-Maritime » is never read as « Charente ».
pub fn departement_cite(libelle: &str) -> Option<&'static str> {
    static REPLIES: LazyLock<Vec<(String, &'static str)>> = LazyLock::new(|| {
        let mut v: Vec<(String, &'static str)> = TABLE
            .iter()
            .filter_map(|l| {
                lj_norm::referential_labels::departement_label(l.departement)
                    .map(|nom| (slug(nom, '-'), l.departement))
            })
            .collect();
        v.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        v
    });
    let replie = slug(libelle, '-');
    REPLIES
        .iter()
        .find(|(nom, _)| contient_mot(&replie, nom))
        .map(|(_, dep)| *dep)
}

/// The région a buyer label names, when it names one. This is what gives a
/// zone to the channels that publish no code at all.
pub fn region_citee(libelle: &str) -> Option<&'static str> {
    let replie = slug(libelle, '-');
    REGIONS_REPLIEES
        .iter()
        .find(|(nom, _)| contient_mot(&replie, nom))
        .map(|(_, region)| *region)
}

/// Zones a place label names, widest reading of a free-text field: a
/// département, else every département of the région, else the whole country.
/// Dropping « France métropolitaine » takes the consultation out of every
/// filter rather than into all of them, which is the opposite of what it says.
pub fn zones_du_libelle(libelle: &str) -> Vec<Zone> {
    const NATIONAL: [&str; 3] = ["france-metropolitaine", "france-entiere", "toute-la-france"];
    if let Some(d) = departement_cite(libelle) {
        return zones_departements([d]);
    }
    if let Some(r) = region_citee(libelle) {
        return zones_departements(departements_de_region(r));
    }
    let replie = slug(libelle, '-');
    if NATIONAL.iter().any(|n| replie.contains(n)) {
        return vec![Zone::FranceEntiere];
    }
    Vec::new()
}

/// Substring on a word boundary: « Corse » does not read inside « Écorse », and
/// « Normandie » does read inside « Basse-Normandie ».
fn contient_mot(foin: &str, aiguille: &str) -> bool {
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
    foin.match_indices(aiguille).any(|(i, _)| {
        !alnum(foin[..i].chars().next_back()) && !alnum(foin[i + aiguille.len()..].chars().next())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_libelle_de_departement_rend_son_code() {
        assert_eq!(departement_cite("Charente-Maritime"), Some("17"));
        assert_eq!(departement_cite("Charente"), Some("16"));
        assert_eq!(
            departement_cite("Lieu d'exécution : Hauts-de-Seine"),
            Some("92")
        );
        assert_eq!(departement_cite("HAUTE-GARONNE"), Some("31"));
        assert_eq!(departement_cite("Corse-du-Sud"), Some("2A"));
        assert_eq!(departement_cite("La Réunion"), Some("974"));
        assert_eq!(departement_cite("France entière"), None);
        assert_eq!(departement_cite(""), None);
    }

    /// A free-text field spells a compound name however the buyer typed it, so
    /// the comparison runs on the slug: « Haute Saône » is the référentiel's
    /// « Haute-Saône », and the word boundary a separator carried survives.
    #[test]
    fn le_separateur_d_un_nom_compose_est_indifferent() {
        assert_eq!(departement_cite("Haute Saône"), Some("70"));
        assert_eq!(departement_cite("COTE D OR"), Some("21"));
        assert_eq!(departement_cite("Hauts de Seine"), Some("92"));
        assert_eq!(region_citee("Pays de la Loire"), Some("Pays de la Loire"));
        assert_eq!(departements_de_region("ile de france").len(), 8);
        assert_eq!(
            codes(&zones_du_libelle("France Métropolitaine")),
            [ZONE_FRANCE]
        );
        assert!(departement_cite("Écorse").is_none());
    }

    #[test]
    fn le_referentiel_porte_les_101_departements_et_les_18_regions() {
        assert_eq!(TABLE.len(), 109);
        assert_eq!(PAR_NUTS3.len(), 101);
        let regions: std::collections::HashSet<&str> = TABLE
            .iter()
            .map(|l| l.region)
            .filter(|r| !r.is_empty())
            .collect();
        assert_eq!(regions.len(), 18);
    }

    #[test]
    fn le_nuts3_rend_le_departement() {
        assert_eq!(departement_depuis_nuts3("FRY30"), Some("973"));
        assert_eq!(departement_depuis_nuts3("FR101"), Some("75"));
        assert_eq!(departement_depuis_nuts3("FRM01"), Some("2A"));
        assert_eq!(departement_depuis_nuts3("fri12"), Some("33"));
        assert_eq!(departement_depuis_nuts3("CH025"), None);
        assert_eq!(departement_depuis_nuts3("FRZZZ"), None);
        assert_eq!(departement_depuis_nuts3("FRY3"), None);
    }

    #[test]
    fn une_region_s_eclate_en_ses_departements() {
        assert_eq!(departements_de_region("Guyane"), ["973"]);
        assert_eq!(departements_de_region("Corse"), ["2A", "2B"]);
        assert_eq!(departements_de_region("nouvelle-aquitaine").len(), 12);
        assert_eq!(departements_de_region("Ile-de-France").len(), 8);
        assert_eq!(departements_de_region("Occitanie").len(), 13);
        assert!(departements_de_region("Aquitaine").is_empty());
    }

    #[test]
    fn la_region_se_lit_dans_le_libelle_d_acheteur() {
        assert_eq!(
            region_citee("Direction Régionale de Guyane"),
            Some("Guyane")
        );
        assert_eq!(
            region_citee("Conseil régional de Nouvelle-Aquitaine"),
            Some("Nouvelle-Aquitaine")
        );
        assert_eq!(region_citee("Commune de Vimy"), None);
    }

    #[test]
    fn le_departement_rend_sa_region_et_la_com_n_en_a_pas() {
        assert_eq!(region_du_departement("973"), Some("Guyane"));
        assert_eq!(region_du_departement("08"), Some("Grand Est"));
        assert_eq!(region_du_departement("975"), None);
        assert_eq!(region_du_departement("99"), None);
    }

    #[test]
    fn les_deux_valeurs_sans_lieu_ne_collisionnent_pas_avec_un_departement() {
        assert_eq!(Zone::parse("fr"), Some(Zone::FranceEntiere));
        assert_eq!(Zone::parse("eu"), Some(Zone::UnionEuropeenne));
        assert_eq!(
            Zone::parse("973"),
            Some(Zone::Departement("973".to_string()))
        );
        assert_eq!(Zone::parse("20"), None);
        assert!(Zone::FranceEntiere.est_sans_lieu());
        assert!(!Zone::Departement("973".to_string()).est_sans_lieu());
    }
}
