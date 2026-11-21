use crate::i18n::NoticesUi;

pub const NOTICES: NoticesUi = NoticesUi {
    unavailable,
    sort_deadline: "Échéance la plus proche",
    sort_seen: "Vues en dernier",
    sort_published: "Parution la plus récente",
    open_notices: "Annonces publiées, encore ouvertes",
    sort_by: "Trier par",
    subscribe: "S'abonner à ces filtres",
    rss: "Flux RSS des nouveautés",
    ics: "Agenda des échéances (ICS)",
    no_account: "Sans compte : l'adresse est l'abonnement.",
    closed_included: "Consultations closes comprises",
    none_open: "Aucune annonce ouverte pour ces filtres.",
    due_before,
    published_on,
    filter: "Filtrer…",
    filter_zone: "Filtrer (ex. Guyane…)",
    filter_cpv: "Filtrer (ex. travaux…)",
    filter_trade: "Filtrer (ex. menuiserie…)",
    see_all,
    channels,
    title: "Journaux - LibreJustice",
    title_for,
    heading: "Journaux",
    by_department: "Par département",
    public_markets: "Marchés publics",
    index_description,
    index_title: "Marchés publics par département - LibreJustice",
    index_heading: "Marchés publics par département",
    index_intro,
    search_all: "Chercher dans toutes les annonces",
    departments: "Départements",
    by_nature: "Par nature",
    department_empty: "Aucune consultation ouverte dans ce département",
    department_title,
    department_description,
    department_heading,
    department_intro,
    filter_by_trade: "Filtrer par domaine ou métier",
    follow_department: "Suivre ce département",
    follow_text: "Deux flux, sans compte ni inscription : les nouveautés dans votre lecteur, les échéances dans votre agenda.",
    calendar: "Calendrier des échéances (ICS)",
    merged: "Cette annonce a été fusionnée : voir la fiche à jour",
    description_awarded,
    description_open,
    description_other,
    closed: "Consultation clôturée : l'échéance est passée, ou l'avis a disparu des plateformes.",
    fact_due: "Remise des offres",
    not_published: "non publiée",
    fact_published: "Parution",
    fact_nature: "Nature",
    fact_cpv: "Catégorie (CPV)",
    fact_cpv_inferred: "Catégorie (CPV), déduite du titre",
    fact_trade: "Métier",
    fact_trade_inferred: "Métier, déduit du titre",
    fact_reference: "Référence acheteur",
    fact_procedure: "Procédure",
    fact_place: "Lieu d'exécution",
    whole_country: "France entière",
    department_count,
    fact_estimate: "Valeur estimée",
    fact_seen: "Relevé",
    see_notice: "Consulter l'avis",
    file_bid: "Déposer, consulter le dossier",
    actors: "Acteurs",
    boamp_notice: "Avis au BOAMP",
    joue_notice: "Avis au JOUE",
    platform_page: "Page de la consultation sur la plateforme",
    lots,
    lot_n,
    criteria: "Critères d'attribution",
    award_notice: "Avis de résultat",
    award: "Attribution",
    offers_received,
    awarded_on,
    previous: "Précédent",
    next: "Suivant",
    page_of,
};

fn unavailable(error: &str) -> String {
    format!("Recherche indisponible ({error}).")
}

fn due_before(day: &str) -> String {
    format!("Remise avant le {day}")
}

fn published_on(day: &str) -> String {
    format!("Publié le {day}")
}

fn see_all(total: i64) -> String {
    format!(
        "{} · tout voir",
        crate::helpers::group_thousands(&super::numbers::NUMBERS, total)
    )
}

fn channels(n: usize) -> String {
    format!("{n} canaux")
}

fn page_of(page: u32, pages: u32) -> String {
    format!("Page {page} sur {pages}")
}

fn title_for(query: &str) -> String {
    format!("{query} - Journaux - LibreJustice")
}

fn index_description(total: &str) -> String {
    format!(
        "Les {total} consultations de marchés publics ouvertes en France, \
         département par département : objet, acheteur et date limite de remise des offres."
    )
}

fn index_intro(total: &str) -> String {
    format!(
        "{total} consultations ouvertes, relevées sur les profils d'acheteurs et le BOAMP. \
         Chaque département mène à ses avis en cours, du plus proche au plus lointain."
    )
}

fn department_title(name: &str, page: u32) -> String {
    if page > 1 {
        format!("Marchés publics : {name} (page {page}) - LibreJustice")
    } else {
        format!("Marchés publics en cours : {name} - LibreJustice")
    }
}

fn department_description(total: &str, name: &str) -> String {
    format!(
        "Les {total} consultations de marchés publics ouvertes en {name} : objet, acheteur, \
         date limite de remise des offres et lien vers le dossier."
    )
}

fn department_heading(name: &str) -> String {
    format!("Marchés publics : {name}")
}

fn department_intro(total: &str) -> String {
    format!("{total} consultations ouvertes, de la plus proche échéance à la plus lointaine.")
}

fn description_awarded(buyer: &str) -> String {
    format!("Marché attribué, {buyer} : titulaires, montants et nombre d'offres reçues.")
}

fn description_open(department: &str, buyer: &str, day: &str) -> String {
    format!(
        "Marché public de {department} : {buyer}, remise des offres jusqu'au {day}, dossier et avis en lien direct."
    )
}

fn description_other(buyer: &str) -> String {
    format!("Marché public, {buyer} : objet, référence, avis et dossier de consultation.")
}

fn plural(n: i64) -> &'static str {
    if n > 1 {
        "s"
    } else {
        ""
    }
}

fn lots(n: usize) -> String {
    format!("Allotissement · {n} lot{}", plural(n as i64))
}

fn lot_n(n: usize) -> String {
    format!("Lot {n}")
}

fn offers_received(n: i64) -> String {
    format!("{n} offre{} reçue{}", plural(n), plural(n))
}

fn awarded_on(day: &str) -> String {
    format!("attribué le {day}")
}

fn department_count(count: usize) -> String {
    format!("{count} départements")
}
