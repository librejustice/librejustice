use crate::i18n::HubsUi;

pub const HUBS: HubsUi = HubsUi {
    unknown_jurisdiction: "Onbekend rechtsstelsel",
    invalid_year: "Ongeldig jaar",
    undated: "Zonder datum",
    texts_and_codes: "Teksten en wetboeken",
    search_these: "Zoeken in deze teksten",
    n_texts,
    catalogue_title: "Catalogus van de regelgeving - LibreJustice",
    catalogue_title_for,
    catalogue_description: "Alle teksten van de regelgeving in het corpus (wetboeken, wetten en ordonnanties, decreten, besluiten, collectieve arbeidsovereenkomsten, Europese teksten, verdragen, omzendbrieven, BOFiP), per verzameling en daarna per jaar.",
    catalogue_heading: "Catalogus van de regelgeving",
    catalogue_intro: "Blader door de teksten van het corpus per verzameling en daarna per jaar. Elke verzameling opent de volledige lijst van haar teksten.",
    other_jurisdictions: "Andere rechtsstelsels",
    in_force_lead: "Deze catalogus vermeldt alle teksten van de verzameling. Om alleen de teksten te behouden die op een bepaalde datum in een gebied van toepassing zijn: ",
    in_force_link: "wat op een plaats van toepassing is",
    fond_title,
    fond_description,
    fond_intro,
    by_year: "Teksten per jaar",
    year_title,
    year_description,
    year_heading,
    year_intro,
    search_year,
    codes_title: "Wetboeken: geconsolideerde teksten - LibreJustice",
    codes_description: "Catalogus van de geconsolideerde wetboeken en grondwetten, één groep per rechtsstelsel, van Frankrijk tot elke staat of elk gebied. Versies op datum, gekoppelde artikelen.",
    codes: "Wetboeken",
    constitutions: "Grondwetten",
    codes_count,
    n_articles,
    corpus_title,
    corpus_description,
    applicability: "Toepasselijkheid",
    corpus_heading,
    corpus_intro,
    corpus_empty: "Geen enkele verwerkte tekst noemt deze plaats op deze datum.",
    and_more,
    on: "Op",
    not_found_title: "Pagina niet gevonden - LibreJustice",
    pagination: "Paginering",
    first: "Eerste",
    previous: "Vorige",
    next: "Volgende",
    last: "Laatste",
    page_of,
    jurisdictions_title: "Rechtscolleges - LibreJustice",
    jurisdictions_description: "Alle rechtscolleges van het corpus (hoven en rechtbanken, administratieve rechtscolleges, Europese hoven), met hun beslissingen per jaar.",
    case_law: "Rechtspraak",
    jurisdictions: "Rechtscolleges",
    jurisdictions_intro: "Blader door de beslissingen van het corpus per rechtscollege en daarna per jaar. Elk rechtscollege opent de volledige lijst van zijn beslissingen.",
    search_decisions: "Zoeken in deze beslissingen",
    n_decisions,
    court_title,
    court_description,
    court_intro,
    decisions_by_year: "Beslissingen per jaar",
    court_year_title,
    court_year_description,
    court_year_heading,
    court_year_intro,
    search_year_decisions,
};

fn n_texts(count: &str, n: i64) -> String {
    if n == 1 {
        format!("{count} tekst")
    } else {
        format!("{count} teksten")
    }
}

fn catalogue_title_for(jurisdiction: &str) -> String {
    format!("Catalogus van de regelgeving ({jurisdiction}) - LibreJustice")
}

fn fond_title(fond: &str) -> String {
    format!("{fond}: teksten - LibreJustice")
}

fn fond_description(count: &str, fond: &str) -> String {
    format!("De {count} teksten van de verzameling \"{fond}\", per jaar.")
}

fn fond_intro(count: &str, span: Option<(i32, i32)>) -> String {
    match span {
        Some((min, max)) => format!("{count} teksten, van {min} tot {max}."),
        None => format!("{count} teksten."),
    }
}

fn year_title(fond: &str, year: &str, page: u32) -> String {
    if page > 1 {
        format!("{fond}: teksten {year} (pagina {page}) - LibreJustice")
    } else {
        format!("{fond}: teksten {year} - LibreJustice")
    }
}

fn year_description(count: &str, year: Option<i32>, fond: &str) -> String {
    match year {
        Some(y) => format!(
            "De {count} teksten van {y} uit de verzameling \"{fond}\", in hun volledige tekst."
        ),
        None => format!("De {count} teksten zonder datum uit de verzameling \"{fond}\"."),
    }
}

fn year_heading(fond: &str, year: &str) -> String {
    format!("{fond} · {year}")
}

fn year_intro(count: &str, year: Option<i32>) -> String {
    match year {
        Some(_) => format!("{count} teksten, van de recentste tot de oudste."),
        None => format!("{count} teksten zonder bekende datum van ondertekening of bekendmaking."),
    }
}

fn search_year(year: i32) -> String {
    format!("Zoeken in de teksten van {year}")
}

fn codes_count(n: usize) -> String {
    if n == 1 {
        "1 tekst.".to_string()
    } else {
        format!("{n} teksten.")
    }
}

fn n_articles(n: i64) -> String {
    if n == 1 {
        "1 artikel".to_string()
    } else {
        format!("{n} artikelen")
    }
}

fn corpus_title(place: &str) -> String {
    format!("Geldend recht: {place} - LibreJustice")
}

fn corpus_description(place: &str, day: &str) -> String {
    format!("De teksten die op {day} van toepassing zijn (gebied: {place}), per publicatieblad en daarna per soort.")
}

fn corpus_heading(place: &str) -> String {
    format!("Geldend recht: {place}")
}

fn corpus_intro(day: &str, count: &str) -> String {
    format!(
        "Van toepassing op {day}: {count}. Een tekst telt mee zodra een van zijn artikelen van toepassing is. \
         Van een te lange groep worden maar enkele titels getoond."
    )
}

fn and_more(count: &str) -> String {
    format!("en {count} andere")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Pagina {page} / {pages}")
}

fn n_decisions(count: &str, n: i64) -> String {
    if n == 1 {
        format!("{count} beslissing")
    } else {
        format!("{count} beslissingen")
    }
}

fn court_title(court: &str) -> String {
    format!("{court}: beslissingen - LibreJustice")
}

fn court_description(count: &str, court: &str) -> String {
    format!("De {count} beslissingen van het rechtscollege \"{court}\", per jaar, in hun volledige tekst.")
}

fn court_intro(count: &str, from: i32, to: i32) -> String {
    format!("{count} beslissingen in hun volledige tekst, van {from} tot {to}.")
}

fn court_year_title(court: &str, year: i32, page: u32) -> String {
    if page > 1 {
        format!("{court}: beslissingen {year} (pagina {page}) - LibreJustice")
    } else {
        format!("{court}: beslissingen {year} - LibreJustice")
    }
}

fn court_year_description(count: &str, year: i32, court: &str) -> String {
    format!(
        "De {count} beslissingen die \"{court}\" in {year} heeft gewezen, in hun volledige tekst."
    )
}

fn court_year_heading(court: &str, year: i32) -> String {
    format!("{court} · beslissingen {year}")
}

fn court_year_intro(count: &str, year: i32) -> String {
    format!("{count} beslissingen gewezen in {year}, van de recentste tot de oudste.")
}

fn search_year_decisions(year: i32) -> String {
    format!("Zoeken in de beslissingen van {year}")
}
