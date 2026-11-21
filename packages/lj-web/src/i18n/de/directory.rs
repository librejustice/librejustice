use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::{DirectoryUi, KindCopy};

pub const DIRECTORY: DirectoryUi = DirectoryUi {
    title: "Verzeichnis der Rechtseinheiten - LibreJustice",
    description: "Finden Sie die Entscheide, in denen ein Unternehmen, eine öffentliche Körperschaft, ein Verein, eine Anwältin, ein Anwalt oder eine Kanzlei vorkommt.",
    directory: "Verzeichnis",
    heading: "Rechtseinheiten aus den Registern",
    intro: "Die Unternehmen, öffentlichen Körperschaften, Vereine, Anwältinnen und Anwälte sowie Kanzleien aus den französischen Registern.",
    search_placeholder: "Unternehmen, Verein, Anwältin oder Anwalt suchen …",
    search_label: "Rechtseinheit suchen",
    search: "Suchen",
    categories: "Kategorien",
    categories_label: "Kategorien des Verzeichnisses",
    results: "Treffer",
    all: "Alle",
    in_register: "im Register",
    linked_to_decisions,
    min_query: "Geben Sie mindestens zwei Zeichen ein, um die Suche zu starten.",
    min_name: "Geben Sie mindestens zwei Zeichen ein.",
    no_match: "Keine Rechtseinheit entspricht dieser Suche.",
    search_unavailable,
    listing_unavailable,
    empty_category: "Noch keine Rechtseinheit in dieser Kategorie.",
    register_count,
    in_court,
    sorted_by_decisions: "nach Anzahl Entscheide sortiert",
    decision_count,
    ceased: "Erloschen",
    pagination: "Seitennavigation",
    previous: "Zurück",
    next: "Weiter",
    page_of,
    listing_title,
    listing_description,
    named_title,
    named_description,
    named_placeholder,
    named_label,
    named_empty,
    sign_in: "Melden Sie sich an",
    sign_in_to_search: ", um diese Suche zu starten.",
    unknown_title: "Kategorie nicht gefunden - LibreJustice",
    unknown_heading: "Diese Kategorie gibt es im Verzeichnis nicht",
    back: "Zurück zum Verzeichnis",
    companies: KindCopy {
        plural: "Unternehmen",
        tagline: "Gesellschaften und Einzelunternehmen, identifiziert über ihre SIREN-Nummer.",
        plural_lower: "Unternehmen",
        singular_lower: "Unternehmen",
        linked: "verknüpft",
    },
    public_bodies: KindCopy {
        plural: "Öffentliche Körperschaften",
        tagline: "Staat, Gebietskörperschaften und öffentlich-rechtliche Anstalten.",
        plural_lower: "öffentliche Körperschaften",
        singular_lower: "öffentliche Körperschaft",
        linked: "verknüpft",
    },
    associations: KindCopy {
        plural: "Vereine",
        tagline: "Eingetragene Vereine, identifiziert über ihre RNA-Nummer.",
        plural_lower: "Vereine",
        singular_lower: "Verein",
        linked: "verknüpft",
    },
    lawyers: KindCopy {
        plural: "Anwältinnen und Anwälte",
        tagline: "Anwältinnen und Anwälte sowie Avocats aux Conseils, nach Namen gesucht.",
        plural_lower: "Anwältinnen und Anwälte",
        singular_lower: "Anwältin oder Anwalt",
        linked: "verknüpft",
    },
    firms: KindCopy {
        plural: "Anwaltskanzleien",
        tagline: "Organisationsformen der Anwaltschaft: SCP, SELARL, AARPI …",
        plural_lower: "Anwaltskanzleien",
        singular_lower: "Anwaltskanzlei",
        linked: "verknüpft",
    },
};

fn linked_to_decisions(count: &str, linked: &str) -> String {
    format!("davon {count} mit Gerichtsentscheiden {linked}")
}

fn search_unavailable(message: &str) -> String {
    format!("Suche nicht verfügbar ({message}).")
}

fn listing_unavailable(message: &str) -> String {
    format!("Liste nicht verfügbar ({message}).")
}

fn register_count(n: i64) -> String {
    match n {
        1 => "1 Rechtseinheit im Register".to_string(),
        n => format!(
            "{} Rechtseinheiten im Register",
            group_thousands(&NUMBERS, n)
        ),
    }
}

fn in_court(n: i64) -> String {
    format!("davon {} vor Gericht", group_thousands(&NUMBERS, n))
}

fn decision_count(n: i64) -> String {
    match n {
        1 => "1 Entscheid".to_string(),
        n => format!("{} Entscheide", group_thousands(&NUMBERS, n)),
    }
}

fn page_of(page: i64, total: i64) -> String {
    format!("Seite {page} von {total}")
}

fn listing_title(plural: &str) -> String {
    format!("Verzeichnis: {plural} - LibreJustice")
}

fn listing_description(plural: &str, tagline: &str) -> String {
    format!("{plural} im Register, nach Anzahl Gerichtsverfahren in der französischen Rechtsprechung sortiert. {tagline}")
}

fn named_title(singular: &str) -> String {
    format!("{singular} suchen - LibreJustice")
}

fn named_description(singular: &str, tagline: &str) -> String {
    format!("{singular} nach Namen in der französischen Rechtsprechung suchen. {tagline}")
}

fn named_placeholder(singular: &str) -> String {
    format!("{singular}: Name …")
}

fn named_label(singular: &str) -> String {
    format!("{singular} nach Namen suchen")
}

fn named_empty(singular: &str) -> String {
    format!("{singular}: keine Treffer für diese Suche.")
}
