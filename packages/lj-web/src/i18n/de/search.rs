use crate::i18n::SearchUi;

pub const SEARCH: SearchUi = SearchUi {
    search: "Suchen",
    clear_search: "Suche zurücksetzen",
    mode_group: "Suchmodus",
    mode_auto: "Auto",
    mode_auto_help: "Wählt den Modus, der am besten zu Ihrer Suche passt.",
    mode_lexical: "Stichwörter",
    mode_lexical_help: "Sucht nach Stichwörtern. Setzen Sie eine Wortfolge in Anführungszeichen oder verbinden Sie Begriffe mit AND, OR, NOT.",
    mode_semantic: "Sinngemäss",
    mode_semantic_help: "Sucht auch nach Synonymen und ähnlichen Formulierungen Ihrer Frage.",
    mode_reference: "Referenz",
    mode_reference_decision_help: "Findet einen Entscheid über seine Geschäftsnummer, seinen gebräuchlichen Namen oder über Gericht und Datum.",
    mode_reference_article_help: "Findet einen Artikel über sein Gesetzbuch und seine Nummer.",
    auto_chose_this,
    sort_label: "Treffer sortieren",
    sort_relevance: "Relevanz",
    sort_newest: "Neueste zuerst",
    sort_oldest: "Älteste zuerst",
    previous_page: "Vorherige Seite",
    next_page: "Nächste Seite",
    page_n,
    card_content: "Anzeige der Treffer",
    excerpt: "Auszug",
    summary: "Zusammenfassung",
    ai_sort: "KI-Sortierung",
    ai_sort_by_date: "Die KI-Sortierung gilt nicht für eine Sortierung nach Datum, die ihre Reihenfolge bereits vorgibt.",
    ai_sort_other_mode: "Die KI-Sortierung gilt nur im Modus «Sinngemäss»: Eine Suche mit Operatoren ordnet nach Übereinstimmung mit den eingegebenen Begriffen, eine Referenzsuche nach der Identität des Entscheids.",
    ai_sort_declined: "Die KI-Sortierung hat diese Liste nicht sortiert: zu wenige Treffer, oder der Dienst war ausgelastet. Die angezeigte Reihenfolge folgt dem Relevanzwert der Suche.",
    ai_sort_on: "Eine KI hat die ersten 50 Treffer nach Ihrer Frage sortiert. Klicken, um zur Reihenfolge nach Relevanzwert zurückzukehren.",
    ai_sort_off: "Die ersten 50 Treffer von einer KI nach Ihrer Frage sortieren lassen.",
    query_too_long,
    no_results: "Keine Treffer",
    no_decision_for_filters: "Kein Entscheid entspricht diesen Filtern",
    remove_a_filter: "Löschen Sie einen Filter oder erweitern Sie den Zeitraum.",
    nothing_found_for,
    rephrase_before: "Versuchen Sie es mit weniger Begriffen, mit Synonymen oder mit booleschen Operatoren (",
    op_and: "AND",
    op_or: "OR",
    op_not: "NOT",
    rephrase_after: "). Der Modus «Sinngemäss» funktioniert am besten mit natürlichen Formulierungen.",
    filters: "Suchfilter",
    filter_date: "Datum",
    filter_jurisdiction: "Gericht",
    filter_jurisdiction_placeholder: "Filtern (z. B. Bern …)",
    filter_publication: "Publikation",
    filter_solution: "Verfahrensausgang",
    filter_search_in: "Suchen in",
    filter_domain: "Rechtsgebiet",
    filter_cited_texts: "Zitierte Erlasse",
    filter_cited_texts_placeholder: "Filtern oder Artikel suchen …",
    filter_formation: "Besetzung",
    fewer_filters: "Weniger Filter",
    more_filters: "Weitere Filter",
    part_headnote: "Regeste",
    part_headnote_hint: "Die vom Gericht publizierte Regeste oder Analyse.",
    part_judgment: "Entscheid",
    part_annexes: "Beschwerdegründe im Anhang",
    part_conclusions: "Schlussanträge",
    part_conclusions_hint: "Die Schlussanträge des Generalanwalts oder des rapporteur public.",
    chip_article,
    chip_search_in,
    chip_dates_between,
    chip_dates_from,
    chip_dates_to,
    remove_filter: "Filter löschen",
    clear_all: "Filter zurücksetzen",
    collapse: "Ausblenden",
    expand: "Einblenden",
    show_less: "Weniger anzeigen",
    show_more: "Mehr anzeigen",
    help: "Suchhilfe",
    close: "Schliessen",
    help_modes: "Suchmodi",
    help_find_one: "Einen bestimmten Entscheid finden",
    help_designations: &[
        ("19-22.015", "Geschäftsnummer"),
        ("1922015", "dieselbe ohne Trennzeichen"),
        ("ECLI:FR:CCASS:2022:CO00288", "ECLI"),
        ("CE 20 octobre 1989", "Gericht und Datum"),
        ("arrêt Nicolo", "gebräuchlicher Name"),
    ],
    help_ai_sort_text: "Im Modus «Sinngemäss» standardmässig aktiv: Eine KI ordnet die ersten 50 Treffer nach Ihrer Frage neu.",
    help_operators_title: "Boolesche Operatoren",
    help_operators: &[
        ("AND", "Schnittmenge"),
        ("OR", "Vereinigung"),
        ("NOT", "Ausschluss"),
        ("NEAR5", "höchstens 5 Wörter Abstand"),
        ("\"…\"", "exakte Wortfolge"),
        ("*", "Trunkierung"),
    ],
    help_how: "So suchen Sie",
    help_notion: "Ein Begriff",
    help_notion_text: "Freie Stichwörter: Die relevantesten geltenden Artikel erscheinen zuerst («Leinenpflicht Hunde»).",
    help_named_text: "Ein bestimmter Erlass",
    help_named_text_text: "Nennen Sie den Erlass, den Gesamtarbeitsvertrag oder das Land, um gezielt dessen Artikel zu finden: «Kündigungsfrist Normalarbeitsvertrag Hauspersonal», «Zivilgesetzbuch Senegal».",
    help_article_number: "Eine Artikelnummer",
    help_article_number_text: "«Art. 336c OR» führt direkt zum Artikel und zu seiner Seite mit allen Fassungen.",
    help_refine: "Eingrenzen",
    help_refine_text: "Die Filter unter dem Suchfeld grenzen die Treffer nach Rechtsordnung, Art des Erlasses (Gesetzbuch, Gesetz, Gesamtarbeitsvertrag …), Geltungsbereich und Quelle ein. Auf der Seite eines Gesetzbuchs beschränkt «In diesem Gesetzbuch suchen» die Suche auf diesen Erlass.",
    rail_years: "Jahre",
    rail_articles: "Meistzitierte Artikel",
    rail_courts: "Gerichte",
    rail_solution: "Verfahrensausgang",
    in_texts: "In den Erlassen",
    in_decisions: "In den Entscheiden",
    texts_stats,
    decisions_stats,
    search_texts: "In den Erlassen suchen",
    search_decisions: "In den Entscheiden suchen",
    see_texts: "Erlasse anzeigen",
    see_decisions: "Entscheide anzeigen",
    decisions_title: "Entscheide - LibreJustice",
    counting: "Zählung läuft",
    loading_corpus: "Bestand wird geladen …",
    searching: "Suche läuft …",
    looking_for_this_one: "Suchen Sie diesen Entscheid?",
    looking_for_these: "Suchen Sie diese Entscheide?",
    other_decisions_match: "Weitere Entscheide entsprechen dieser Referenz",
    also_docket: "auch Nr. ",
    open_decision: "Entscheid öffnen",
    pagination: "Seitennavigation",
    texts_by_last_change: "Erlasse, zuletzt geänderte zuerst",
    maybe_looking_for: "Suchen Sie vielleicht",
    article_of,
    other_articles_match: "Weitere Artikel tragen diese Referenz",
    rail_scope: "Geltungsbereich",
    rail_law: "Recht",
    rail_sources: "Quellen",
    filter_source: "Quelle",
    n_places,
    back_to_metropolitan_france: "Zurück zum europäischen Frankreich",
    back_to_default_place,
    chip_in_text,
    chip_in_force_at,
    no_text_for_filters: "Keine Erlasse für diese Filter.",
    no_article_for,
    page_title: "Suche",
    texts_page_title: "Suche in Erlassen",
    texts_title: "Erlasse - LibreJustice",
    texts_title_for,
    results_summary: "Übersicht der Treffer",
    see_also: "Siehe auch",
    results: "Treffer",
    search_results: "Suchtreffer",
    pending: "Wird gesucht …",
    no_value: "Keine Werte",
    in_force: "Geltende Artikel und Erlasse",
    automatic: "Automatisch",
    catalogue_searching: "Katalog wird durchsucht …",
    suggestions: "Vorschläge",
    journal_of,
    placeholder_decisions: "In der Rechtsprechung suchen …",
    placeholder_texts: "In Gesetzbüchern und Gesetzen suchen …",
    placeholder_notices: "Gegenstand oder Beteiligte: «Strassenbau», «Konkurs» …",
    rate_limited: "Zu viele Suchen in kurzer Zeit.",
    rate_limited_hint: "Bitte versuchen Sie es in einer Minute erneut.",
    too_long,
    too_long_hint: "Behalten Sie die Frage und entfernen Sie den mitkopierten Text: Die Suche richtet sich nach dem Sinn der Wörter, nicht nach der Länge.",
    unknown_filter: "Ein Filter dieser Suche wird nicht erkannt.",
    unknown_filter_hint: "Der Link enthält wohl einen veralteten Wert: Löschen Sie den Filter, um die Suche erneut zu starten.",
    down: "Die Suche ist fehlgeschlagen.",
    down_hint: "Die API ist möglicherweise nicht erreichbar. Bitte versuchen Sie es gleich erneut.",
};

fn auto_chose_this(help: &str) -> String {
    format!("{help} Hier vom Modus «Auto» gewählt.")
}

fn page_n(page: u32) -> String {
    format!("Seite {page}")
}

fn query_too_long(n: usize, max: usize) -> String {
    format!("{n} Zeichen eingegeben, zulässig sind {max}: Gesucht wird nur mit den ersten {max}. Behalten Sie die Frage und entfernen Sie den mitkopierten Text.")
}

fn nothing_found_for(query: &str) -> String {
    format!("Keine Treffer für «{query}»")
}

fn chip_article(text: &str, article: &str) -> String {
    format!("{text} · Art. {}", super::article_num(article))
}

fn chip_search_in(parts: &str) -> String {
    format!("In: {parts}")
}

fn chip_dates_between(from: &str, to: &str) -> String {
    format!("Vom {from} bis {to}")
}

fn chip_dates_from(from: &str) -> String {
    format!("Ab {from}")
}

fn chip_dates_to(to: &str) -> String {
    format!("Bis {to}")
}

fn texts_stats(texts: &str, articles: &str) -> String {
    format!("{texts} Erlasse · {articles} geltende Artikel")
}

fn decisions_stats(decisions: &str) -> String {
    format!("{decisions} Entscheide aus zwei Gerichtsbarkeiten")
}

fn article_of(text: &str, num: &str) -> String {
    format!("{text}, Artikel {}", super::article_num(num))
}

fn n_places(n: usize) -> String {
    format!("{n} Orte")
}

fn back_to_default_place(place: &str) -> String {
    format!("Zurück zum Standardort ({place})")
}

fn chip_in_text(title: &str) -> String {
    format!("In: {title}")
}

fn chip_in_force_at(day: &str) -> String {
    format!("In Kraft am {day}")
}

fn no_article_for(query: &str) -> String {
    format!("Kein Artikel für «{query}».")
}

fn texts_title_for(query: &str) -> String {
    format!("{query} - Erlasse - LibreJustice")
}

fn journal_of(day: &str) -> String {
    format!("JO vom {day}")
}

fn too_long(chars: usize, max: usize) -> String {
    format!("Diese Suche umfasst {chars} Zeichen, zulässig sind höchstens {max}.")
}
