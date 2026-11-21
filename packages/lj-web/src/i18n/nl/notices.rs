use crate::i18n::NoticesUi;

pub const NOTICES: NoticesUi = NoticesUi {
    unavailable,
    sort_deadline: "Dichtstbijzijnde termijn",
    sort_seen: "Laatst opgehaald",
    sort_published: "Meest recent bekendgemaakt",
    open_notices: "Bekendgemaakte opdrachten die nog openstaan",
    sort_by: "Sorteren op",
    subscribe: "Op deze filters abonneren",
    rss: "RSS-feed met nieuwe aankondigingen",
    ics: "Agenda van de termijnen (ICS)",
    no_account: "Zonder account: het adres is het abonnement.",
    closed_included: "Inclusief afgesloten opdrachten",
    none_open: "Geen openstaande aankondiging voor deze filters.",
    due_before,
    published_on,
    filter: "Filteren…",
    filter_zone: "Filteren (bv. Guyane…)",
    filter_cpv: "Filteren (bv. werken…)",
    filter_trade: "Filteren (bv. schrijnwerk…)",
    see_all,
    channels,
    title: "Bekendmakingen - LibreJustice",
    title_for,
    heading: "Bekendmakingen",
    by_department: "Per departement",
    public_markets: "Overheidsopdrachten",
    index_description,
    index_title: "Overheidsopdrachten per departement - LibreJustice",
    index_heading: "Overheidsopdrachten per departement",
    index_intro,
    search_all: "Zoeken in alle aankondigingen",
    departments: "Departementen",
    by_nature: "Per soort",
    department_empty: "Geen openstaande opdracht in dit departement",
    department_title,
    department_description,
    department_heading,
    department_intro,
    filter_by_trade: "Filteren op sector of vakgebied",
    follow_department: "Dit departement volgen",
    follow_text: "Twee feeds, zonder account of registratie: de nieuwe aankondigingen in uw feedlezer, de termijnen in uw agenda.",
    calendar: "Kalender van de termijnen (ICS)",
    merged: "Deze aankondiging werd samengevoegd: zie de bijgewerkte fiche",
    description_awarded,
    description_open,
    description_other,
    closed: "Opdracht afgesloten: de termijn is verstreken, of de aankondiging is van de platformen verdwenen.",
    fact_due: "Indiening van de offertes",
    not_published: "niet bekendgemaakt",
    fact_published: "Bekendmaking",
    fact_nature: "Soort",
    fact_cpv: "Categorie (CPV)",
    fact_cpv_inferred: "Categorie (CPV), afgeleid uit de titel",
    fact_trade: "Vakgebied",
    fact_trade_inferred: "Vakgebied, afgeleid uit de titel",
    fact_reference: "Referentie van de aanbestedende overheid",
    fact_procedure: "Procedure",
    fact_place: "Plaats van uitvoering",
    whole_country: "Heel Frankrijk",
    department_count,
    fact_estimate: "Geraamde waarde",
    fact_seen: "Opgehaald",
    see_notice: "De aankondiging bekijken",
    file_bid: "Offerte indienen, opdrachtdocumenten raadplegen",
    actors: "Betrokkenen",
    boamp_notice: "Aankondiging in het BOAMP",
    joue_notice: "Aankondiging in het Publicatieblad van de EU",
    platform_page: "Pagina van de opdracht op het platform",
    lots,
    lot_n,
    criteria: "Gunningscriteria",
    award_notice: "Aankondiging van gegunde opdracht",
    award: "Gunning",
    offers_received,
    awarded_on,
    previous: "Vorige",
    next: "Volgende",
    page_of,
};

fn unavailable(error: &str) -> String {
    format!("Zoeken niet beschikbaar ({error}).")
}

fn due_before(day: &str) -> String {
    format!("Indienen vóór {day}")
}

fn published_on(day: &str) -> String {
    format!("Bekendgemaakt op {day}")
}

fn see_all(total: i64) -> String {
    format!(
        "{} · alles bekijken",
        crate::helpers::group_thousands(&super::numbers::NUMBERS, total)
    )
}

fn channels(n: usize) -> String {
    format!("{n} kanalen")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Pagina {page} van {pages}")
}

fn title_for(query: &str) -> String {
    format!("{query} - Bekendmakingen - LibreJustice")
}

fn index_description(total: &str) -> String {
    format!(
        "De {total} openstaande overheidsopdrachten in Frankrijk, \
         departement per departement: voorwerp, aanbestedende overheid en uiterste datum voor de indiening van de offertes."
    )
}

fn index_intro(total: &str) -> String {
    format!(
        "{total} openstaande opdrachten, verzameld op de kopersprofielen en in het BOAMP. \
         Elk departement leidt naar zijn lopende aankondigingen, van de eerstvolgende tot de laatste termijn."
    )
}

fn department_title(name: &str, page: u32) -> String {
    if page > 1 {
        format!("Overheidsopdrachten: {name} (pagina {page}) - LibreJustice")
    } else {
        format!("Lopende overheidsopdrachten: {name} - LibreJustice")
    }
}

fn department_description(total: &str, name: &str) -> String {
    format!(
        "De {total} openstaande overheidsopdrachten in {name}: voorwerp, aanbestedende overheid, \
         uiterste datum voor de indiening van de offertes en link naar de opdrachtdocumenten."
    )
}

fn department_heading(name: &str) -> String {
    format!("Overheidsopdrachten: {name}")
}

fn department_intro(total: &str) -> String {
    format!("{total} openstaande opdrachten, van de eerstvolgende tot de laatste termijn.")
}

fn description_awarded(buyer: &str) -> String {
    format!("Gegunde opdracht, {buyer}: opdrachtnemers, bedragen en aantal ontvangen offertes.")
}

fn description_open(department: &str, buyer: &str, day: &str) -> String {
    format!(
        "Overheidsopdracht in {department}: {buyer}, indiening van de offertes tot {day}, opdrachtdocumenten en aankondiging met een rechtstreekse link."
    )
}

fn description_other(buyer: &str) -> String {
    format!("Overheidsopdracht, {buyer}: voorwerp, referentie, aankondiging en opdrachtdocumenten.")
}

fn lots(n: usize) -> String {
    match n {
        1 => "Opdeling in percelen · 1 perceel".to_string(),
        n => format!("Opdeling in percelen · {n} percelen"),
    }
}

fn lot_n(n: usize) -> String {
    format!("Perceel {n}")
}

fn offers_received(n: i64) -> String {
    match n {
        1 => "1 offerte ontvangen".to_string(),
        n => format!("{n} offertes ontvangen"),
    }
}

fn awarded_on(day: &str) -> String {
    format!("gegund op {day}")
}

fn department_count(count: usize) -> String {
    format!("{count} departementen")
}
