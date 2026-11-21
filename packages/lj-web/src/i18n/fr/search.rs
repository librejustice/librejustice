use crate::i18n::SearchUi;

pub const SEARCH: SearchUi = SearchUi {
    search: "Rechercher",
    clear_search: "Effacer la recherche",
    mode_group: "Mode de recherche",
    mode_auto: "Auto",
    mode_auto_help: "Choisit le mode le plus adapté à votre recherche.",
    mode_lexical: "Lexicale",
    mode_lexical_help: "Recherche en utilisant des mots-clés. Mettez une expression entre guillemets ou combinez des termes avec ET, OU, SAUF.",
    mode_semantic: "Reformulation",
    mode_semantic_help: "Recherche aussi les synonymes et les formulations proches de votre question.",
    mode_reference: "Référence",
    mode_reference_decision_help: "Pour retrouver une décision à partir de son numéro, de son nom d'usage ou de sa juridiction et sa date.",
    mode_reference_article_help: "Pour retrouver un article à partir de son code et de son numéro.",
    auto_chose_this,
    sort_label: "Trier les résultats",
    sort_relevance: "Pertinence",
    sort_newest: "Récents",
    sort_oldest: "Anciens",
    previous_page: "Page précédente",
    next_page: "Page suivante",
    page_n,
    card_content: "Contenu des cartes",
    excerpt: "Extrait",
    summary: "Résumé",
    ai_sort: "Tri IA",
    ai_sort_by_date: "Le tri IA ne s'applique pas à un classement par date, qui impose déjà son ordre.",
    ai_sort_other_mode: "Le tri IA ne s'applique qu'au mode Reformulation : une recherche à opérateurs classe par correspondance aux termes saisis, une recherche par référence par l'identité de la décision.",
    ai_sort_declined: "Le tri IA n'a pas trié cette liste : trop peu de résultats, ou le service était saturé. Le classement affiché est celui du score de recherche.",
    ai_sort_on: "Une IA a trié les 50 premiers résultats selon votre question. Cliquer pour revenir au classement par score de recherche.",
    ai_sort_off: "Faire trier les 50 premiers résultats par une IA, selon votre question.",
    query_too_long,
    no_results: "Aucun résultat",
    no_decision_for_filters: "Aucune décision ne répond à ces filtres",
    remove_a_filter: "Retirez un filtre, ou élargissez la période.",
    nothing_found_for,
    rephrase_before: "Essayez de reformuler avec moins de termes, des synonymes ou des opérateurs booléens (",
    op_and: "ET",
    op_or: "OU",
    op_not: "SAUF",
    rephrase_after: "). Le mode Reformulation fonctionne mieux avec des formulations naturelles.",
    filters: "Filtres de recherche",
    filter_date: "Date",
    filter_jurisdiction: "Juridiction",
    filter_jurisdiction_placeholder: "Filtrer (ex. Lyon…)",
    filter_publication: "Publication",
    filter_solution: "Solution",
    filter_search_in: "Chercher dans",
    filter_domain: "Matière",
    filter_cited_texts: "Textes cités",
    filter_cited_texts_placeholder: "Filtrer ou chercher un article…",
    filter_formation: "Formation",
    fewer_filters: "Moins de filtres",
    more_filters: "Plus de filtres",
    part_headnote: "Sommaire",
    part_headnote_hint: "Le sommaire ou l'analyse publiés par la juridiction.",
    part_judgment: "Décision",
    part_annexes: "Moyens annexés",
    part_conclusions: "Conclusions",
    part_conclusions_hint: "Les conclusions de l'avocat général ou du rapporteur public.",
    chip_article,
    chip_search_in,
    chip_dates_between,
    chip_dates_from,
    chip_dates_to,
    remove_filter: "Retirer ce filtre",
    clear_all: "Tout effacer",
    collapse: "Réduire",
    expand: "Développer",
    show_less: "Voir moins",
    show_more: "Voir plus",
    help: "Aide à la recherche",
    close: "Fermer",
    help_modes: "Modes de recherche",
    help_find_one: "Trouver une décision précise",
    help_designations: &[
        ("19-22.015", "numéro de pourvoi ou de requête"),
        ("1922015", "le même, sans séparateurs"),
        ("ECLI:FR:CCASS:2022:CO00288", "ECLI"),
        ("CE 20 octobre 1989", "cour et date"),
        ("arrêt Nicolo", "nom d'usage"),
    ],
    help_ai_sort_text: "Actif par défaut en mode Reformulation : une IA réordonne les 50 premiers résultats selon votre question.",
    help_operators_title: "Opérateurs booléens",
    help_operators: &[
        ("ET", "intersection"),
        ("OU", "union"),
        ("SAUF", "exclusion"),
        ("PROCHE5", "proximité de 5 mots"),
        ("\"…\"", "expression exacte"),
        ("*", "troncature"),
    ],
    help_how: "Comment chercher",
    help_notion: "Une notion",
    help_notion_text: "Mots-clés libres : les articles en vigueur les plus pertinents remontent en premier (« congés trimestriels convention collective de 1966 »).",
    help_named_text: "Un texte nommé",
    help_named_text_text: "Nommer le code, la convention ou le pays cible ses articles : « préavis de démission code du travail », « code civil du Sénégal ».",
    help_article_number: "Un n° d'article",
    help_article_number_text: "« L1237-1 » remonte directement l'article et sa page versionnée.",
    help_refine: "Affiner",
    help_refine_text: "Les filtres sous la barre bornent les résultats par juridiction, nature (code, loi, convention collective…), portée et source. Depuis la page d'un code, « Rechercher dans ce code » limite la recherche à ce texte.",
    rail_years: "Années",
    rail_articles: "Articles les plus cités",
    rail_courts: "Juridictions",
    rail_solution: "Solution",
    in_texts: "Dans les textes",
    in_decisions: "Dans les décisions",
    texts_stats,
    decisions_stats,
    search_texts: "Rechercher dans les textes",
    search_decisions: "Rechercher dans les décisions",
    see_texts: "Voir les textes",
    see_decisions: "Voir les décisions",
    decisions_title: "Décisions - LibreJustice",
    counting: "Décompte en cours",
    loading_corpus: "Chargement du fonds…",
    searching: "Recherche en cours…",
    looking_for_this_one: "Cherchez-vous cette décision ?",
    looking_for_these: "Cherchez-vous ces décisions ?",
    other_decisions_match: "D'autres décisions répondent à cette référence",
    also_docket: "aussi n° ",
    open_decision: "Consulter la décision",
    pagination: "Pagination",
    texts_by_last_change: "Textes, du dernier modifié au plus ancien",
    maybe_looking_for: "Vous cherchez peut-être",
    article_of,
    other_articles_match: "D'autres articles portent cette référence",
    rail_scope: "Portée",
    rail_law: "Droit",
    rail_sources: "Sources",
    filter_source: "Source",
    n_places,
    back_to_metropolitan_france: "Revenir à la France métropolitaine",
    back_to_default_place,
    chip_in_text,
    chip_in_force_at,
    no_text_for_filters: "Aucun texte pour ces filtres.",
    no_article_for,
    page_title: "Recherche",
    texts_page_title: "Recherche de textes",
    texts_title: "Textes - LibreJustice",
    texts_title_for,
    results_summary: "Synthèse des résultats",
    see_also: "Voir aussi",
    results: "Résultats",
    search_results: "Résultats de recherche",
    pending: "Recherche…",
    no_value: "Aucune valeur",
    in_force: "Articles et textes en vigueur",
    automatic: "Automatique",
    catalogue_searching: "Recherche au catalogue…",
    suggestions: "Suggestions",
    journal_of,
    placeholder_decisions: "Rechercher dans la jurisprudence…",
    placeholder_texts: "Rechercher dans les codes et lois…",
    placeholder_notices: "Objet ou acteur : « voirie », « redressement »…",
    rate_limited: "Trop de recherches en peu de temps.",
    rate_limited_hint: "Réessayez dans une minute.",
    too_long,
    too_long_hint: "Gardez la question, retirez le texte collé autour : le moteur cherche sur le sens des mots, pas sur la longueur.",
    unknown_filter: "Un filtre de cette recherche n'est pas reconnu.",
    unknown_filter_hint: "Le lien porte sans doute une valeur périmée : retirez le filtre pour relancer la recherche.",
    down: "La recherche n'a pas abouti.",
    down_hint: "L'API est peut-être indisponible. Réessayez dans un instant.",
};

fn auto_chose_this(help: &str) -> String {
    format!("{help} C'est ce que le mode Auto a retenu ici.")
}

fn page_n(page: u32) -> String {
    format!("Page {page}")
}

fn query_too_long(n: usize, max: usize) -> String {
    format!("{n} caractères saisis, le maximum est {max} : la recherche portera sur les {max} premiers. Gardez la question, retirez le texte collé autour.")
}

fn nothing_found_for(query: &str) -> String {
    format!("Rien trouvé pour «\u{a0}{query}\u{a0}»")
}

fn chip_article(text: &str, article: &str) -> String {
    format!("{text} · art. {article}")
}

fn chip_search_in(parts: &str) -> String {
    format!("Dans : {parts}")
}

fn chip_dates_between(from: &str, to: &str) -> String {
    format!("Du {from} au {to}")
}

fn chip_dates_from(from: &str) -> String {
    format!("Depuis le {from}")
}

fn chip_dates_to(to: &str) -> String {
    format!("Jusqu'au {to}")
}

fn texts_stats(texts: &str, articles: &str) -> String {
    format!("{texts} textes · {articles} articles en vigueur")
}

fn decisions_stats(decisions: &str) -> String {
    format!("{decisions} décisions, deux ordres de juridiction")
}

fn article_of(text: &str, num: &str) -> String {
    format!("{text}, article {num}")
}

fn n_places(n: usize) -> String {
    format!("{n} lieux")
}

fn back_to_default_place(place: &str) -> String {
    format!("Revenir au lieu par défaut ({place})")
}

fn chip_in_text(title: &str) -> String {
    format!("Dans\u{a0}: {title}")
}

fn chip_in_force_at(day: &str) -> String {
    format!("En vigueur au {day}")
}

fn no_article_for(query: &str) -> String {
    format!("Aucun article pour « {query} ».")
}

fn texts_title_for(query: &str) -> String {
    format!("{query} - Textes - LibreJustice")
}

fn journal_of(day: &str) -> String {
    format!("JO du {day}")
}

fn too_long(chars: usize, max: usize) -> String {
    format!("Cette recherche fait {chars} caractères, le maximum est {max}.")
}
