use crate::i18n::NoticesUi;

pub const NOTICES: NoticesUi = NoticesUi {
    unavailable,
    sort_deadline: "Nächste Frist zuerst",
    sort_seen: "Zuletzt erfasst",
    sort_published: "Zuletzt publiziert",
    open_notices: "Publizierte, noch offene Ausschreibungen",
    sort_by: "Sortieren nach",
    subscribe: "Diese Filter abonnieren",
    rss: "RSS-Feed der Neuheiten",
    ics: "Fristenkalender (ICS)",
    no_account: "Ohne Konto: Die URL ist das Abonnement.",
    closed_included: "Auch abgeschlossene Ausschreibungen",
    none_open: "Keine offenen Ausschreibungen für diese Filter.",
    due_before,
    published_on,
    filter: "Filtern …",
    filter_zone: "Filtern (z. B. Guyane …)",
    filter_cpv: "Filtern (z. B. travaux …)",
    filter_trade: "Filtern (z. B. menuiserie …)",
    see_all,
    channels,
    title: "Amtsblätter - LibreJustice",
    title_for,
    heading: "Amtsblätter",
    by_department: "Nach Departement",
    public_markets: "Öffentliches Beschaffungswesen",
    index_description,
    index_title: "Öffentliches Beschaffungswesen nach Departement - LibreJustice",
    index_heading: "Öffentliches Beschaffungswesen nach Departement",
    index_intro,
    search_all: "In allen Ausschreibungen suchen",
    departments: "Departemente",
    by_nature: "Nach Art",
    department_empty: "Keine offene Ausschreibung in diesem Departement",
    department_title,
    department_description,
    department_heading,
    department_intro,
    filter_by_trade: "Nach Bereich oder Branche filtern",
    follow_department: "Dieses Departement verfolgen",
    follow_text: "Zwei Feeds, ohne Konto und ohne Anmeldung: die Neuheiten in Ihrem Feedreader, die Fristen in Ihrem Kalender.",
    calendar: "Fristenkalender (ICS)",
    merged: "Diese Ausschreibung wurde zusammengeführt: zum aktuellen Eintrag",
    description_awarded,
    description_open,
    description_other,
    closed: "Ausschreibung abgeschlossen: Die Frist ist abgelaufen oder die Publikation ist von den Plattformen verschwunden.",
    fact_due: "Angebotsfrist",
    not_published: "nicht publiziert",
    fact_published: "Publikationsdatum",
    fact_nature: "Art",
    fact_cpv: "CPV-Code",
    fact_cpv_inferred: "CPV-Code, aus dem Titel abgeleitet",
    fact_trade: "Branche",
    fact_trade_inferred: "Branche, aus dem Titel abgeleitet",
    fact_reference: "Referenz des Auftraggebers",
    fact_procedure: "Verfahrensart",
    fact_place: "Ort der Auftragserfüllung",
    whole_country: "Ganz Frankreich",
    department_count,
    fact_estimate: "Geschätzter Wert",
    fact_seen: "Erfasst am",
    see_notice: "Publikation ansehen",
    file_bid: "Angebot einreichen, Unterlagen einsehen",
    actors: "Beteiligte",
    boamp_notice: "Publikation im BOAMP",
    joue_notice: "Publikation im Amtsblatt der EU",
    platform_page: "Seite der Ausschreibung auf der Plattform",
    lots,
    lot_n,
    criteria: "Zuschlagskriterien",
    award_notice: "Publikation des Zuschlags",
    award: "Zuschlag",
    offers_received,
    awarded_on,
    previous: "Zurück",
    next: "Weiter",
    page_of,
};

fn unavailable(error: &str) -> String {
    format!("Suche nicht verfügbar ({error}).")
}

fn due_before(day: &str) -> String {
    format!("Angebotsfrist: {day}")
}

fn published_on(day: &str) -> String {
    format!("Publiziert am {day}")
}

fn see_all(total: i64) -> String {
    format!(
        "{} · alle anzeigen",
        crate::helpers::group_thousands(&super::numbers::NUMBERS, total)
    )
}

fn channels(n: usize) -> String {
    format!("{n} Kanäle")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Seite {page} von {pages}")
}

fn title_for(query: &str) -> String {
    format!("{query} - Amtsblätter - LibreJustice")
}

fn index_description(total: &str) -> String {
    format!(
        "Die {total} offenen Ausschreibungen des öffentlichen Beschaffungswesens in Frankreich, Departement für Departement: Gegenstand, Auftraggeber und Angebotsfrist."
    )
}

fn index_intro(total: &str) -> String {
    format!(
        "{total} offene Ausschreibungen, erfasst auf den Beschafferprofilen und im BOAMP. Jedes Departement führt zu seinen laufenden Ausschreibungen, die nächste Frist zuerst."
    )
}

fn department_title(name: &str, page: u32) -> String {
    if page > 1 {
        format!("Öffentliches Beschaffungswesen: {name} (Seite {page}) - LibreJustice")
    } else {
        format!("Laufende Ausschreibungen: {name} - LibreJustice")
    }
}

fn department_description(total: &str, name: &str) -> String {
    format!(
        "Die {total} offenen Ausschreibungen des öffentlichen Beschaffungswesens im Departement {name}: Gegenstand, Auftraggeber, Angebotsfrist und Link zu den Unterlagen."
    )
}

fn department_heading(name: &str) -> String {
    format!("Öffentliches Beschaffungswesen: {name}")
}

fn department_intro(total: &str) -> String {
    format!("{total} offene Ausschreibungen, die nächste Frist zuerst.")
}

fn description_awarded(buyer: &str) -> String {
    format!("Zuschlag erteilt, {buyer}: berücksichtigte Anbietende, Beträge und Anzahl eingegangener Angebote.")
}

fn description_open(department: &str, buyer: &str, day: &str) -> String {
    format!(
        "Öffentlicher Auftrag im Departement {department}: {buyer}, Angebotsfrist {day}, Unterlagen und Ausschreibung direkt verlinkt."
    )
}

fn description_other(buyer: &str) -> String {
    format!("Öffentlicher Auftrag, {buyer}: Gegenstand, Referenz, Ausschreibung und Ausschreibungsunterlagen.")
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "e"
    }
}

fn lots(n: usize) -> String {
    format!("Aufteilung in Lose · {n} Los{}", plural(n as i64))
}

fn lot_n(n: usize) -> String {
    format!("Los {n}")
}

fn offers_received(n: i64) -> String {
    match n {
        1 => "1 eingegangenes Angebot".to_string(),
        n => format!("{n} eingegangene Angebote"),
    }
}

fn awarded_on(day: &str) -> String {
    format!("Zuschlag am {day}")
}

fn department_count(count: usize) -> String {
    format!("{count} Departemente")
}
