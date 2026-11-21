use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::EntityUi;

pub const ENTITY: EntityUi = EntityUi {
    not_found_title: "Entité introuvable - LibreJustice",
    company: "Entreprise",
    association: "Association",
    lawyer: "Avocat",
    supreme_lawyer: "Avocat aux Conseils",
    entity: "Entité",
    supreme_lawyers: "Avocats aux Conseils",
    decisions_referenced,
    meta_description,
    active: "En activité",
    acronym,
    denominations: "Chronologie des dénominations",
    since,
    until,
    litigation: "Contentieux",
    decisions: "Décisions",
    as_applicant: "Comme demandeur",
    as_defendant: "Comme défendeur",
    for_applicant: "En demande",
    for_defendant: "En défense",
    no_decision: "Aucune décision ne référence cette entité pour l'instant.",
    by_jurisdiction: "Par juridiction",
    by_year: "Par année",
    registry: "Registre",
    head_office: "Siège",
    activity: "Activité (NAF)",
    created: "Création",
    workforce: "Effectif",
    officers: "Dirigeants",
    accounts: "Comptes publiés",
    turnover,
    net_result,
    notices: "Journaux",
    notices_total,
    documents: "Actes et comptes annuels",
    show_documents: "Afficher les documents déposés",
    loading: "Chargement…",
    no_documents: "Aucun document déposé au registre.",
    annual_accounts: "Comptes annuels",
    deeds: "Actes",
    documents_source: "Source : INPI, registre national des entreprises.",
    documents_unavailable: "Liste indisponible pour le moment. ",
    see_inpi: "Consulter sur data.inpi.fr",
    loading_decisions: "Chargement des décisions…",
    decisions_unavailable,
    no_decision_on_page: "Aucune décision sur cette page.",
    counsel: "Conseil",
    applicant: "Demandeur",
    defendant: "Défendeur",
};

fn decisions_referenced(n: i64) -> String {
    match n {
        0 => "aucune décision référencée".to_string(),
        1 => "1 décision référencée".to_string(),
        n => format!("{} décisions référencées", group_thousands(&NUMBERS, n)),
    }
}

fn meta_description(name: &str, category: &str, decisions: &str) -> String {
    format!("{name}, {category}. {decisions} sur LibreJustice.")
}

fn acronym(acronym: &str) -> String {
    format!("Sigle : {acronym}")
}

fn since(date: &str) -> String {
    format!("depuis {date}")
}

fn until(date: &str) -> String {
    format!("jusqu'au {date}")
}

fn turnover(amount: &str) -> String {
    format!("CA {amount}")
}

fn net_result(amount: &str) -> String {
    format!("résultat {amount}")
}

fn notices_total(n: i64) -> String {
    format!("{} annonces au total.", group_thousands(&NUMBERS, n))
}

fn decisions_unavailable(error: &str) -> String {
    format!("Décisions indisponibles ({error}).")
}
