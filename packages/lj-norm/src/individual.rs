//! An act that names its addressees, read off its title, an act the JO may print only out of
//! outside search engines' reach, and a JO text that is no norm. PURE.

use std::sync::LazyLock;

use regex::Regex;

/// The title formulas of an individual measure, matched case-insensitively.
/// Plain alternations, so the same string serves a Postgres `~*`.
pub const INDIVIDUAL_MEASURE: &str = "portant (nominations?|promotions?|titularisations?|naturalisation|mutations?|affectations?|détachements?|maintien|réintégration|mise à disposition|mise en disponibilité|mise à la retraite|cessation de fonctions|admission à la retraite|acceptation de la démission|radiation|reclassement|inscription (au tableau|sur la liste)|attribution de (la|une) médaille|(modification de l.)?agrément|habilitation)|^avis de vacance|nommant |conférant (un|le) grade|maintenant dans (un|le) grade|accordant la nationalité française|conférant l.honorariat|acceptant la démission|tableau d.avancement|listes? d.aptitude|échelle de solde|(^|[^a-z])(m\\.|mm\\.|mme|mmes|mlle|mlles) [^;]*(est|sont) (nomm|d[ée]sign|charg|admis|maintenu|plac|r[ée]int[ée]gr|mis |mise|[ée]lev|promu|titularis|ray[ée]|d[ée]tach|affect|mut[ée]|radi[ée]|inscrit|confirm|d[ée]l[ée]gu|renouvel)|[0-9]{4},? nomination (de m\\.|de mme|de mlle|des membres|de membres)";

/// The title formulas of the acts CRPA R. 221-15 and R. 221-16 keep from outside search
/// engines: name changes, nationality, francisation, exclusion from an order, disciplinary
/// sanctions, and DILA's own marker for them. Plain alternations, as [`INDIVIDUAL_MEASURE`].
pub const PROTECTED_ACT: &str = "\\(accès protégé|^demandes? de changement de nom|portant changement de noms?\\s*(\\(|$)|portant (naturalisation|réintégration dans la nationalité|acquisition de la nationalité|libération (des liens )?de l.allégeance|francisation|attribution de prénom|perte de la nationalité|déchéance de la nationalité|réintégration dans (la qualité de|l.ordre|les droits)|sanctions? disciplinaires?|exclusion temporaire de fonctions|mise à la retraite d.office)|relative à une sanction disciplinaire|rapportant (un|le|des) décrets? (de|portant) (naturalisation|réintégration|changement de nom|francisation)|(exclusion|suspension)( temporaire)? (de|des membres de) (l.ordre|la légion d.honneur|la médaille militaire)|portant (révocation|rétrogradation|abaissement d.échelon|blâme|avertissement) (de m|de mme|d.un|d.une)";

/// Words one of which every title [`PROTECTED_ACT`] matches holds, its marker aside: a full-text
/// search for any of them narrows the titles the formulas have to read.
pub const PROTECTED_WORDS: [&str; 17] = [
    "changement",
    "naturalisation",
    "nationalité",
    "allégeance",
    "francisation",
    "prénom",
    "réintégration",
    "disciplinaire",
    "disciplinaires",
    "exclusion",
    "suspension",
    "office",
    "révocation",
    "rétrogradation",
    "échelon",
    "blâme",
    "avertissement",
];

/// The natures of a JO text that is no norm: a notice of its issue.
pub const NOTICE_NATURES: [&str; 14] = [
    "AVIS",
    "AVISEURO",
    "INFORMATIONS_PARLEMENTAIRES",
    "INFORMATIONS_DIVERSES",
    "RESULTATS",
    "LISTE",
    "TABLEAU",
    "RAPPORT",
    "SAISINE",
    "OBSERVATION",
    "ANNONCES",
    "COMMUNIQUE",
    "DECLARATION",
    "RECTIFICATIF",
];

/// Where a text that is no norm goes: its notice, or the decision of the corpus it prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaves {
    Journal,
    Decision,
}

/// The French natures whose texts may be no norm, each read by its title.
const FRENCH_NO_NORM: [&str; 31] = [
    "ANNONCES",
    "ARRET",
    "AVIS",
    "AVISEURO",
    "CITATION",
    "COMMUNIQUE",
    "CONVENTION",
    "DECISION",
    "DECISION_CC",
    "DECISION_EURO",
    "DECLARATION",
    "DECLARATIONEURO",
    "DELIBERATION",
    "DELIBERATIONEURO",
    "EXEQUATUR",
    "INFORMATION",
    "INFORMATIONS_CESE",
    "INFORMATIONS_DIVERSES",
    "INFORMATIONS_PARLEMENTAIRES",
    "LETTRE",
    "LISTE",
    "MEMOIRE",
    "OBSERVATION",
    "RAPPORT",
    "RECOMMANDATION",
    "REGLEMENT",
    "REMISE",
    "RESULTATS",
    "SAISINE",
    "TABLEAU",
    "VOCABULAIRE",
];

/// The natures that may be no norm in another order, the Union and the treaties aside.
const FOREIGN_NO_NORM: [&str; 6] = [
    "AVIS",
    "DECISION",
    "DELIBERATION",
    "RECTIFICATIF",
    "RESOLUTION",
    "TEXTE",
];

/// The natures of the texts that may leave the norms, every order mixed.
pub fn no_norm_natures() -> Vec<&'static str> {
    let mut natures: Vec<&str> = FRENCH_NO_NORM
        .iter()
        .chain(&FOREIGN_NO_NORM)
        .copied()
        .collect();
    natures.sort_unstable();
    natures.dedup();
    natures
}

fn title_pattern(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("valid regex")
}

static COURT: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(
        r"^décision n°\s*\d{2,4}-\d+(/\d+)*\s+(dc|qpc|an|sen|l|fnr|lp|lom|pdr|ref|elec|d|i|orga|ar16|rip|lib)([^/a-z0-9]|$)|conseil d.[ée]tat statuant au contentieux|^arr[êe]t du conseil d.[ée]tat|de la cjce|cour de justice des communaut|de la cour de cassation",
    )
});
static RULE: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(
        r"fixant (les|la|le|l.) |r[eè]glement(ant|ation)|homologu|portant approbation du r[eè]glement|portant modification du r[eè]glement|r[eè]glement int[ée]rieur|portant extension|portant cr[ée]ation|portant organisation|relati(f|ve)s? (à|aux) (la |l.)?(r[eè]gles|conditions|modalit|proc[ée]dure|crit[eè]res|tarifs?|cahier)|cahier des charges|pris pour l.application|portant adoption (d.une|d.un|de la|du) (recommandation|norme|r[eè]glement|r[ée]f[ée]rentiel|m[ée]thodologie|cadre)|d[ée]finissant|pr[ée]cisant les|portant (fixation|d[ée]finition)|nomenclature|r[ée]pertoire des groupes g[ée]n[ée]riques|liste des (actes|produits|prestations)",
    )
});
static GAME: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(
        r"jeux?|loto|euro ?millions|op[ée]ration promotionnelle|tirage|grattage|pari|additif",
    )
});
static LABOUR_AGREEMENT: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(r"assurance ch[ôo]mage|accord national interprofessionnel|convention collective")
});
static BROADCAST_REGULATOR: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(
        r"conseil sup[ée]rieur de l.audiovisuel|aux services de (radio|t[ée]l[ée]vision)|arcom",
    )
});
static DELEGATION: LazyLock<Regex> = LazyLock::new(|| {
    title_pattern(
        r"portant (d[ée]l[ée]gation|subd[ée]l[ée]gation)s? (de signature|de pouvoirs?|d.attributions?)|d[ée]l[ée]gations? de signature",
    )
});
static DECREE: LazyLock<Regex> = LazyLock::new(|| title_pattern(r"^d[ée]cret"));
static REPORT: LazyLock<Regex> = LazyLock::new(|| title_pattern(r"^(additif|rapport)"));
static CONSTITUTIONAL_LAW: LazyLock<Regex> =
    LazyLock::new(|| title_pattern(r"loi constitutionnelle"));

/// Where a text goes when it is no norm: a decision or deliberation whose title reads as neither
/// rule nor delegation, a notice, a court's ruling, a game's rules. `None`: it stays.
pub fn leaves_norms(jurisdiction: &str, nature: &str, title: &str) -> Option<Leaves> {
    let nature = nature.to_uppercase();
    let nature = nature.as_str();
    let read = match jurisdiction {
        "FR" => FRENCH_NO_NORM.contains(&nature),
        "EU" | "INTL" => false,
        _ => FOREIGN_NO_NORM.contains(&nature),
    };
    if !read {
        return None;
    }
    let journal_unless = |kept: bool| (!kept).then_some(Leaves::Journal);
    match nature {
        "DECISION_CC" => Some(Leaves::Decision),
        "DECISION" | "ARRET" if COURT.is_match(title) => Some(Leaves::Decision),
        "ARRET" if DECREE.is_match(title) => None,
        "ARRET" => Some(Leaves::Decision),
        "TEXTE" => journal_unless(
            CONSTITUTIONAL_LAW.is_match(title)
                || !(is_individual_measure(title) || REPORT.is_match(title)),
        ),
        "REGLEMENT" => journal_unless(!GAME.is_match(title)),
        "CONVENTION" => journal_unless(LABOUR_AGREEMENT.is_match(title)),
        "RECOMMANDATION" => journal_unless(BROADCAST_REGULATOR.is_match(title)),
        "DECISION" | "DECISION_EURO" | "DELIBERATION" | "DELIBERATIONEURO" => journal_unless(
            DELEGATION.is_match(title) || (!is_individual_measure(title) && RULE.is_match(title)),
        ),
        _ => Some(Leaves::Journal),
    }
}

static CONSTITUTIONAL_DOCKET: LazyLock<Regex> =
    LazyLock::new(|| title_pattern(r"^décision n°\s*(\d{2,4}-\d+)"));
static COUNCIL_OF_STATE: LazyLock<Regex> = LazyLock::new(|| title_pattern(r"conseil d.[ée]tat"));
static COUNCIL_DOCKET: LazyLock<Regex> = LazyLock::new(|| title_pattern(r"n(?:°|o)s?\s*(\d{5,6})"));
static EUROPEAN_CASE: LazyLock<Regex> = LazyLock::new(|| title_pattern(r"affaire (\d+)-(\d+)"));

/// The court code and the docket a printed ruling's title names, as the decisions carry them.
pub fn printed_ruling(title: &str) -> Option<(&'static str, String)> {
    if let Some(c) = CONSTITUTIONAL_DOCKET.captures(title) {
        return Some(("constit", c[1].to_string()));
    }
    if COUNCIL_OF_STATE.is_match(title) {
        return COUNCIL_DOCKET
            .captures(title)
            .map(|c| ("ce", c[1].to_string()));
    }
    EUROPEAN_CASE
        .captures(title)
        .map(|c| ("cjue", format!("C-{}/{}", &c[1], &c[2])))
}

pub fn is_individual_measure(title: &str) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!("(?i){INDIVIDUAL_MEASURE}")).expect("valid regex"));
    RE.is_match(title)
}

pub fn is_protected_act(title: &str) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!("(?i){PROTECTED_ACT}")).expect("valid regex"));
    RE.is_match(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTECTED: [&str; 12] = [
        "Décret du 4 mai 2022 portant naturalisation, réintégration, mention d'enfants mineurs bénéficiant de l'effet collectif attaché à l'acquisition de la nationalité française par leurs parents, francisation de noms et de prénoms et libération de l'allégeance française (Accès protégé)",
        "Demandes de changement de nom (Accès protégé textes 70 à 82)",
        "Demandes de changement de nom",
        "Décret du 11 juin 1990    portant déchéance de la nationalité française",
        "Décret du 7 juillet 2021 rapportant un décret de naturalisation (rectificatif)",
        "Décret du 3 août 1999 portant réintégration dans la qualité de chevalier de l'ordre national de la Légion d'honneur",
        "Décret du 23 juin 2015 portant exclusion temporaire de fonctions d'un préfet - M. BURG (Marc)",
        "Décret du 26 décembre 2022 portant mise à la retraite d'office d'un sous-préfet - M. DEVEMY (Jean-François)",
        "Décret du 7 juillet 2014 portant sanction disciplinaire (tribunaux administratifs et cours administratives d'appel)",
        "Décision du 11 septembre 2000  relative à une sanction disciplinaire",
        "Décret du 28 mai 2009 portant révocation de M. Gérard DALONGEVILLE, maire de la commune de Hénin-Beaumont (Pas-de-Calais)",
        "Décret du 13 janvier 2011  portant révocation d'un maire",
    ];

    #[test]
    fn the_acts_crpa_r221_15_and_r221_16_keep_from_search_engines_are_protected() {
        for title in PROTECTED {
            assert!(is_protected_act(title), "{title}");
        }
        for title in [
            "Décret du 30 janvier 1987 PORTANT CHANGEMENT DE NOM DE COMMUNES",
            "Décret n° 2011-1774 du 5 décembre 2011 portant changement de nom d'un établissement d'enseignement",
            "Décret n° 2005-794 du 15 juillet 2005 relatif aux sanctions disciplinaires et à la suspension de fonctions applicables aux militaires",
            "Décision n° 94-576 du 8 novembre 1994 portant suspension d'autorisation d'exploiter un service de radiodiffusion sonore",
            "Arrêté du 16 mars 2023 portant suspension des procédures d'adoption internationale concernant les enfants résidant en Haïti",
            "Décret du 4 mai 2012 portant concession de la médaille militaire",
            "Décret du 13 novembre 2026 portant nomination de la sous-préfète de Calais (groupe II) - Mme INFANTE (Nathalie)",
            "Arrêté du 31 décembre 2008 fixant le modèle du formulaire « Déclaration de départ de salariés, soit à 55 ans au moins dans le cadre d'un licenciement, soit dans le cadre d'une mise à la retraite d'office »",
        ] {
            assert!(!is_protected_act(title), "{title}");
        }
    }

    #[test]
    fn every_protected_title_holds_one_of_the_words_its_search_narrows_by() {
        for title in PROTECTED {
            let lower = title.to_lowercase();
            let mut words = lower.split(|c: char| !c.is_alphanumeric() && c != '-');
            assert!(words.any(|w| PROTECTED_WORDS.contains(&w)), "{title}");
        }
    }

    #[test]
    fn a_text_that_is_no_norm_leaves_for_its_notice_or_the_ruling_it_prints() {
        use Leaves::{Decision, Journal};
        for (jurisdiction, nature, title, leaves) in [
            ("FR", "DECISION", "Décision n° 2025-882 DC du 15 mai 2025", Some(Decision)),
            ("FR", "DECISION_CC", "Décision n° 86-208 DC du 2 juillet 1986", Some(Decision)),
            ("FR", "DECISION", "Décision n° 407289 du 26 mars 2018 du Conseil d'Etat statuant au contentieux", Some(Decision)),
            ("FR", "ARRET", "Arrêt du 4 décembre 1986 DE LA CJCE (AFFAIRE 220-83): COMMISSION CONTRE REPUBLIQUE FRANCAISE", Some(Decision)),
            ("FR", "ARRET", "Décret du 12 mars 1965 portant arrêt de comptes", None),
            ("FR", "DECISION", "Décision n° 2019-014 du 15 janvier 2019 portant autorisation d'un service de radio", Some(Journal)),
            ("FR", "DECISION", "Décision du 3 mai 2021 portant nomination au conseil d'administration", Some(Journal)),
            ("FR", "DELIBERATION", "Délibération n° 2019-160 du 21 novembre 2019 portant adoption d'un référentiel relatif aux traitements", None),
            ("FR", "DECISION", "Décision n° 2020-1 du 4 juin 2020 fixant les tarifs d'utilisation des réseaux", None),
            ("FR", "DECISION", "Décision du 5 mars 2020 portant délégation de signature", None),
            ("FR", "REGLEMENT", "Règlement du jeu de loterie EuroMillions", Some(Journal)),
            ("FR", "REGLEMENT", "Règlement n° 2014-03 relatif au plan comptable général", None),
            ("FR", "CONVENTION", "Convention du 14 avril 2017 relative à l'assurance chômage", None),
            ("FR", "CONVENTION", "Convention du 2 août 2010 entre l'Etat et l'ANR relative au programme d'investissements d'avenir", Some(Journal)),
            ("FR", "RECOMMANDATION", "Recommandation du 7 novembre 2006 du Conseil supérieur de l'audiovisuel aux services de télévision", None),
            ("FR", "RECOMMANDATION", "Recommandation du Contrôleur général des lieux de privation de liberté", Some(Journal)),
            ("FR", "AVIS", "Avis n° 2020-1 relatif au projet de décret", Some(Journal)),
            ("FR", "EXEQUATUR", "Exequatur", Some(Journal)),
            ("FR", "DECRET", "Décret du 3 mai 2021 portant nomination", None),
            ("DJ", "DECISION", "Décision n° 270 portant nomination d'un chef de service", Some(Journal)),
            ("DJ", "TEXTE", "Loi constitutionnelle portant révision de la Constitution", None),
            ("DJ", "TEXTE", "Rapport sur l'exécution du budget", Some(Journal)),
            ("DJ", "RECTIFICATIF", "Rectificatif au décret n° 2019-12", Some(Journal)),
            ("INTL", "RESOLUTION", "Résolution MSC.1 portant amendement", None),
            ("EU", "DECISION", "Décision du Conseil du 3 mai 2021", None),
        ] {
            assert_eq!(leaves_norms(jurisdiction, nature, title), leaves, "{title}");
        }
    }

    #[test]
    fn a_printed_ruling_names_the_docket_its_court_files_it_under() {
        for (title, ruling) in [
            (
                "Décision n° 2017-663 QPC du 19 octobre 2017",
                Some(("constit", "2017-663")),
            ),
            (
                "Décision n° 59-3 FNR du 27 novembre 1959",
                Some(("constit", "59-3")),
            ),
            (
                "Décision nos 383313 du 9 novembre 2015 du Conseil d'Etat statuant au contentieux",
                Some(("ce", "383313")),
            ),
            (
                "Arrêt du 4 décembre 1986 DE LA CJCE (AFFAIRE 220-83): COMMISSION",
                Some(("cjue", "C-220/83")),
            ),
            (
                "Arrêt du 17 septembre 2025 de la Cour de cassation (chambre criminelle)",
                None,
            ),
        ] {
            assert_eq!(
                printed_ruling(title),
                ruling.map(|(court, docket)| (court, docket.to_string())),
                "{title}"
            );
        }
    }

    #[test]
    fn a_measure_naming_its_addressees_is_individual_and_a_rule_is_not() {
        for title in [
            "Décision n° 9402 du 17 octobre 2018 portant promotion",
            "Arrêté du 2 mai 2024 portant inscription au tableau d'avancement au grade d'attaché principal",
            "Arrêté du 3 juin 2021 fixant la liste d'aptitude aux fonctions de commissaire",
            "Avis de vacance d'un emploi de sous-directeur",
            "Décision du 12 janvier 2022 nommant les membres du jury",
            "Arrêté du 26 février 2019 conférant un grade d'officier de réserve",
            "Arrêté du 21 mars 2019 portant agrément d'un organisme de formation",
            "Décret du 27 avril 1978 M. BLANC MARCEL EST NOMME PREFET DE LA REGION CENTRE",
            "Décret du 4 décembre 1975 M. SEGALAT (ANDRE) EST ADMIS A FAIRE VALOIR SES DROITS A LA RETRAITE",
            "Arrêté du 1 août 1979 MM. SCHOETTL ET DELARUE SONT NOMMES RAPPORTEURS",
            "Arrêté du 7 juin 1949 NOMINATION DE MEMBRES",
        ] {
            assert!(is_individual_measure(title), "{title}");
        }
        for title in [
            "Arrêté du 5 mars 2020 portant délégation de signature",
            "Arrêté du 9 juillet 2019 portant organisation de la direction des libertés publiques",
            "Arrêté du 25 février 2019 portant création de la trésorerie militaire",
            "Arrêté du 4 avril 2022 fixant les conditions d'agrément des organismes de formation",
            "Instruction n° 9469 GEND/DPMGN/SDGP du 5 février 2019 relative à la prime",
            "Arrêté du 3 juin 2021 fixant les modalités de nomination des membres du conseil",
            "Décret du 31 décembre 1969 PORTANT VIREMENT DE CREDITS (PM CHAP. 34-02 A 34-92)",
        ] {
            assert!(!is_individual_measure(title), "{title}");
        }
    }
}
