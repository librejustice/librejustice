use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::{DirectoryUi, KindCopy};

pub const DIRECTORY: DirectoryUi = DirectoryUi {
    title: "Gids van de entiteiten - LibreJustice",
    description: "Vind de beslissingen terug waarin een onderneming, een overheid, een vereniging, een advocaat of een advocatenkantoor voorkomt.",
    directory: "Gids",
    heading: "Entiteiten uit de registers",
    intro: "De ondernemingen, overheden, verenigingen, advocaten en advocatenkantoren uit de Franse registers.",
    search_placeholder: "Een onderneming, een vereniging, een advocaat zoeken…",
    search_label: "Een entiteit zoeken",
    search: "Zoeken",
    categories: "Categorieën",
    categories_label: "Categorieën van de gids",
    results: "Resultaten",
    all: "Alle",
    in_register: "in het register",
    linked_to_decisions,
    min_query: "Geef minstens twee tekens in om te zoeken.",
    min_name: "Geef minstens twee tekens in.",
    no_match: "Geen enkele entiteit komt overeen met deze zoekopdracht.",
    search_unavailable,
    listing_unavailable,
    empty_category: "Voorlopig geen entiteit in deze categorie.",
    register_count,
    in_court,
    sorted_by_decisions: "gerangschikt naar aantal beslissingen",
    decision_count,
    ceased: "Stopgezet",
    pagination: "Paginering",
    previous: "Vorige",
    next: "Volgende",
    page_of,
    listing_title,
    listing_description,
    named_title,
    named_description,
    named_placeholder,
    named_label,
    named_empty,
    sign_in: "Log in",
    sign_in_to_search: " om deze zoekopdracht uit te voeren.",
    unknown_title: "Categorie niet gevonden - LibreJustice",
    unknown_heading: "Deze categorie van de gids bestaat niet",
    back: "Terug naar de gids",
    companies: KindCopy {
        plural: "Ondernemingen",
        tagline: "Vennootschappen en ondernemers, geïdentificeerd door hun SIREN-nummer.",
        plural_lower: "ondernemingen",
        singular_lower: "onderneming",
        linked: "gekoppeld",
    },
    public_bodies: KindCopy {
        plural: "Overheden",
        tagline: "Staat, lokale besturen en openbare instellingen.",
        plural_lower: "overheden",
        singular_lower: "overheid",
        linked: "gekoppeld",
    },
    associations: KindCopy {
        plural: "Verenigingen",
        tagline: "Geregistreerde verenigingen, geïdentificeerd door hun RNA-nummer.",
        plural_lower: "verenigingen",
        singular_lower: "vereniging",
        linked: "gekoppeld",
    },
    lawyers: KindCopy {
        plural: "Advocaten",
        tagline: "Advocaten en cassatieadvocaten, gezocht op hun naam.",
        plural_lower: "advocaten",
        singular_lower: "advocaat",
        linked: "gekoppeld",
    },
    firms: KindCopy {
        plural: "Advocatenkantoren",
        tagline: "Structuren waarin advocaten hun beroep uitoefenen: SCP, SELARL, AARPI…",
        plural_lower: "advocatenkantoren",
        singular_lower: "advocatenkantoor",
        linked: "gekoppeld",
    },
};

fn linked_to_decisions(count: &str, linked: &str) -> String {
    format!("waarvan {count} {linked} aan rechterlijke beslissingen")
}

fn search_unavailable(message: &str) -> String {
    format!("Zoeken niet beschikbaar ({message}).")
}

fn listing_unavailable(message: &str) -> String {
    format!("Lijst niet beschikbaar ({message}).")
}

fn register_count(n: i64) -> String {
    match n {
        1 => "1 entiteit in het register".to_string(),
        n => format!(
            "{} entiteiten in het register",
            group_thousands(&NUMBERS, n)
        ),
    }
}

fn in_court(n: i64) -> String {
    format!("waarvan {} in rechtszaken", group_thousands(&NUMBERS, n))
}

fn decision_count(n: i64) -> String {
    match n {
        1 => "1 beslissing".to_string(),
        n => format!("{} beslissingen", group_thousands(&NUMBERS, n)),
    }
}

fn page_of(page: i64, total: i64) -> String {
    format!("Pagina {page} van {total}")
}

fn listing_title(plural: &str) -> String {
    format!("Gids van de {plural} - LibreJustice")
}

fn listing_description(plural: &str, tagline: &str) -> String {
    format!("{plural} in het register, gerangschikt naar het aantal zaken in de Franse rechtspraak. {tagline}")
}

fn named_title(singular: &str) -> String {
    format!("Een {singular} zoeken - LibreJustice")
}

fn named_description(singular: &str, tagline: &str) -> String {
    format!("Zoek een {singular} op naam in de Franse rechtspraak. {tagline}")
}

fn named_placeholder(singular: &str) -> String {
    format!("Naam van de {singular}…")
}

fn named_label(singular: &str) -> String {
    format!("Een {singular} op naam zoeken")
}

fn named_empty(singular: &str) -> String {
    format!("Geen enkele {singular} komt overeen met deze zoekopdracht.")
}
