use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::EntityUi;

pub const ENTITY: EntityUi = EntityUi {
    not_found_title: "Rechtseinheit nicht gefunden - LibreJustice",
    company: "Unternehmen",
    association: "Verein",
    lawyer: "Anwältin/Anwalt",
    supreme_lawyer: "Avocat aux Conseils",
    entity: "Rechtseinheit",
    supreme_lawyers: "Avocats aux Conseils",
    decisions_referenced,
    meta_description,
    active: "Aktiv",
    acronym,
    denominations: "Namensverlauf",
    since,
    until,
    litigation: "Verfahren",
    decisions: "Entscheide",
    as_applicant: "Als klagende Partei",
    as_defendant: "Als beklagte Partei",
    for_applicant: "Für die klagende Partei",
    for_defendant: "Für die beklagte Partei",
    no_decision: "Bisher nennt kein Entscheid diese Rechtseinheit.",
    by_jurisdiction: "Nach Gericht",
    by_year: "Nach Jahr",
    registry: "Register",
    head_office: "Sitz",
    activity: "Tätigkeit (NAF)",
    created: "Gründung",
    workforce: "Mitarbeitende",
    officers: "Geschäftsleitung",
    accounts: "Publizierte Jahresrechnungen",
    turnover,
    net_result,
    notices: "Amtsblätter",
    notices_total,
    documents: "Belege und Jahresrechnungen",
    show_documents: "Hinterlegte Dokumente anzeigen",
    loading: "Wird geladen …",
    no_documents: "Keine im Register hinterlegten Dokumente.",
    annual_accounts: "Jahresrechnungen",
    deeds: "Belege",
    documents_source: "Quelle: INPI, nationales Unternehmensregister.",
    documents_unavailable: "Liste derzeit nicht verfügbar. ",
    see_inpi: "Auf data.inpi.fr ansehen",
    loading_decisions: "Entscheide werden geladen …",
    decisions_unavailable,
    no_decision_on_page: "Kein Entscheid auf dieser Seite.",
    counsel: "Rechtsvertretung",
    applicant: "Klagende Partei",
    defendant: "Beklagte Partei",
};

fn decisions_referenced(n: i64) -> String {
    match n {
        0 => "Keine verzeichneten Entscheide".to_string(),
        1 => "1 verzeichneter Entscheid".to_string(),
        n => format!("{} verzeichnete Entscheide", group_thousands(&NUMBERS, n)),
    }
}

/// The page hands the category lowercased, as French writes it mid-sentence; German keeps the noun's capital.
fn meta_description(name: &str, category: &str, decisions: &str) -> String {
    let category = [
        ENTITY.company,
        ENTITY.association,
        ENTITY.lawyer,
        ENTITY.supreme_lawyer,
        ENTITY.entity,
    ]
    .into_iter()
    .find(|label| label.to_lowercase() == category)
    .unwrap_or(category);
    format!("{name}, {category}. {decisions} auf LibreJustice.")
}

fn acronym(acronym: &str) -> String {
    format!("Abkürzung: {acronym}")
}

fn since(date: &str) -> String {
    format!("seit dem {date}")
}

fn until(date: &str) -> String {
    format!("bis zum {date}")
}

fn turnover(amount: &str) -> String {
    format!("Umsatz {amount}")
}

fn net_result(amount: &str) -> String {
    format!("Ergebnis {amount}")
}

fn notices_total(n: i64) -> String {
    match n {
        1 => "1 Publikation insgesamt.".to_string(),
        n => format!("{} Publikationen insgesamt.", group_thousands(&NUMBERS, n)),
    }
}

fn decisions_unavailable(error: &str) -> String {
    format!("Entscheide nicht verfügbar ({error}).")
}
