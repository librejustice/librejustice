use crate::i18n::SearchUi;

pub const SEARCH: SearchUi = SearchUi {
    search: "Zoeken",
    clear_search: "Zoekopdracht wissen",
    mode_group: "Zoekmodus",
    mode_auto: "Auto",
    mode_auto_help: "Kiest de modus die het best bij uw zoekopdracht past.",
    mode_lexical: "Trefwoorden",
    mode_lexical_help: "Zoekt op trefwoorden. Zet een woordgroep tussen aanhalingstekens of combineer termen met AND, OR, NOT.",
    mode_semantic: "Betekenis",
    mode_semantic_help: "Zoekt ook naar synoniemen en naar formuleringen die dicht bij uw vraag liggen.",
    mode_reference: "Referentie",
    mode_reference_decision_help: "Om een beslissing terug te vinden aan de hand van haar nummer of haar gangbare naam.",
    mode_reference_article_help: "Om een artikel terug te vinden aan de hand van zijn wetboek en zijn nummer.",
    auto_chose_this,
    sort_label: "Resultaten sorteren",
    sort_relevance: "Relevantie",
    sort_newest: "Nieuwste eerst",
    sort_oldest: "Oudste eerst",
    previous_page: "Vorige pagina",
    next_page: "Volgende pagina",
    page_n,
    card_content: "Inhoud van de kaarten",
    excerpt: "Fragment",
    summary: "Samenvatting",
    ai_sort: "AI-sortering",
    ai_sort_by_date: "AI-sortering geldt niet voor een rangschikking op datum, die haar volgorde al oplegt.",
    ai_sort_other_mode: "AI-sortering geldt alleen in de modus Betekenis: een zoekopdracht met operatoren rangschikt op overeenstemming met de ingegeven termen, een zoekopdracht op referentie op de identiteit van de beslissing.",
    ai_sort_declined: "De AI heeft deze lijst niet gesorteerd: te weinig resultaten, of de dienst was overbelast. De getoonde rangschikking is die van de zoekscore.",
    ai_sort_on: "Een AI heeft de eerste 50 resultaten gesorteerd volgens uw vraag. Klik om terug te gaan naar de rangschikking op zoekscore.",
    ai_sort_off: "De eerste 50 resultaten door een AI laten sorteren volgens uw vraag.",
    query_too_long,
    no_results: "Geen resultaten",
    no_decision_for_filters: "Geen enkele beslissing voldoet aan deze filters",
    remove_a_filter: "Verwijder een filter of verruim de periode.",
    nothing_found_for,
    rephrase_before: "Probeer te herformuleren met minder termen, met synoniemen of met booleaanse operatoren (",
    op_and: "AND",
    op_or: "OR",
    op_not: "NOT",
    rephrase_after: "). De modus Betekenis werkt het best met natuurlijke formuleringen.",
    filters: "Zoekfilters",
    filter_date: "Datum",
    filter_jurisdiction: "Rechtscollege",
    filter_jurisdiction_placeholder: "Filteren (bv. Gent…)",
    filter_publication: "Publicatie",
    filter_solution: "Uitkomst",
    filter_search_in: "Zoeken in",
    filter_domain: "Rechtsgebied",
    filter_cited_texts: "Aangehaalde teksten",
    filter_cited_texts_placeholder: "Filteren of een artikel zoeken…",
    filter_formation: "Samenstelling",
    fewer_filters: "Minder filters",
    more_filters: "Meer filters",
    part_headnote: "Samenvatting",
    part_headnote_hint: "De samenvatting of de analyse die het rechtscollege publiceert.",
    part_judgment: "Beslissing",
    part_annexes: "Bijgevoegde middelen",
    part_conclusions: "Conclusies",
    part_conclusions_hint: "De conclusie van de advocaat-generaal of van de rapporteur public.",
    chip_article,
    chip_search_in,
    chip_dates_between,
    chip_dates_from,
    chip_dates_to,
    remove_filter: "Deze filter verwijderen",
    clear_all: "Alles wissen",
    collapse: "Inklappen",
    expand: "Uitklappen",
    show_less: "Minder tonen",
    show_more: "Meer tonen",
    help: "Zoekhulp",
    close: "Sluiten",
    help_modes: "Zoekmodi",
    help_find_one: "Een bepaalde beslissing vinden",
    help_designations: &[
        ("265.228", "nummer van het arrest of zaaknummer"),
        ("265228", "hetzelfde, zonder scheidingstekens"),
        ("ECLI:FR:CCASS:2022:CO00288", "ECLI"),
        ("Van Gend en Loos", "gangbare naam"),
    ],
    help_ai_sort_text: "Standaard actief in de modus Betekenis: een AI rangschikt de eerste 50 resultaten opnieuw volgens uw vraag.",
    help_operators_title: "Booleaanse operatoren",
    help_operators: &[
        ("AND", "beide termen"),
        ("OR", "een van beide termen"),
        ("NOT", "sluit een term uit"),
        ("NEAR5", "binnen 5 woorden van elkaar"),
        ("\"…\"", "exacte woordgroep"),
        ("*", "woorden die zo beginnen"),
    ],
    help_how: "Hoe zoeken",
    help_notion: "Een begrip",
    help_notion_text: "Vrije zoektermen: de meest relevante geldende artikelen komen eerst (\"erfbelasting vrijstelling gezinswoning\").",
    help_named_text: "Een bepaalde tekst",
    help_named_text_text: "Wie het wetboek, de cao of het land noemt, krijgt de artikelen ervan: \"code du travail préavis de démission\", \"code civil du Sénégal\".",
    help_article_number: "Een artikelnummer",
    help_article_number_text: "\"L1237-1\" brengt meteen het artikel en zijn pagina met versies naar boven.",
    help_refine: "Verfijnen",
    help_refine_text: "De filters onder de zoekbalk beperken de resultaten tot een rechtscollege, een soort tekst (wetboek, wet, cao…), een aard of een bron. Vanaf de pagina van een wetboek beperkt \"Zoeken in dit wetboek\" de zoekopdracht tot die tekst.",
    rail_years: "Jaren",
    rail_articles: "Meest aangehaalde artikelen",
    rail_courts: "Rechtscolleges",
    rail_solution: "Uitkomst",
    in_texts: "In de wetgeving",
    in_decisions: "In de rechtspraak",
    texts_stats,
    decisions_stats,
    search_texts: "Zoeken in de wetgeving",
    search_decisions: "Zoeken in de rechtspraak",
    see_texts: "Wetgeving bekijken",
    see_decisions: "Rechtspraak bekijken",
    decisions_title: "Rechtspraak - LibreJustice",
    counting: "Bezig met tellen",
    loading_corpus: "Corpus wordt geladen…",
    searching: "Bezig met zoeken…",
    looking_for_this_one: "Zoekt u deze beslissing?",
    looking_for_these: "Zoekt u deze beslissingen?",
    other_decisions_match: "Ook andere beslissingen hebben deze referentie",
    also_docket: "ook nr. ",
    open_decision: "Beslissing openen",
    pagination: "Paginering",
    texts_by_last_change: "Teksten, van de laatst gewijzigde tot de oudste",
    maybe_looking_for: "Misschien zoekt u",
    article_of,
    other_articles_match: "Ook andere artikelen hebben deze referentie",
    rail_scope: "Aard",
    rail_law: "Recht",
    rail_sources: "Bronnen",
    filter_source: "Bron",
    n_places,
    back_to_metropolitan_france: "Terug naar Europees Frankrijk",
    back_to_default_place,
    chip_in_text,
    chip_in_force_at,
    no_text_for_filters: "Geen tekst voor deze filters.",
    no_article_for,
    page_title: "Zoeken",
    texts_page_title: "Wetgeving zoeken",
    texts_title: "Wetgeving - LibreJustice",
    texts_title_for,
    results_summary: "Overzicht van de resultaten",
    see_also: "Zie ook",
    results: "Resultaten",
    search_results: "Zoekresultaten",
    pending: "Zoeken…",
    no_value: "Geen waarde",
    in_force: "Geldende artikelen en teksten",
    automatic: "Automatisch",
    catalogue_searching: "Zoeken in de catalogus…",
    suggestions: "Suggesties",
    journal_of,
    placeholder_decisions: "Zoeken in de rechtspraak…",
    placeholder_texts: "Zoeken in wetboeken, wetten en decreten…",
    placeholder_notices: "Voorwerp of partij…",
    rate_limited: "Te veel zoekopdrachten in korte tijd.",
    rate_limited_hint: "Probeer het over een minuut opnieuw.",
    too_long,
    too_long_hint: "Behoud de vraag en verwijder de geplakte tekst eromheen: de zoekmachine zoekt op de betekenis van de woorden, niet op de lengte.",
    unknown_filter: "Een filter van deze zoekopdracht wordt niet herkend.",
    unknown_filter_hint: "De link bevat wellicht een verouderde waarde: verwijder de filter om opnieuw te zoeken.",
    down: "De zoekopdracht is mislukt.",
    down_hint: "De API is misschien onbereikbaar. Probeer het zo meteen opnieuw.",
};

fn auto_chose_this(help: &str) -> String {
    format!("{help} De modus Auto heeft hier voor deze modus gekozen.")
}

fn page_n(page: u32) -> String {
    format!("Pagina {page}")
}

fn query_too_long(n: usize, max: usize) -> String {
    format!("{n} tekens ingegeven, het maximum is {max}: de zoekopdracht gebruikt de eerste {max}. Behoud de vraag en verwijder de geplakte tekst eromheen.")
}

fn nothing_found_for(query: &str) -> String {
    format!("Niets gevonden voor \"{query}\"")
}

fn chip_article(text: &str, article: &str) -> String {
    format!("{text} · art. {}", super::article_num(article))
}

fn chip_search_in(parts: &str) -> String {
    format!("In: {parts}")
}

fn chip_dates_between(from: &str, to: &str) -> String {
    format!("Van {from} tot en met {to}")
}

fn chip_dates_from(from: &str) -> String {
    format!("Vanaf {from}")
}

fn chip_dates_to(to: &str) -> String {
    format!("Tot en met {to}")
}

fn texts_stats(texts: &str, articles: &str) -> String {
    format!("{texts} teksten · {articles} geldende artikelen")
}

fn decisions_stats(decisions: &str) -> String {
    format!("{decisions} beslissingen, van gewone en administratieve rechtscolleges")
}

fn article_of(text: &str, num: &str) -> String {
    format!("{text}, artikel {}", super::article_num(num))
}

fn n_places(n: usize) -> String {
    format!("{n} plaatsen")
}

fn back_to_default_place(place: &str) -> String {
    format!("Terug naar de standaardplaats ({place})")
}

fn chip_in_text(title: &str) -> String {
    format!("In: {title}")
}

fn chip_in_force_at(day: &str) -> String {
    format!("Geldend op {day}")
}

fn no_article_for(query: &str) -> String {
    format!("Geen artikel voor \"{query}\".")
}

fn texts_title_for(query: &str) -> String {
    format!("{query} - Wetgeving - LibreJustice")
}

fn journal_of(day: &str) -> String {
    format!("JO van {day}")
}

fn too_long(chars: usize, max: usize) -> String {
    format!("Deze zoekopdracht telt {chars} tekens, het maximum is {max}.")
}
