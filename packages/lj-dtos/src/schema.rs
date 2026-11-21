//! Canonical enums of the decision taxonomy, shared by the API and the web.
//!
//! One schema for the administrative order (CE/CAA/TA/CNDA/Cour des comptes/TC)
//! and the judicial order (Cour de cassation/CA/TJ/TCOM). The facet enums
//! ([`Solution`], [`Domain`]) are the compiled mirror of the `facet_value` seed:
//! each serialised value is the suffix of a uid, and `enums_match_migration_seed`
//! diffs enum against seed, so a drift is a red build. The other enums keep the
//! SCREAMING_CASE strings of the Postgres `text` columns.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JurisdictionLevel {
    // Ordre administratif
    #[serde(rename = "TRIBUNAL_ADMINISTRATIF")]
    TribunalAdministratif,
    #[serde(rename = "COUR_ADMINISTRATIVE_APPEL")]
    CourAdministrativeAppel,
    #[serde(rename = "CONSEIL_D_ETAT")]
    ConseilDEtat,
    #[serde(rename = "TRIBUNAL_DES_CONFLITS")]
    TribunalDesConflits,
    #[serde(rename = "COUR_NATIONALE_DROIT_ASILE")]
    CourNationaleDroitAsile,
    #[serde(rename = "COUR_DES_COMPTES")]
    CourDesComptes,
    // Ordre judiciaire
    #[serde(rename = "COUR_DE_CASSATION")]
    CourDeCassation,
    #[serde(rename = "COUR_APPEL")]
    CourAppel,
    #[serde(rename = "TRIBUNAL_JUDICIAIRE")]
    TribunalJudiciaire,
    #[serde(rename = "TRIBUNAL_COMMERCE")]
    TribunalCommerce,
    #[serde(rename = "AUTRE")]
    Autre,
}

/// Solution: what became of the claim, whatever the court, the roots of the
/// `solution:*` seed. A decision carries its court's own word, a leaf of
/// [`Solution::LEAVES`], and a filter on a root matches its leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Solution {
    Granted,
    PartlyGranted,
    Dismissed,
    Inadmissible,
    Closed,
    Other,
}

impl Solution {
    /// The roots, in the seed's `sort` order.
    pub const ALL: [Solution; 6] = [
        Solution::Granted,
        Solution::PartlyGranted,
        Solution::Dismissed,
        Solution::Inadmissible,
        Solution::Closed,
        Solution::Other,
    ];

    /// The roots' codes, in declaration order.
    const CODES: [&'static str; 6] = [
        "GRANTED",
        "PARTLY_GRANTED",
        "DISMISSED",
        "INADMISSIBLE",
        "CLOSED",
        "OTHER",
    ];

    fn code(self) -> &'static str {
        Self::CODES[self as usize]
    }

    /// Every leaf, a `solution:*` uid suffix, under its root, in the seed's order.
    pub const LEAVES: [(&'static str, Solution); 27] = [
        ("SATISFACTION_TOTALE", Solution::Granted),
        ("CASSATION", Solution::Granted),
        ("ANNULATION", Solution::Granted),
        ("INFIRMATION", Solution::Granted),
        ("NON_CONFORMITE", Solution::Granted),
        ("INELIGIBILITE", Solution::Granted),
        ("VIOLATION", Solution::Granted),
        ("ADMISSION", Solution::Granted),
        ("SATISFACTION_PARTIELLE", Solution::PartlyGranted),
        ("CASSATION_PARTIELLE", Solution::PartlyGranted),
        ("ANNULATION_PARTIELLE", Solution::PartlyGranted),
        ("INFIRMATION_PARTIELLE", Solution::PartlyGranted),
        ("REFORMATION", Solution::PartlyGranted),
        ("ADMISSION_PARTIELLE", Solution::PartlyGranted),
        ("REJET", Solution::Dismissed),
        ("CONFIRMATION", Solution::Dismissed),
        ("CONFORMITE", Solution::Dismissed),
        ("NON_VIOLATION", Solution::Dismissed),
        ("IRRECEVABILITE", Solution::Inadmissible),
        ("NON_ADMISSION", Solution::Inadmissible),
        ("DECHEANCE", Solution::Inadmissible),
        ("DESISTEMENT", Solution::Closed),
        ("NON_LIEU_A_STATUER", Solution::Closed),
        ("REGLEMENT_AMIABLE", Solution::Closed),
        ("RADIATION", Solution::Closed),
        ("AUTRE", Solution::Other),
        ("RECEVABLE", Solution::Other),
    ];

    pub fn leaves(self) -> impl Iterator<Item = &'static str> {
        Self::LEAVES
            .iter()
            .filter(move |(_, root)| *root == self)
            .map(|(leaf, _)| *leaf)
    }

    /// The root of a leaf code (`"REJET"` → [`Solution::Dismissed`]).
    pub fn of_leaf(leaf: &str) -> Option<Solution> {
        Self::LEAVES
            .iter()
            .find(|(code, _)| *code == leaf)
            .map(|(_, root)| *root)
    }
}

impl Serialize for Solution {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

/// A leaf's code reads as its root: the leaves were the filter's values before
/// the roots, and published guides still send them.
impl<'de> Deserialize<'de> for Solution {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Solution::ALL
            .into_iter()
            .find(|root| root.code() == code)
            .or_else(|| Solution::of_leaf(&code))
            .ok_or_else(|| serde::de::Error::unknown_variant(&code, &Solution::CODES))
    }
}

/// Legal domain tree, mirror of the `legal_domain:*` rows of `facet_value`: 9 roots
/// and 45 leaves, a leaf's code prefixed by its root's. A selected root matches
/// itself and all its leaves (expanded on the API side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Domain {
    // Roots.
    Civil,
    Commercial,
    Public,
    Social,
    Fiscal,
    ProprieteIntellectuelle,
    Europeen,
    Criminel,
    Constitutionnel,
    // CIVIL leaves.
    CivilProceduresCivilesExecution,
    CivilDroitImmobilierConstruction,
    CivilDroitLocatif,
    CivilDroitPersonnesFamille,
    CivilDroitCoproprieteProprieteImmobiliere,
    CivilDroitAssurances,
    CivilDroitResponsabilite,
    CivilDroitBancaireBoursier,
    CivilDroitSuccessions,
    CivilDroitExpropriationPreemption,
    CivilDivorceSeparationCorps,
    CivilDroitRural,
    CivilDroitResponsabiliteContrats,
    CivilDroitSaisieImmobiliere,
    CivilDroitMineurs,
    CivilDroitInternationalPrive,
    // COMMERCIAL leaves.
    CommercialDroitEntreprisesDifficulte,
    CommercialDroitBancaireBoursier,
    CommercialDroitContrats,
    CommercialDroitSocietes,
    CommercialDroitNumerique,
    CommercialDroitTransport,
    CommercialDroitAssurances,
    CommercialDroitConcurrence,
    CommercialDroitConsommation,
    CommercialDroitArbitrage,
    // PUBLIC leaves.
    PublicDroitEtrangersNationalite,
    PublicDroitUrbanismeImmobilierPublic,
    PublicDroitTravail,
    PublicDroitPenalPublic,
    PublicDroitAideActionSociale,
    PublicDroitEnvironnement,
    PublicDroitLibertesFondamentales,
    PublicDroitCommandePublique,
    // SOCIAL leaves.
    SocialDroitTravail,
    SocialDroitAideActionSociale,
    SocialDroitPenalSocial,
    // FISCAL leaves.
    FiscalDroitDouanier,
    // PROPRIETE_INTELLECTUELLE leaves.
    ProprieteIntellectuelleIndustrielle,
    ProprieteIntellectuelleLitteraireArtistique,
    // EUROPEEN leaves.
    EuropeenLibertesCirculation,
    EuropeenAidesEtat,
    EuropeenPolitiqueAgricole,
    EuropeenActionExterieure,
    EuropeenDroitInstitutionnel,
}

impl Domain {
    /// Every variant, the roots then the leaves grouped by root.
    pub const ALL: [Domain; 54] = [
        Domain::Civil,
        Domain::Commercial,
        Domain::Public,
        Domain::Social,
        Domain::Fiscal,
        Domain::ProprieteIntellectuelle,
        Domain::Europeen,
        Domain::Criminel,
        Domain::Constitutionnel,
        Domain::CivilProceduresCivilesExecution,
        Domain::CivilDroitImmobilierConstruction,
        Domain::CivilDroitLocatif,
        Domain::CivilDroitPersonnesFamille,
        Domain::CivilDroitCoproprieteProprieteImmobiliere,
        Domain::CivilDroitAssurances,
        Domain::CivilDroitResponsabilite,
        Domain::CivilDroitBancaireBoursier,
        Domain::CivilDroitSuccessions,
        Domain::CivilDroitExpropriationPreemption,
        Domain::CivilDivorceSeparationCorps,
        Domain::CivilDroitRural,
        Domain::CivilDroitResponsabiliteContrats,
        Domain::CivilDroitSaisieImmobiliere,
        Domain::CivilDroitMineurs,
        Domain::CivilDroitInternationalPrive,
        Domain::CommercialDroitEntreprisesDifficulte,
        Domain::CommercialDroitBancaireBoursier,
        Domain::CommercialDroitContrats,
        Domain::CommercialDroitSocietes,
        Domain::CommercialDroitNumerique,
        Domain::CommercialDroitTransport,
        Domain::CommercialDroitAssurances,
        Domain::CommercialDroitConcurrence,
        Domain::CommercialDroitConsommation,
        Domain::CommercialDroitArbitrage,
        Domain::PublicDroitEtrangersNationalite,
        Domain::PublicDroitUrbanismeImmobilierPublic,
        Domain::PublicDroitTravail,
        Domain::PublicDroitPenalPublic,
        Domain::PublicDroitAideActionSociale,
        Domain::PublicDroitEnvironnement,
        Domain::PublicDroitLibertesFondamentales,
        Domain::PublicDroitCommandePublique,
        Domain::SocialDroitTravail,
        Domain::SocialDroitAideActionSociale,
        Domain::SocialDroitPenalSocial,
        Domain::FiscalDroitDouanier,
        Domain::ProprieteIntellectuelleIndustrielle,
        Domain::ProprieteIntellectuelleLitteraireArtistique,
        Domain::EuropeenLibertesCirculation,
        Domain::EuropeenAidesEtat,
        Domain::EuropeenPolitiqueAgricole,
        Domain::EuropeenActionExterieure,
        Domain::EuropeenDroitInstitutionnel,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubjectMatterTaxonomy {
    #[serde(rename = "NAC")]
    Nac,
    #[serde(rename = "CASSATION_THEMES")]
    CassationThemes,
    #[serde(rename = "LEBON_PUBLICATION")]
    LebonPublication,
    #[serde(rename = "AUTRE")]
    Autre,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecisionType {
    #[serde(rename = "ARRET")]
    Arret,
    #[serde(rename = "JUGEMENT")]
    Jugement,
    #[serde(rename = "ORDONNANCE")]
    Ordonnance,
    #[serde(rename = "AVIS")]
    Avis,
    #[serde(rename = "DECISION")]
    Decision,
    #[serde(rename = "AUTRE")]
    Autre,
}

/// Court type of a decision: `TA` `CAA` `CE` `TC` `CONSTIT` (administrative and
/// supreme), `CC` `CA` `TJ` `TCOM` `CPH` (judicial), `ASSISES` `CCD` (the cours d'assises and the
/// cours criminelles départementales), `CEDH` `CJUE` (European),
/// `CNDA` (asylum court), `CNIL` (CNIL deliberations), `DDD` (the Défenseur des droits and the
/// authorities it succeeded), `ADLC` (the Autorité de la concurrence and the Conseil before it),
/// `AMF` (the Autorité des marchés financiers' Commission des sanctions), `COMEU` (the European
/// Commission's competition decisions), then the families of the foreign courts, whose code
/// starts with their place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JurisdictionType {
    #[serde(rename = "TA")]
    Ta,
    #[serde(rename = "CAA")]
    Caa,
    #[serde(rename = "CE")]
    Ce,
    #[serde(rename = "CONSTIT")]
    Constit,
    #[serde(rename = "TC")]
    Tc,
    #[serde(rename = "CC")]
    Cc,
    #[serde(rename = "CA")]
    Ca,
    #[serde(rename = "TJ")]
    Tj,
    #[serde(rename = "TCOM")]
    Tcom,
    #[serde(rename = "CPH")]
    Cph,
    #[serde(rename = "ASSISES")]
    Assises,
    #[serde(rename = "CCD")]
    Ccd,
    #[serde(rename = "CEDH")]
    Cedh,
    #[serde(rename = "CJUE")]
    Cjue,
    #[serde(rename = "CNDA")]
    Cnda,
    #[serde(rename = "CNIL")]
    Cnil,
    #[serde(rename = "DDD")]
    Ddd,
    #[serde(rename = "ADLC")]
    Adlc,
    #[serde(rename = "AMF")]
    Amf,
    #[serde(rename = "COMEU")]
    Comeu,
    #[serde(rename = "COUR_SUPREME")]
    CourSupreme,
    #[serde(rename = "COUR_CASSATION")]
    CourCassation,
    #[serde(rename = "COUR_CONSTITUTIONNELLE")]
    CourConstitutionnelle,
    #[serde(rename = "COUR_APPEL")]
    CourAppel,
    #[serde(rename = "COUR_ADMINISTRATIVE")]
    CourAdministrative,
    #[serde(rename = "TRIBUNAL_ADMINISTRATIF")]
    TribunalAdministratif,
    #[serde(rename = "TRIBUNAL_PREMIERE_INSTANCE")]
    TribunalPremiereInstance,
    #[serde(rename = "TRIBUNAL_COMMERCE")]
    TribunalCommerce,
    #[serde(rename = "COUR_CRIMINELLE")]
    CourCriminelle,
    #[serde(rename = "COUR_JUSTICE")]
    CourJustice,
    #[serde(rename = "CCJA")]
    Ccja,
    #[serde(rename = "COUR_DROITS_HOMME")]
    CourDroitsHomme,
    #[serde(rename = "AUTORITE_ADMINISTRATIVE")]
    AutoriteAdministrative,
}

impl JurisdictionType {
    /// Toutes les variantes, dans l'ordre de déclaration.
    pub const ALL: [JurisdictionType; 33] = [
        JurisdictionType::Ta,
        JurisdictionType::Caa,
        JurisdictionType::Ce,
        JurisdictionType::Constit,
        JurisdictionType::Tc,
        JurisdictionType::Cc,
        JurisdictionType::Ca,
        JurisdictionType::Tj,
        JurisdictionType::Tcom,
        JurisdictionType::Cph,
        JurisdictionType::Assises,
        JurisdictionType::Ccd,
        JurisdictionType::Cedh,
        JurisdictionType::Cjue,
        JurisdictionType::Cnda,
        JurisdictionType::Cnil,
        JurisdictionType::Ddd,
        JurisdictionType::Adlc,
        JurisdictionType::Amf,
        JurisdictionType::Comeu,
        JurisdictionType::CourSupreme,
        JurisdictionType::CourCassation,
        JurisdictionType::CourConstitutionnelle,
        JurisdictionType::CourAppel,
        JurisdictionType::CourAdministrative,
        JurisdictionType::TribunalAdministratif,
        JurisdictionType::TribunalPremiereInstance,
        JurisdictionType::TribunalCommerce,
        JurisdictionType::CourCriminelle,
        JurisdictionType::CourJustice,
        JurisdictionType::Ccja,
        JurisdictionType::CourDroitsHomme,
        JurisdictionType::AutoriteAdministrative,
    ];

    /// The jurisdiction segment of a decision URL, in the alphabet of text URLs: a foreign
    /// court's place, read from its code.
    pub fn url_segment(self, jurisdiction_code: Option<&str>) -> String {
        match self {
            JurisdictionType::Cjue | JurisdictionType::Comeu => "eu".to_string(),
            JurisdictionType::Cedh => "intl".to_string(),
            JurisdictionType::Ta
            | JurisdictionType::Caa
            | JurisdictionType::Ce
            | JurisdictionType::Constit
            | JurisdictionType::Tc
            | JurisdictionType::Cc
            | JurisdictionType::Ca
            | JurisdictionType::Tj
            | JurisdictionType::Tcom
            | JurisdictionType::Cph
            | JurisdictionType::Assises
            | JurisdictionType::Ccd
            | JurisdictionType::Cnda
            | JurisdictionType::Cnil
            | JurisdictionType::Ddd
            | JurisdictionType::Adlc
            | JurisdictionType::Amf => "fr".to_string(),
            foreign => lj_vocab::url_segment(&lj_vocab::courts::foreign_order(
                jurisdiction_code
                    .unwrap_or_else(|| panic!("a foreign court without a code: {foreign:?}")),
            )),
        }
    }

    /// The variant a stored type code names (`CJUE`, `TA`…).
    pub fn from_code(code: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(code.to_string())).ok()
    }

    /// Whether the court dates its decision by its public reading, as the administrative order does.
    pub fn dated_by_reading(self) -> bool {
        matches!(
            self,
            JurisdictionType::Ta
                | JurisdictionType::Caa
                | JurisdictionType::Ce
                | JurisdictionType::Tc
                | JurisdictionType::Cnda
        )
    }
}

/// Abréviations de citation : tête de nom de juridiction (minuscules) →
/// abréviation doctrinale. Couvre tous les libellés du référentiel
/// `jurisdiction` (y compris les « Tribunal des activités économiques » issus
/// de la réforme 2025, rattachés au type TCOM). Les préfixes emboîtés sont
/// classés du plus long au plus court (« cour administrative d'appel » avant
/// « cour d'appel »).
const JURISDICTION_ABBREVIATIONS: [(&str, &str); 24] = [
    ("cour administrative d'appel", "CAA"),
    ("cour nationale du droit d'asile", "CNDA"),
    ("cour européenne des droits de l'homme", "CEDH"),
    ("cour de justice de l'union européenne", "CJUE"),
    ("cour de cassation", "Cass."),
    ("cour d'appel", "CA"),
    ("cour d'assises", "C. assises"),
    ("cour criminelle départementale", "CCD"),
    ("tribunal administratif", "TA"),
    ("tribunal des activités économiques", "TAE"),
    ("tribunal de commerce", "T. com."),
    ("tribunal des conflits", "T. confl."),
    ("tribunal judiciaire", "TJ"),
    ("conseil d'état", "CE"),
    ("conseil d'etat", "CE"),
    ("conseil constitutionnel", "Cons. const."),
    (
        "commission nationale de l'informatique et des libertés",
        "CNIL",
    ),
    ("défenseur des droits", "DDD"),
    (
        "haute autorité de lutte contre les discriminations et pour l'égalité",
        "HALDE",
    ),
    ("commission nationale de déontologie de la sécurité", "CNDS"),
    ("autorité de la concurrence", "Aut. conc."),
    ("conseil de la concurrence", "Cons. conc."),
    ("autorité des marchés financiers", "AMF"),
    ("commission européenne", "Comm. UE"),
];

/// Nom de juridiction abrégé pour une citation : la tête du nom est remplacée
/// par son abréviation doctrinale, le reste (ville) est conservé — « Tribunal
/// judiciaire de Nanterre » → « TJ de Nanterre », « Cour de cassation » →
/// « Cass. ». Nom hors table rendu tel quel. Consommée par la référence courte
/// (`lj-web` : citation copiée, nom de PDF, topbar) et le nom de fichier
/// d'export (`lj-api`).
pub fn abbreviate_jurisdiction_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for (head, abbr) in JURISDICTION_ABBREVIATIONS {
        // `to_lowercase` préserve la longueur en octets sur ces alphabets
        // (é/É : 2 octets) — le slice sur `name` reste aligné sur `head`.
        if lower.starts_with(head) {
            return format!("{abbr}{}", &name[head.len()..]);
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_foreign_court_type_is_a_variant_sitting_in_its_place() {
        for code in lj_vocab::courts::FOREIGN_COURT_TYPES {
            let kind = JurisdictionType::from_code(code).unwrap_or_else(|| panic!("{code}"));
            let court = lj_vocab::courts::foreign_court_code("sn", code, &[]);
            assert_eq!(kind.url_segment(Some(&court)), "sn", "{code}");
        }
    }

    #[test]
    fn abbreviate_all_corpus_jurisdictions() {
        // One case per name head of the `jurisdiction` referential.
        for (name, expected) in [
            ("Tribunal administratif de Caen", "TA de Caen"),
            ("Cour administrative d'appel de Bordeaux", "CAA de Bordeaux"),
            ("Conseil d'État", "CE"),
            ("Cour d'appel d'Aix-en-Provence", "CA d'Aix-en-Provence"),
            ("Cour de cassation", "Cass."),
            ("Tribunal judiciaire du Mans", "TJ du Mans"),
            ("Cour d'assises de Paris", "C. assises de Paris"),
            (
                "Cour criminelle départementale du Vaucluse",
                "CCD du Vaucluse",
            ),
            ("Tribunal de commerce d'Agen", "T. com. d'Agen"),
            (
                "Tribunal des activités économiques de Nanterre",
                "TAE de Nanterre",
            ),
            ("Tribunal des conflits", "T. confl."),
            ("Conseil constitutionnel", "Cons. const."),
            ("Cour nationale du droit d'asile", "CNDA"),
            ("Cour européenne des droits de l'homme", "CEDH"),
            ("Cour de justice de l'Union européenne", "CJUE"),
            (
                "Commission nationale de l'informatique et des libertés",
                "CNIL",
            ),
            ("Défenseur des droits", "DDD"),
            (
                "Haute autorité de lutte contre les discriminations et pour l'égalité",
                "HALDE",
            ),
            ("Commission nationale de déontologie de la sécurité", "CNDS"),
            ("Autorité de la concurrence", "Aut. conc."),
            ("Conseil de la concurrence", "Cons. conc."),
        ] {
            assert_eq!(abbreviate_jurisdiction_name(name), expected, "{name}");
        }
    }

    #[test]
    fn abbreviate_unknown_name_passthrough() {
        assert_eq!(
            abbreviate_jurisdiction_name("Juridiction nationale des activités foraines"),
            "Juridiction nationale des activités foraines"
        );
    }

    fn ser<T: Serialize>(v: &T) -> String {
        serde_json::to_value(v)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn jurisdiction_level_serialized_strings() {
        assert_eq!(ser(&JurisdictionLevel::ConseilDEtat), "CONSEIL_D_ETAT");
        assert_eq!(
            ser(&JurisdictionLevel::TribunalAdministratif),
            "TRIBUNAL_ADMINISTRATIF"
        );
        assert_eq!(
            ser(&JurisdictionLevel::CourDeCassation),
            "COUR_DE_CASSATION"
        );
        assert_eq!(ser(&JurisdictionLevel::Autre), "AUTRE");
    }

    #[test]
    fn facet_enums_serialized_strings() {
        assert_eq!(ser(&Solution::PartlyGranted), "PARTLY_GRANTED");
        assert_eq!(ser(&Domain::CivilDroitLocatif), "CIVIL_DROIT_LOCATIF");
    }

    #[test]
    fn solution_reads_a_root_or_a_leaf_as_its_root() {
        let de = |code: &str| serde_json::from_value::<Solution>(serde_json::json!(code));
        for root in Solution::ALL {
            assert_eq!(de(&ser(&root)).unwrap(), root);
        }
        assert_eq!(de("REJET").unwrap(), Solution::Dismissed);
        assert_eq!(
            de("SATISFACTION_PARTIELLE").unwrap(),
            Solution::PartlyGranted
        );
        let refused = de("ACQUITTEMENT").unwrap_err().to_string();
        assert!(refused.contains("expected one of `GRANTED`"), "{refused}");
    }

    /// A facet's uids over every migration in order, seeded by `('facet:X'`, removed by DELETE.
    fn seed_uid_suffixes(facet: &str) -> std::collections::BTreeSet<String> {
        let dir = format!("{}/../lj-store/migrations", env!("CARGO_MANIFEST_DIR"));
        let mut fichiers: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "sql"))
            .collect();
        fichiers.sort();
        let prefix = format!("('{facet}:");
        let mut uids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for fichier in fichiers {
            let sql = std::fs::read_to_string(&fichier).unwrap();
            for ligne in sql.lines().map(str::trim_start) {
                if let Some(rest) = ligne.strip_prefix(&prefix) {
                    if let Some(fin) = rest.find('\'') {
                        uids.insert(rest[..fin].to_string());
                    }
                }
            }
            for statement in sql.split(';') {
                if !statement.contains("DELETE FROM facet_value") {
                    continue;
                }
                for uid in cite_uids(statement, facet) {
                    uids.remove(&uid);
                }
            }
        }
        assert!(!uids.is_empty(), "aucun uid `{facet}:*` semé");
        uids
    }

    /// Suffixes des littéraux `'facet:X'` d'un fragment SQL.
    fn cite_uids(sql: &str, facet: &str) -> Vec<String> {
        let motif = format!("'{facet}:");
        sql.match_indices(&motif)
            .filter_map(|(i, _)| {
                let rest = &sql[i + motif.len()..];
                Some(rest[..rest.find('\'')?].to_string())
            })
            .collect()
    }

    fn all_codes<T: Serialize>(all: &[T]) -> std::collections::BTreeSet<String> {
        all.iter().map(ser).collect()
    }

    /// Les enums de facettes sont le miroir exact du seed DB : toute dérive enum ↔
    /// référentiel est un build rouge. `ALL` porte chaque variante (l'égalité de
    /// cardinal garde la complétude des deux côtés).
    #[test]
    fn enums_match_migration_seed() {
        let mut solutions = all_codes(&Solution::ALL);
        solutions.extend(Solution::LEAVES.iter().map(|(leaf, _)| leaf.to_string()));
        assert_eq!(solutions, seed_uid_suffixes("solution"));
        let mut domains = seed_uid_suffixes("domaine");
        domains.extend(seed_uid_suffixes("legal_domain"));
        assert_eq!(all_codes(&Domain::ALL), domains);
        assert_eq!(Solution::ALL.len(), 6);
        assert_eq!(Domain::ALL.len(), 54);
    }

    /// Every leaf sits under the root the seed gives it, `('solution:LEAF', 'solution:ROOT'`.
    #[test]
    fn solution_leaves_match_migration_seed() {
        let dir = format!("{}/../lj-store/migrations", env!("CARGO_MANIFEST_DIR"));
        let mut seeded: Vec<(String, String)> = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let sql = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for line in sql.lines() {
                if let [leaf, root] = &cite_uids(line, "solution")[..] {
                    seeded.push((leaf.clone(), root.clone()));
                }
            }
        }
        seeded.sort();
        let mut leaves: Vec<(String, String)> = Solution::LEAVES
            .iter()
            .map(|(leaf, root)| (leaf.to_string(), ser(root)))
            .collect();
        leaves.sort();
        assert_eq!(leaves, seeded);
    }

    #[test]
    fn jurisdiction_type_round_trip() {
        for v in [
            JurisdictionType::Ta,
            JurisdictionType::Caa,
            JurisdictionType::Ce,
            JurisdictionType::Constit,
            JurisdictionType::Tc,
            JurisdictionType::Cc,
            JurisdictionType::Ca,
            JurisdictionType::Tj,
            JurisdictionType::Tcom,
            JurisdictionType::Cedh,
            JurisdictionType::Cjue,
            JurisdictionType::Cnda,
        ] {
            let s = serde_json::to_string(&v).unwrap();
            let back: JurisdictionType = serde_json::from_str(&s).unwrap();
            assert_eq!(v, back);
        }
        assert_eq!(ser(&JurisdictionType::Tcom), "TCOM");
        assert_eq!(ser(&JurisdictionType::Constit), "CONSTIT");
        assert_eq!(ser(&JurisdictionType::Tc), "TC");
        assert_eq!(ser(&JurisdictionType::Cedh), "CEDH");
        assert_eq!(ser(&JurisdictionType::Cjue), "CJUE");
        assert_eq!(ser(&JurisdictionType::Cnda), "CNDA");
    }

    #[test]
    fn deserializes_from_python_strings() {
        let lvl: JurisdictionLevel =
            serde_json::from_str("\"COUR_NATIONALE_DROIT_ASILE\"").unwrap();
        assert_eq!(lvl, JurisdictionLevel::CourNationaleDroitAsile);
        let dt: DecisionType = serde_json::from_str("\"ORDONNANCE\"").unwrap();
        assert_eq!(dt, DecisionType::Ordonnance);
        let smt: SubjectMatterTaxonomy = serde_json::from_str("\"CASSATION_THEMES\"").unwrap();
        assert_eq!(smt, SubjectMatterTaxonomy::CassationThemes);
        let dom: Domain = serde_json::from_str("\"COMMERCIAL_DROIT_SOCIETES\"").unwrap();
        assert_eq!(dom, Domain::CommercialDroitSocietes);
    }
}
