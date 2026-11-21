use crate::i18n::HubsUi;

pub const HUBS: HubsUi = HubsUi {
    unknown_jurisdiction: "Unbekannte Rechtsordnung",
    invalid_year: "Ungültiges Jahr",
    undated: "Ohne Datum",
    texts_and_codes: "Erlasse & Gesetzbücher",
    search_these: "In diesen Erlassen suchen",
    n_texts,
    catalogue_title: "Verzeichnis der Erlasse - LibreJustice",
    catalogue_title_for,
    catalogue_description: "Alle Erlasse des Bestands (Gesetzbücher, Gesetze und Verordnungen, Dekrete, Beschlüsse, Gesamtarbeitsverträge, europäische Erlasse, Staatsverträge, Kreisschreiben, BOFiP), nach Sammlung und dann nach Jahr.",
    catalogue_heading: "Verzeichnis der Erlasse",
    catalogue_intro: "Blättern Sie durch die Erlasse des Bestands, zuerst nach Sammlung, dann nach Jahr. Jede Sammlung öffnet die vollständige Liste ihrer Erlasse.",
    other_jurisdictions: "Weitere Rechtsordnungen",
    in_force_lead: "Dieses Verzeichnis führt alle Erlasse der Sammlung auf. Um nur jene zu sehen, die an einem bestimmten Datum in einem Gebiet gelten: ",
    in_force_link: "was an einem Ort gilt",
    fond_title,
    fond_description,
    fond_intro,
    by_year: "Erlasse nach Jahr",
    year_title,
    year_description,
    year_heading,
    year_intro,
    search_year,
    codes_title: "Gesetzbücher: konsolidierte Fassungen - LibreJustice",
    codes_description: "Verzeichnis der konsolidierten Gesetzbücher und Verfassungen, eine Gruppe pro Rechtsordnung, von Frankreich bis zu jedem Staat oder Gebiet. Fassungen nach Stichtag, verknüpfte Artikel.",
    codes: "Gesetzbücher",
    constitutions: "Verfassungen",
    codes_count,
    n_articles,
    corpus_title,
    corpus_description,
    applicability: "Geltungsbereich",
    corpus_heading,
    corpus_intro,
    corpus_empty: "Kein ausgewerteter Erlass nennt diesen Ort an diesem Datum.",
    and_more,
    on: "Stand am",
    not_found_title: "Seite nicht gefunden - LibreJustice",
    pagination: "Seitennavigation",
    first: "Erste",
    previous: "Vorherige",
    next: "Nächste",
    last: "Letzte",
    page_of,
    jurisdictions_title: "Gerichte - LibreJustice",
    jurisdictions_description: "Alle Gerichte des Bestands (Conseil d'État, Cour de cassation, Berufungsgerichte, Verwaltungsgerichte und erstinstanzliche Gerichte, europäische Gerichte), mit ihren Entscheiden nach Jahr.",
    case_law: "Rechtsprechung",
    jurisdictions: "Gerichte",
    jurisdictions_intro: "Blättern Sie durch die Entscheide des Bestands, zuerst nach Gericht, dann nach Jahr. Jedes Gericht öffnet die vollständige Liste seiner Entscheide.",
    search_decisions: "In diesen Entscheiden suchen",
    n_decisions,
    court_title,
    court_description,
    court_intro,
    decisions_by_year: "Entscheide nach Jahr",
    court_year_title,
    court_year_description,
    court_year_heading,
    court_year_intro,
    search_year_decisions,
};

fn n_texts(count: &str, n: i64) -> String {
    format!("{count} Erlass{}", if n == 1 { "" } else { "e" })
}

fn catalogue_title_for(jurisdiction: &str) -> String {
    format!("Verzeichnis der Erlasse ({jurisdiction}) - LibreJustice")
}

fn fond_title(fond: &str) -> String {
    format!("{fond}: Erlasse - LibreJustice")
}

fn fond_description(count: &str, fond: &str) -> String {
    format!("Die {count} Erlasse der Sammlung «{fond}», nach Jahr.")
}

fn fond_intro(count: &str, span: Option<(i32, i32)>) -> String {
    match span {
        Some((min, max)) => format!("{count} Erlasse, von {min} bis {max}."),
        None => format!("{count} Erlasse."),
    }
}

fn year_title(fond: &str, year: &str, page: u32) -> String {
    if page > 1 {
        format!("{fond}: Erlasse {year} (Seite {page}) - LibreJustice")
    } else {
        format!("{fond}: Erlasse {year} - LibreJustice")
    }
}

fn year_description(count: &str, year: Option<i32>, fond: &str) -> String {
    match year {
        Some(y) => {
            format!("Die {count} Erlasse aus dem Jahr {y} der Sammlung «{fond}», im Volltext.")
        }
        None => format!("Die {count} undatierten Erlasse der Sammlung «{fond}»."),
    }
}

fn year_heading(fond: &str, year: &str) -> String {
    format!("{fond} · {year}")
}

fn year_intro(count: &str, year: Option<i32>) -> String {
    match year {
        Some(_) => format!("{count} Erlasse, die neuesten zuerst."),
        None => {
            format!("{count} Erlasse ohne bekanntes Datum der Unterzeichnung oder Publikation.")
        }
    }
}

fn search_year(year: i32) -> String {
    format!("In den Erlassen aus {year} suchen")
}

fn codes_count(n: usize) -> String {
    match n {
        1 => "1 Erlass.".to_string(),
        n => format!("{n} Erlasse."),
    }
}

fn n_articles(n: i64) -> String {
    format!("{n} Artikel")
}

fn corpus_title(place: &str) -> String {
    format!("Geltendes Recht: {place} - LibreJustice")
}

fn corpus_description(place: &str, day: &str) -> String {
    format!(
        "Die am {day} anwendbaren Erlasse, Standort {place}, nach Amtsblatt und dann nach Art geordnet."
    )
}

fn corpus_heading(place: &str) -> String {
    format!("Geltendes Recht: {place}")
}

fn corpus_intro(day: &str, count: &str) -> String {
    format!(
        "Am {day} gelten dort {count}. Ein Erlass zählt, sobald einer seiner Artikel gilt. Eine zu lange Gruppe zeigt nur einige Titel."
    )
}

fn and_more(count: &str) -> String {
    format!("und {count} weitere")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Seite {page} von {pages}")
}

fn n_decisions(count: &str, n: i64) -> String {
    format!("{count} Entscheid{}", if n == 1 { "" } else { "e" })
}

fn court_title(court: &str) -> String {
    format!("{court}: Entscheide - LibreJustice")
}

fn court_description(count: &str, court: &str) -> String {
    format!("Die {count} Entscheide des Gerichts «{court}», nach Jahr, im Volltext.")
}

fn court_intro(count: &str, from: i32, to: i32) -> String {
    format!("{count} Entscheide im Volltext, von {from} bis {to}.")
}

fn court_year_title(court: &str, year: i32, page: u32) -> String {
    if page > 1 {
        format!("{court}: Entscheide {year} (Seite {page}) - LibreJustice")
    } else {
        format!("{court}: Entscheide {year} - LibreJustice")
    }
}

fn court_year_description(count: &str, year: i32, court: &str) -> String {
    format!("Die {count} Entscheide des Gerichts «{court}» aus dem Jahr {year}, im Volltext.")
}

fn court_year_heading(court: &str, year: i32) -> String {
    format!("{court} · Entscheide {year}")
}

fn court_year_intro(count: &str, year: i32) -> String {
    format!("{count} Entscheide aus dem Jahr {year}, die neuesten zuerst.")
}

fn search_year_decisions(year: i32) -> String {
    format!("In den Entscheiden aus {year} suchen")
}
