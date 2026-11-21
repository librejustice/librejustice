//! What the Belgian lanes share: an act's title as cited, its nature, and the governments whose
//! acts Justel lists.

use chrono::{Datelike, NaiveDate};

const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// A government whose acts Justel lists: the prefix of their uids, its issuers as Justel's searches
/// name them, and the act types naming it its headings open on, the longest first.
pub(super) struct Government {
    pub(super) prefix: &'static str,
    pub(super) issuers: &'static [&'static str],
    pub(super) heads: &'static [&'static str],
}

/// The act types a heading opens on without naming its government, the longest first.
const ACT_TYPES: &[&str] = &[
    "Arrêté du Gouvernement",
    "Arrêté de l'Exécutif",
    "Arrêté des Membres du Collège réuni",
    "Arrêté de Membres du Collège réuni",
    "Arrêté du Collège réuni",
    "Arrêté du Collège",
    "Arrêté du Vice-Ministre-Président",
    "Arrêté du Ministre-Président",
    "Arrêté du Ministre",
    "Arrêté ministériel",
    "Arrêté royal",
    "Arrêté réglementaire",
    "Arrêté",
    "Décret spécial",
    "Décret-programme",
    "Décret",
    "Ordonnance conjointe",
    "Ordonnance",
    "Accord de coopération",
    "Règlement",
];

impl Government {
    /// An act's title as cited, from the heading Justel lists it under.
    pub(super) fn title(&self, heading: &str, adopted: Option<NaiveDate>) -> String {
        dated_title_among(
            self.heads.iter().chain(ACT_TYPES).copied(),
            heading,
            adopted,
        )
    }
}

/// The federal State: every issuer Justel's searches name but the regions', the communities' and
/// the Constitutional Court's, whose acts are its judgments.
pub(super) const FEDERAL: Government = Government {
    prefix: "BE",
    issuers: &[
        "AFFAIRES ECONOMIQUES",
        "AFFAIRES ETRANGERES",
        "AFFAIRES ETRANGERES, COMMERCE EXTERIEUR ET COOPERATION AU DEVELOPPEMENT",
        "AFFAIRES SOCIALES",
        "AGENCE FEDERALE DE CONTROLE NUCLEAIRE",
        "AGENCE FEDERALE DES MEDICAMENTS ET DES PRODUITS DE SANTE",
        "AGENCE FEDERALE POUR LA SECURITE DE LA CHAINE ALIMENTAIRE",
        "AGENCE POUR LE COMMERCE EXTERIEUR",
        "AGRICULTURE",
        "BANQUE NATIONALE DE BELGIQUE",
        "BUDGET",
        "BUDGET ET CONTROLE DE GESTION",
        "BUREAU DE SELECTION DE L'ADMINISTRATION FEDERALE",
        "CENTRE FEDERAL POUR L'ANALYSE DES FLUX MIGRATOIRES",
        "CENTRE INTERFEDERAL POUR L'EGALITE DES CHANCES ...",
        "CHAMBRES FEDERALES",
        "CHANCELLERIE DU PREMIER MINISTRE",
        "CHANCELLERIE ET SERVICES GENERAUX",
        "CLASSES MOYENNES",
        "COMMERCE EXTERIEUR",
        "COMMISSION BANCAIRE, FINANCIERE ET DES ASSURANCES",
        "COMMISSION DE REGULATION DE L'ELECTRICITE ET DU GAZ",
        "COMMISSION POUR LA PROTECTION DE LA VIE PRIVEE",
        "COMMUNICATIONS",
        "COOPERATION AU DEVELOPPEMENT",
        "CORPS INTERFEDERAL DE L'INSPECTION DES FINANCES",
        "COUR DES COMPTES",
        "DEFENSE NATIONALE",
        "DEVELOPPEMENT DURABLE",
        "ECONOMIE, PME, CLASSES MOYENNES ET ENERGIE",
        "EDUCATION NATIONALE",
        "EDUCATION NATIONALE ET CULTURE FRANCAISE",
        "EDUCATION NATIONALE ET CULTURE NEERLANDAISE",
        "EMPLOI ET TRAVAIL",
        "EMPLOI, TRAVAIL ET CONCERTATION SOCIALE",
        "FINANCES",
        "FONCTION PUBLIQUE",
        "GESTION DES ACTIFS",
        "INSTITUT BELGE DES SERVICES POSTAUX ET DES TELECOMMUNICATIONS",
        "INSTITUT NATIONAL ASSURANCE MALADIE-INVALIDITE",
        "INSTITUT NATIONAL ASSURANCES SOCIALES POUR TRAVAILLEURS INDEPENDANTS",
        "INSTITUT POUR L'EGALITE DES FEMMES ET DES HOMMES",
        "INSTITUT PROFESSIONNEL DES AGENTS IMMOBILIERS",
        "INSTITUT SCIENTIFIQUE DE SANTE PUBLIQUE",
        "INTEGRATION SOCIALE, LUTTE CONTRE LA PAUVRETE ET ECONOMIE SOCIALE",
        "INTERIEUR",
        "JUSTICE",
        "MOBILITE ET TRANSPORTS",
        "OFFICE NATIONAL D'ALLOCATIONS FAMILIALES POUR TRAVAILLEURS SALARIES",
        "OFFICE NATIONAL DE SECURITE SOCIALE DES ADMINISTRATIONS PROVINCIALES ET LOCALES",
        "OFFICE NATIONAL DES PENSIONS",
        "ORDRE DES BARREAUX FLAMANDS",
        "ORDRE DES BARREAUX FRANCOPHONES ET GERMANOPHONE",
        "PENSIONS",
        "PERSONNEL ET ORGANISATION",
        "POLICE FEDERALE ET FONCTIONNEMENT INTEGRE",
        "POLITIQUE SCIENTIFIQUE",
        "POSTES, TELEGRAPHES ET TELEPHONES",
        "PREMIER MINISTRE",
        "PREVOYANCE SOCIALE",
        "PROTECTION DES CONSOMMATEURS",
        "REGIE DES BATIMENTS",
        "SANTE PUBLIQUE",
        "SANTE PUBLIQUE ET ENVIRONNEMENT",
        "SANTE PUBLIQUE ET FAMILLE",
        "SANTE PUBLIQUE, SECURITE DE LA CHAINE ALIMENTAIRE ET ENVIRONNEMENT",
        "SECURITE SOCIALE",
        "STRATEGIE ET APPUI",
        "STRUCTURE DE COORDINATION DE L'INFORMATION PATRIMONIALE",
        "TECHNOLOGIE DE L'INFORMATION ET DE LA COMMUNICATION",
        "TELECOMMUNICATION",
        "TRAVAUX PUBLICS",
    ],
    heads: &[
        "Loi-programme",
        "Loi spéciale",
        "Loi",
        "Arrêté du Régent",
        "Arrêté-loi",
    ],
};

/// The Brussels-Capital Region, the COCOM and the COCOF.
pub(super) const BRUSSELS: Government = Government {
    prefix: "BE-BRU",
    issuers: &[
        "REGION DE BRUXELLES-CAPITALE",
        "REGION BRUXELLOISE",
        "SERVICE PUBLIC REGIONAL DE BRUXELLES",
        "COMMISSION COMMUNAUTAIRE COMMUNE",
        "ASSEMBLEE REUNIE DE LA COMMISSION COMMUNAUTAIRE COMMUNE",
        "COMMISSION COMMUNAUTAIRE FRANCAISE",
    ],
    heads: &[
        "Arrêté du Gouvernement de la Région de Bruxelles-Capitale",
        "Arrêté du Gouvernement de la Région de Bruxelles Capitale",
        "Arrêté du Gouvernement de la Région de BruxellesCapitale",
        "Arrêté de l'Exécutif de la Région de Bruxelles-Capitale",
        "Arrêté du Ministre-Président de la Région de Bruxelles-Capitale",
        "Arrêté des Membres du Collège réuni de la Commission communautaire commune",
        "Arrêté de Membres du Collège réuni de la Commission communautaire commune",
        "Arrêté du Collège réuni de la Commission communautaire commune de la Région de Bruxelles-Capitale",
        "Arrêté du Collège réuni de la Commission communautaire commune de Bruxelles-Capitale",
        "Arrêté du Collège réuni de la Commission communautaire commune",
        "Arrêté du Collège de la Commission communautaire française",
    ],
};

/// The French Community.
pub(super) const FRENCH_COMMUNITY: Government = Government {
    prefix: "BE-CFR",
    issuers: &["COMMUNAUTE FRANCAISE"],
    heads: &[
        "Arrêté du Gouvernement de la Communauté française",
        "Arrêté de l'Exécutif de la Communauté française",
    ],
};

/// The German-speaking Community.
pub(super) const GERMAN_COMMUNITY: Government = Government {
    prefix: "BE-CDE",
    issuers: &["COMMUNAUTE GERMANOPHONE"],
    heads: &[
        "Arrêté du Gouvernement de la Communauté germanophone",
        "Arrêté de l'Exécutif de la Communauté germanophone",
    ],
};

/// `Décret du 5 février 1998 relatif …`: the date after the act type its heading opens on, in
/// whatever case or accents Justel spells it, unless the heading dates it already; a code keeps
/// its name, and the brackets of a heading Justel marks as amended go.
pub(super) fn dated_title(heads: &[&str], heading: &str, adopted: Option<NaiveDate>) -> String {
    dated_title_among(heads.iter().copied(), heading, adopted)
}

fn dated_title_among<'h>(
    mut heads: impl Iterator<Item = &'h str>,
    heading: &str,
    adopted: Option<NaiveDate>,
) -> String {
    let heading = heading.replace('\u{2019}', "'");
    let heading = match heading.strip_prefix('[') {
        Some(inner) => inner.trim_end().strip_suffix(']').unwrap_or(inner).trim(),
        None => heading.as_str(),
    };
    let head = heads.find_map(|h| {
        let end = heading
            .char_indices()
            .nth(h.chars().count())
            .map_or(heading.len(), |(i, _)| i);
        let rest = &heading[end..];
        (lj_fold::fold(&heading[..end]) == lj_fold::fold(h)
            && (rest.is_empty() || rest.starts_with([' ', ','])))
        .then(|| (h, rest.trim_start_matches([' ', ','])))
    });
    let dated = |rest: &str| {
        rest.strip_prefix("du ")
            .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
    };
    match (head, adopted) {
        (Some((head, rest)), Some(d)) if !dated(rest) => {
            let day = match d.day() {
                1 => "1er".to_string(),
                n => n.to_string(),
            };
            format!(
                "{head} du {day} {} {} {rest}",
                MONTHS_FR[d.month0() as usize],
                d.year()
            )
            .trim_end()
            .to_string()
        }
        (Some((head, rest)), _) => format!("{head} {rest}").trim_end().to_string(),
        (None, _) => heading.to_string(),
    }
}

/// An act's nature, from the first words of its title.
pub(super) fn nature(title: &str) -> &'static str {
    let folded = lj_fold::fold(title);
    [
        ("code", "CODE"),
        ("loi", "LOI"),
        ("ordonnance", "ORDONNANCE"),
        ("decret", "DECRET"),
        ("arrete", "ARRETE"),
        ("accord de cooperation", "ACCORD"),
        ("circulaire", "CIRCULAIRE"),
        ("reglement", "REGLEMENT"),
    ]
    .into_iter()
    .find_map(|(opening, nature)| folded.starts_with(opening).then_some(nature))
    .unwrap_or("ACTE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brussels_title_dates_its_act_type() {
        let day = NaiveDate::from_ymd_opt(2026, 7, 9);
        assert_eq!(
            BRUSSELS.title("Arrêté du Gouvernement de la Région de Bruxelles-Capitale modifiant le Code bruxellois de l'aménagement du territoire",
                day
            ),
            "Arrêté du Gouvernement de la Région de Bruxelles-Capitale du 9 juillet 2026 modifiant le Code bruxellois de l'aménagement du territoire"
        );
        assert_eq!(
            BRUSSELS.title("Ordonnance portant le Code bruxellois du Logement", day),
            "Ordonnance du 9 juillet 2026 portant le Code bruxellois du Logement"
        );
        assert_eq!(
            BRUSSELS.title(
                "Code bruxellois de l'aménagement du territoire (CoBAT)",
                day
            ),
            "Code bruxellois de l'aménagement du territoire (CoBAT)"
        );
        assert_eq!(nature("Ordonnance du 9 juillet 2026"), "ORDONNANCE");
    }

    #[test]
    fn a_heading_in_capitals_or_brackets_or_dated_already_reads_as_the_act_is_cited() {
        let day = NaiveDate::from_ymd_opt(1990, 3, 12);
        assert_eq!(
            FRENCH_COMMUNITY.title("ARRETE de l'Exécutif de la Communauté francaise relatif aux bourses d'études",
                day
            ),
            "Arrêté de l'Exécutif de la Communauté française du 12 mars 1990 relatif aux bourses d'études"
        );
        assert_eq!(
            FRENCH_COMMUNITY.title("DECRET contenant le budget", day),
            "Décret du 12 mars 1990 contenant le budget"
        );
        assert_eq!(
            FRENCH_COMMUNITY.title("[ Arrêté du Gouvernement de la Communauté française du 25 janvier 2024 portant exécution du Code ]",
                NaiveDate::from_ymd_opt(2024, 1, 25)
            ),
            "Arrêté du Gouvernement de la Communauté française du 25 janvier 2024 portant exécution du Code"
        );
        assert_eq!(
            GERMAN_COMMUNITY.title("Arrêté du Gouvernement modifiant l'arrêté du Gouvernement du 29 février 2024", day),
            "Arrêté du Gouvernement du 12 mars 1990 modifiant l'arrêté du Gouvernement du 29 février 2024"
        );
        assert_eq!(
            BRUSSELS.title("Arrêté ministériel portant exécution", day),
            "Arrêté ministériel du 12 mars 1990 portant exécution"
        );
        assert_eq!(
            FEDERAL.title("Loi-programme", day),
            "Loi-programme du 12 mars 1990"
        );
        assert_eq!(
            FEDERAL.title("Arrêté-Loi relatif aux réquisitions", day),
            "Arrêté-loi du 12 mars 1990 relatif aux réquisitions"
        );
        assert_eq!(
            FEDERAL.title("Arrêté royal rendant obligatoire la convention collective de travail du 5 mai 1989", day),
            "Arrêté royal du 12 mars 1990 rendant obligatoire la convention collective de travail du 5 mai 1989"
        );
        assert_eq!(nature("Loi spéciale du 12 mars 1990"), "LOI");
        assert_eq!(nature("DECRET contenant le budget"), "DECRET");
        assert_eq!(nature("Contrat de gestion"), "ACTE");
    }
}
