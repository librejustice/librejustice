use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::{DirectoryUi, KindCopy};

pub const DIRECTORY: DirectoryUi = DirectoryUi {
    title: "Annuaire des entités - LibreJustice",
    description: "Retrouvez les décisions où apparaît une entreprise, une personne publique, une association, un avocat ou un cabinet.",
    directory: "Annuaire",
    heading: "Entités des registres",
    intro: "Les entreprises, personnes publiques, associations, avocats et cabinets des registres français.",
    search_placeholder: "Rechercher une entreprise, une association, un avocat…",
    search_label: "Rechercher une entité",
    search: "Rechercher",
    categories: "Catégories",
    categories_label: "Catégories de l'annuaire",
    results: "Résultats",
    all: "Toutes",
    in_register: "au registre",
    linked_to_decisions,
    min_query: "Précisez au moins deux caractères pour lancer la recherche.",
    min_name: "Saisissez au moins deux caractères.",
    no_match: "Aucune entité ne correspond à cette recherche.",
    search_unavailable,
    listing_unavailable,
    empty_category: "Aucune entité dans cette catégorie pour l'instant.",
    register_count,
    in_court,
    sorted_by_decisions: "classées par nombre de décisions",
    decision_count,
    ceased: "Cessée",
    pagination: "Pagination",
    previous: "Précédent",
    next: "Suivant",
    page_of,
    listing_title,
    listing_description,
    named_title,
    named_description,
    named_placeholder,
    named_label,
    named_empty,
    sign_in: "Connectez-vous",
    sign_in_to_search: " pour lancer cette recherche.",
    unknown_title: "Catégorie introuvable - LibreJustice",
    unknown_heading: "Cette catégorie d'annuaire n'existe pas",
    back: "Retour à l'annuaire",
    companies: KindCopy {
        plural: "Entreprises",
        tagline: "Sociétés et entrepreneurs identifiés par leur SIREN.",
        plural_lower: "entreprises",
        singular_lower: "entreprise",
        linked: "liées",
    },
    public_bodies: KindCopy {
        plural: "Personnes publiques",
        tagline: "État, collectivités et établissements publics.",
        plural_lower: "personnes publiques",
        singular_lower: "personne publique",
        linked: "liées",
    },
    associations: KindCopy {
        plural: "Associations",
        tagline: "Associations déclarées identifiées par leur RNA.",
        plural_lower: "associations",
        singular_lower: "association",
        linked: "liées",
    },
    lawyers: KindCopy {
        plural: "Avocats",
        tagline: "Avocats et avocats aux Conseils, cherchés par leur nom.",
        plural_lower: "avocats",
        singular_lower: "avocat",
        linked: "liés",
    },
    firms: KindCopy {
        plural: "Cabinets d'avocats",
        tagline: "Structures d'exercice des avocats : SCP, SELARL, AARPI…",
        plural_lower: "cabinets d'avocats",
        singular_lower: "cabinet d'avocats",
        linked: "liés",
    },
};

fn linked_to_decisions(count: &str, linked: &str) -> String {
    format!("dont {count} {linked} à des décisions de justice")
}

fn search_unavailable(message: &str) -> String {
    format!("Recherche indisponible ({message}).")
}

fn listing_unavailable(message: &str) -> String {
    format!("Listing indisponible ({message}).")
}

fn register_count(n: i64) -> String {
    match n {
        1 => "1 entité au registre".to_string(),
        n => format!("{} entités au registre", group_thousands(&NUMBERS, n)),
    }
}

fn in_court(n: i64) -> String {
    format!("dont {} en justice", group_thousands(&NUMBERS, n))
}

fn decision_count(n: i64) -> String {
    match n {
        1 => "1 déc.".to_string(),
        n => format!("{} déc.", group_thousands(&NUMBERS, n)),
    }
}

fn page_of(page: i64, total: i64) -> String {
    format!("Page {page} sur {total}")
}

fn listing_title(plural: &str) -> String {
    format!("Annuaire des {plural} - LibreJustice")
}

fn listing_description(plural: &str, tagline: &str) -> String {
    format!("{plural} au registre, classés par volume de contentieux dans la jurisprudence française. {tagline}")
}

fn named_title(singular: &str) -> String {
    format!("Rechercher un {singular} - LibreJustice")
}

fn named_description(singular: &str, tagline: &str) -> String {
    format!("Recherchez un {singular} par son nom dans la jurisprudence française. {tagline}")
}

fn named_placeholder(singular: &str) -> String {
    format!("Nom de l'{singular}…")
}

fn named_label(singular: &str) -> String {
    format!("Rechercher un {singular} par son nom")
}

fn named_empty(singular: &str) -> String {
    format!("Aucun {singular} ne correspond à cette recherche.")
}
