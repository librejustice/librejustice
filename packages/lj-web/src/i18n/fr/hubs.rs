use crate::i18n::HubsUi;

pub const HUBS: HubsUi = HubsUi {
    unknown_jurisdiction: "Juridiction inconnue",
    invalid_year: "Année invalide",
    undated: "Sans date",
    texts_and_codes: "Textes & codes",
    search_these: "Rechercher dans ces textes",
    n_texts,
    catalogue_title: "Catalogue des normes - LibreJustice",
    catalogue_title_for,
    catalogue_description: "Tous les textes du corpus normatif (codes, lois et ordonnances, décrets, arrêtés, conventions collectives, textes européens, traités, circulaires, BOFiP), par fond puis par année.",
    catalogue_heading: "Catalogue des normes",
    catalogue_intro: "Parcourez les textes du corpus par fond, puis par année. Chaque fond ouvre la liste complète de ses textes.",
    other_jurisdictions: "Autres juridictions",
    in_force_lead: "Ce catalogue liste tous les textes du fonds. Pour ne garder que ceux qui s'appliquent en un territoire, à une date donnée : ",
    in_force_link: "ce qui s'applique en un lieu",
    fond_title,
    fond_description,
    fond_intro,
    by_year: "Textes par année",
    year_title,
    year_description,
    year_heading,
    year_intro,
    search_year,
    codes_title: "Codes : textes consolidés - LibreJustice",
    codes_description: "Catalogue des codes et constitutions consolidés, un groupe par ordre juridique, de la France à chaque État ou territoire. Versions à date, articles liés.",
    codes: "Codes",
    constitutions: "Constitutions",
    codes_count,
    n_articles,
    corpus_title,
    corpus_description,
    applicability: "Applicabilité",
    corpus_heading,
    corpus_intro,
    corpus_empty: "Aucun texte compilé ne nomme ce lieu à cette date.",
    and_more,
    on: "Au",
    not_found_title: "Page introuvable - LibreJustice",
    pagination: "Pagination",
    first: "Première",
    previous: "Précédente",
    next: "Suivante",
    last: "Dernière",
    page_of,
    jurisdictions_title: "Juridictions - LibreJustice",
    jurisdictions_description: "Toutes les juridictions du corpus (Conseil d'État, Cour de cassation, cours d'appel, tribunaux administratifs et judiciaires, cours européennes), avec leurs décisions par année.",
    case_law: "Jurisprudence",
    jurisdictions: "Juridictions",
    jurisdictions_intro: "Parcourez les décisions du corpus par juridiction, puis par année. Chaque juridiction ouvre la liste complète de ses décisions.",
    search_decisions: "Rechercher dans ces décisions",
    n_decisions,
    court_title,
    court_description,
    court_intro,
    decisions_by_year: "Décisions par année",
    court_year_title,
    court_year_description,
    court_year_heading,
    court_year_intro,
    search_year_decisions,
};

fn n_texts(count: &str, n: i64) -> String {
    format!("{count} texte{}", if n > 1 { "s" } else { "" })
}

fn catalogue_title_for(jurisdiction: &str) -> String {
    format!("Catalogue des normes ({jurisdiction}) - LibreJustice")
}

fn fond_title(fond: &str) -> String {
    format!("{fond} : textes - LibreJustice")
}

fn fond_description(count: &str, fond: &str) -> String {
    format!("Les {count} textes du fond « {fond} », par année.")
}

fn fond_intro(count: &str, span: Option<(i32, i32)>) -> String {
    match span {
        Some((min, max)) => format!("{count} textes, de {min} à {max}."),
        None => format!("{count} textes."),
    }
}

fn year_title(fond: &str, year: &str, page: u32) -> String {
    if page > 1 {
        format!("{fond} : textes {year} (page {page}) - LibreJustice")
    } else {
        format!("{fond} : textes {year} - LibreJustice")
    }
}

fn year_description(count: &str, year: Option<i32>, fond: &str) -> String {
    match year {
        Some(y) => format!("Les {count} textes de {y} du fond « {fond} », en texte intégral."),
        None => format!("Les {count} textes sans date du fond « {fond} »."),
    }
}

fn year_heading(fond: &str, year: &str) -> String {
    format!("{fond} · {year}")
}

fn year_intro(count: &str, year: Option<i32>) -> String {
    match year {
        Some(_) => format!("{count} textes, du plus récent au plus ancien."),
        None => format!("{count} textes sans date de signature ni de publication connue."),
    }
}

fn search_year(year: i32) -> String {
    format!("Rechercher dans les textes de {year}")
}

fn codes_count(n: usize) -> String {
    format!("{n} textes.")
}

fn n_articles(n: i64) -> String {
    format!("{n} article{}", if n > 1 { "s" } else { "" })
}

fn corpus_title(place: &str) -> String {
    format!("Ce qui s'applique {place} - LibreJustice")
}

fn corpus_description(place: &str, day: &str) -> String {
    format!("Les textes applicables {place} au {day}, par journal officiel puis par nature.")
}

fn corpus_heading(place: &str) -> String {
    format!("Ce qui s'applique {place}")
}

fn corpus_intro(day: &str, count: &str) -> String {
    format!(
        "Au {day}, {count} s'y appliquent. Un texte compte dès qu'un de ses articles s'applique. \
         Un groupe trop long n'affiche que quelques titres."
    )
}

fn and_more(count: &str) -> String {
    format!("et {count} autres")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Page {page} / {pages}")
}

fn n_decisions(count: &str, n: i64) -> String {
    format!("{count} décision{}", if n > 1 { "s" } else { "" })
}

fn court_title(court: &str) -> String {
    format!("{court} : décisions - LibreJustice")
}

fn court_description(count: &str, court: &str) -> String {
    format!("Les {count} décisions de la juridiction « {court} », par année, en texte intégral.")
}

fn court_intro(count: &str, from: i32, to: i32) -> String {
    format!("{count} décisions en texte intégral, de {from} à {to}.")
}

fn court_year_title(court: &str, year: i32, page: u32) -> String {
    if page > 1 {
        format!("{court} : décisions {year} (page {page}) - LibreJustice")
    } else {
        format!("{court} : décisions {year} - LibreJustice")
    }
}

fn court_year_description(count: &str, year: i32, court: &str) -> String {
    format!("Les {count} décisions rendues en {year} par « {court} », en texte intégral.")
}

fn court_year_heading(court: &str, year: i32) -> String {
    format!("{court} · décisions {year}")
}

fn court_year_intro(count: &str, year: i32) -> String {
    format!("{count} décisions rendues en {year}, de la plus récente à la plus ancienne.")
}

fn search_year_decisions(year: i32) -> String {
    format!("Rechercher dans les décisions de {year}")
}
