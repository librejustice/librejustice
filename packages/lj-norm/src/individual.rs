//! An act that names its addressees, read off its title. PURE.

use std::sync::LazyLock;

use regex::Regex;

/// The title formulas of an individual measure, matched case-insensitively.
/// Plain alternations, so the same string serves a Postgres `~*`.
pub const INDIVIDUAL_MEASURE: &str = "portant (nominations?|promotions?|titularisations?|naturalisation|mutations?|affectations?|détachements?|maintien|réintégration|mise à disposition|mise en disponibilité|mise à la retraite|cessation de fonctions|admission à la retraite|acceptation de la démission|radiation|reclassement|inscription (au tableau|sur la liste)|attribution de (la|une) médaille|(modification de l.)?agrément|habilitation)|^avis de vacance|nommant |conférant (un|le) grade|maintenant dans (un|le) grade|accordant la nationalité française|conférant l.honorariat|acceptant la démission|tableau d.avancement|listes? d.aptitude|échelle de solde";

pub fn is_individual_measure(title: &str) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!("(?i){INDIVIDUAL_MEASURE}")).expect("valid regex"));
    RE.is_match(title)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ] {
            assert!(is_individual_measure(title), "{title}");
        }
        for title in [
            "Arrêté du 5 mars 2020 portant délégation de signature",
            "Arrêté du 9 juillet 2019 portant organisation de la direction des libertés publiques",
            "Arrêté du 25 février 2019 portant création de la trésorerie militaire",
            "Arrêté du 4 avril 2022 fixant les conditions d'agrément des organismes de formation",
            "Instruction n° 9469 GEND/DPMGN/SDGP du 5 février 2019 relative à la prime",
        ] {
            assert!(!is_individual_measure(title), "{title}");
        }
    }
}
