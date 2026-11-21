use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::EntityUi;

pub const ENTITY: EntityUi = EntityUi {
    not_found_title: "Entiteit niet gevonden - LibreJustice",
    company: "Onderneming",
    association: "Vereniging",
    lawyer: "Advocaat",
    supreme_lawyer: "Cassatieadvocaat",
    entity: "Entiteit",
    supreme_lawyers: "Cassatieadvocaten",
    decisions_referenced,
    meta_description,
    active: "Actief",
    acronym,
    denominations: "Opeenvolgende benamingen",
    since,
    until,
    litigation: "Geschillen",
    decisions: "Beslissingen",
    as_applicant: "Als eiser",
    as_defendant: "Als verweerder",
    for_applicant: "Voor de eiser",
    for_defendant: "Voor de verweerder",
    no_decision: "Voorlopig verwijst geen enkele beslissing naar deze entiteit.",
    by_jurisdiction: "Per rechtscollege",
    by_year: "Per jaar",
    registry: "Register",
    head_office: "Zetel",
    activity: "Activiteit (NAF)",
    created: "Oprichting",
    workforce: "Personeelsbestand",
    officers: "Bestuurders",
    accounts: "Bekendgemaakte jaarrekeningen",
    turnover,
    net_result,
    notices: "Bekendmakingen",
    notices_total,
    documents: "Akten en jaarrekeningen",
    show_documents: "De neergelegde documenten tonen",
    loading: "Laden…",
    no_documents: "Geen document neergelegd in het register.",
    annual_accounts: "Jaarrekeningen",
    deeds: "Akten",
    documents_source: "Bron: INPI, het Franse nationale ondernemingsregister.",
    documents_unavailable: "Lijst momenteel niet beschikbaar. ",
    see_inpi: "Bekijken op data.inpi.fr",
    loading_decisions: "Beslissingen worden geladen…",
    decisions_unavailable,
    no_decision_on_page: "Geen beslissing op deze pagina.",
    counsel: "Raadsman",
    applicant: "Eiser",
    defendant: "Verweerder",
};

fn decisions_referenced(n: i64) -> String {
    match n {
        0 => "Geen beslissingen".to_string(),
        1 => "1 beslissing".to_string(),
        n => format!("{} beslissingen", group_thousands(&NUMBERS, n)),
    }
}

fn meta_description(name: &str, category: &str, decisions: &str) -> String {
    format!("{name}, {category}. {decisions} op LibreJustice.")
}

fn acronym(acronym: &str) -> String {
    format!("Afkorting: {acronym}")
}

fn since(date: &str) -> String {
    format!("sinds {date}")
}

fn until(date: &str) -> String {
    format!("tot {date}")
}

fn turnover(amount: &str) -> String {
    format!("omzet {amount}")
}

fn net_result(amount: &str) -> String {
    format!("resultaat {amount}")
}

fn notices_total(n: i64) -> String {
    match n {
        1 => "1 bekendmaking in totaal.".to_string(),
        n => format!("{} bekendmakingen in totaal.", group_thousands(&NUMBERS, n)),
    }
}

fn decisions_unavailable(error: &str) -> String {
    format!("Beslissingen niet beschikbaar ({error}).")
}
