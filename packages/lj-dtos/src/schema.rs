//! Enums canoniques de la taxonomie décisions, partagés api ↔ web.
//!
//! Schéma unifié : ordre administratif (CE/CAA/TA/CNDA/Cour des comptes/TC) ET
//! ordre judiciaire (Cour de cassation/CA/TJ/TCOM). Les enums de facettes
//! ([`Solution`], [`Procedure`], [`Office`], [`Domain`]) sont le **miroir compilé
//! du seed** de la migration `0100_facet_referentiels.sql` (ADR 0146 §4) :
//! chaque valeur sérialisée est le suffixe d'un uid `facet_value` — le test
//! `enums_match_migration_seed` diffe enum ↔ seed, toute dérive est un build
//! rouge. Les autres enums gardent les chaînes SCREAMING_CASE historiques des
//! colonnes Postgres `text`.

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

/// Solution — miroir compilé du seed `solution:*` (migration 0100, ADR 0146) :
/// les 15 valeurs de référence + Satisfaction totale/partielle. Chaque
/// valeur sérialisée est le suffixe d'un uid `facet_value` (`"REJET"` ↔
/// `solution:REJET`). Le test `enums_match_migration_seed` diffe enum ↔ seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Solution {
    Rejet,
    Irrecevabilite,
    Desistement,
    NonLieuAStatuer,
    Confirmation,
    Infirmation,
    InfirmationPartielle,
    Reformation,
    Cassation,
    CassationPartielle,
    Annulation,
    Conformite,
    NonConformite,
    Ineligibilite,
    SatisfactionTotale,
    SatisfactionPartielle,
    Autre,
}

impl Solution {
    /// Toutes les variantes, dans l'ordre `sort` du seed 0100.
    pub const ALL: [Solution; 17] = [
        Solution::Rejet,
        Solution::Irrecevabilite,
        Solution::Desistement,
        Solution::NonLieuAStatuer,
        Solution::Confirmation,
        Solution::Infirmation,
        Solution::InfirmationPartielle,
        Solution::Reformation,
        Solution::Cassation,
        Solution::CassationPartielle,
        Solution::Annulation,
        Solution::Conformite,
        Solution::NonConformite,
        Solution::Ineligibilite,
        Solution::SatisfactionTotale,
        Solution::SatisfactionPartielle,
        Solution::Autre,
    ];
}

/// Voie procédurale — miroir du seed `voie:*` (migration 0100), namespace
/// renommé `procedure:*` par la 0145 (ADR 0213). `null` = procédure
/// contentieuse ordinaire ; vocabulaire fermé, pas de `AUTRE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Procedure {
    RefereSuspension,
    RefereLiberte,
    RefereMesuresUtiles,
    ReferePrecontractuel,
    RefereProvision,
    RefereCivil,
    #[serde(rename = "FILTRAGE_R222_1")]
    FiltrageR2221,
    Papc,
    Qpc,
    QuestionPrejudicielleCjue,
    RecoursRevision,
    TierceOpposition,
    RectificationInterpretation,
}

impl Procedure {
    /// Toutes les variantes, dans l'ordre `sort` du seed 0100.
    pub const ALL: [Procedure; 13] = [
        Procedure::RefereSuspension,
        Procedure::RefereLiberte,
        Procedure::RefereMesuresUtiles,
        Procedure::ReferePrecontractuel,
        Procedure::RefereProvision,
        Procedure::RefereCivil,
        Procedure::FiltrageR2221,
        Procedure::Papc,
        Procedure::Qpc,
        Procedure::QuestionPrejudicielleCjue,
        Procedure::RecoursRevision,
        Procedure::TierceOpposition,
        Procedure::RectificationInterpretation,
    ];
}

/// Juge/office spécialisé — miroir du seed `office:*` (migration 0100).
/// `null` = formation ordinaire ; vocabulaire fermé, pas de `AUTRE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Office {
    Jld,
    Jaf,
    Jcp,
    Jex,
    JugeEnfants,
    PremierPresident,
    MagistratDesigne,
}

impl Office {
    /// Toutes les variantes, dans l'ordre `sort` du seed 0100.
    pub const ALL: [Office; 7] = [
        Office::Jld,
        Office::Jaf,
        Office::Jcp,
        Office::Jex,
        Office::JugeEnfants,
        Office::PremierPresident,
        Office::MagistratDesigne,
    ];
}

/// Portée jurisprudentielle — miroir du seed `portee:*` (migration 0114,
/// ADR 0167), namespace renommé `significance:*` par la 0145 (ADR 0213) :
/// groupes de `publication_codes` au rang le plus fort
/// (`significance_codes` de lj-core). Mapping total : `INDETERMINEE` sans code
/// classant (gabarit  : majeure · importante · limitée · indéterminée).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Significance {
    Majeure,
    Importante,
    Limitee,
    Indeterminee,
}

impl Significance {
    /// Toutes les variantes, dans l'ordre `sort` du seed 0114.
    pub const ALL: [Significance; 4] = [
        Significance::Majeure,
        Significance::Importante,
        Significance::Limitee,
        Significance::Indeterminee,
    ];
}

/// Domaine juridique — arbre de référence verbatim, miroir du seed `domaine:*`
/// (migration 0100), namespace renommé `legal_domain:*` par la 0145
/// (ADR 0213) : 9 racines + 36 feuilles. La racine d'une feuille est portée par
/// `parent_uid` en base ; côté requête une racine sélectionnée matche
/// elle-même + toutes ses feuilles (expansion côté API).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Domain {
    // Racines.
    Civil,
    Commercial,
    Public,
    Social,
    Fiscal,
    ProprieteIntellectuelle,
    Europeen,
    Criminel,
    Constitutionnel,
    // Feuilles Civil.
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
    // Feuilles Commercial.
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
    // Feuilles Public.
    PublicDroitEtrangersNationalite,
    PublicDroitUrbanismeImmobilierPublic,
    PublicDroitTravail,
    PublicDroitPenalPublic,
    PublicDroitAideActionSociale,
    PublicDroitEnvironnement,
    // Feuilles Social.
    SocialDroitTravail,
    SocialDroitAideActionSociale,
    SocialDroitPenalSocial,
    // Feuilles Propriété intellectuelle.
    ProprieteIntellectuelleIndustrielle,
    ProprieteIntellectuelleLitteraireArtistique,
}

impl Domain {
    /// Toutes les variantes (racines puis feuilles, ordre du seed 0100).
    pub const ALL: [Domain; 45] = [
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
        Domain::SocialDroitTravail,
        Domain::SocialDroitAideActionSociale,
        Domain::SocialDroitPenalSocial,
        Domain::ProprieteIntellectuelleIndustrielle,
        Domain::ProprieteIntellectuelleLitteraireArtistique,
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

/// Ordre de juridiction déduit (préfixe uid opendata / type Judilibre).
/// Valeurs : `TA` `CAA` `CE` `TC` `CONSTIT` (admin/suprême) ; `CC` `CA` `TJ`
/// `TCOM` (judiciaire) ; `CEDH` `CJUE` (européen, ADR 0094) ; `CNDA` (Cour
/// nationale du droit d'asile, source scrapée, ADR 0096) ; `CNIL` (délibérations
/// de la CNIL, fond DILA, ADR 0185).
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
    #[serde(rename = "CEDH")]
    Cedh,
    #[serde(rename = "CJUE")]
    Cjue,
    #[serde(rename = "CNDA")]
    Cnda,
    #[serde(rename = "CNIL")]
    Cnil,
}

impl JurisdictionType {
    /// Toutes les variantes, dans l'ordre de déclaration.
    pub const ALL: [JurisdictionType; 13] = [
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
        JurisdictionType::Cnil,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ser<T: Serialize>(v: &T) -> String {
        serde_json::to_value(v)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn jurisdiction_level_serialized_strings() {
        // Doit matcher les StrEnum Python (colonnes Postgres text + DTOs).
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
        assert_eq!(ser(&Solution::NonLieuAStatuer), "NON_LIEU_A_STATUER");
        assert_eq!(
            ser(&Solution::SatisfactionPartielle),
            "SATISFACTION_PARTIELLE"
        );
        assert_eq!(ser(&Procedure::FiltrageR2221), "FILTRAGE_R222_1");
        assert_eq!(
            ser(&Procedure::ReferePrecontractuel),
            "REFERE_PRECONTRACTUEL"
        );
        assert_eq!(ser(&Office::Jex), "JEX");
        assert_eq!(ser(&Domain::CivilDroitLocatif), "CIVIL_DROIT_LOCATIF");
    }

    /// Uids d'une facette dans un seed de migration (lignes `('facet:… — les seeds gardent les préfixes historiques, la 0145 renomme`).
    fn seed_uid_suffixes_in(migration: &str, facet: &str) -> std::collections::BTreeSet<String> {
        let sql = std::fs::read_to_string(format!(
            "{}/../lj-store/migrations/{migration}",
            env!("CARGO_MANIFEST_DIR"),
        ))
        .unwrap();
        let prefix = format!("('{facet}:");
        let uids: std::collections::BTreeSet<String> = sql
            .lines()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix(&prefix)?;
                Some(rest[..rest.find('\'')?].to_string())
            })
            .collect();
        assert!(!uids.is_empty(), "aucun uid `{facet}:*` dans {migration}");
        uids
    }

    fn seed_uid_suffixes(facet: &str) -> std::collections::BTreeSet<String> {
        seed_uid_suffixes_in("0100_facet_referentiels.sql", facet)
    }

    fn all_codes<T: Serialize>(all: &[T]) -> std::collections::BTreeSet<String> {
        all.iter().map(ser).collect()
    }

    /// Les enums de facettes sont le miroir exact du seed DB (ADR 0146 §4) :
    /// toute dérive enum ↔ référentiel est un build rouge. `ALL` porte chaque
    /// variante (l'égalité de cardinal garde la complétude des deux côtés).
    #[test]
    fn enums_match_migration_seed() {
        assert_eq!(all_codes(&Solution::ALL), seed_uid_suffixes("solution"));
        assert_eq!(all_codes(&Procedure::ALL), seed_uid_suffixes("voie"));
        assert_eq!(all_codes(&Office::ALL), seed_uid_suffixes("office"));
        assert_eq!(all_codes(&Domain::ALL), seed_uid_suffixes("domaine"));
        assert_eq!(
            all_codes(&Significance::ALL),
            seed_uid_suffixes_in("0114_portee_facette.sql", "portee")
        );
        assert_eq!(Solution::ALL.len(), 17);
        assert_eq!(Procedure::ALL.len(), 13);
        assert_eq!(Office::ALL.len(), 7);
        assert_eq!(Domain::ALL.len(), 45);
        assert_eq!(Significance::ALL.len(), 4);
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
