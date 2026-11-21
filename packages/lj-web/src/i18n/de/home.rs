use crate::i18n::HomeUi;

pub const HOME: HomeUi = HomeUi {
    eyebrow: "Frei zugänglich und kostenlos",
    headline,
    headline_accent: "für alle.",
    tagline: "Durchsuchen Sie Gerichtsentscheide, Gesetze, Gesetzbücher, Gesamtarbeitsverträge und öffentliche Ausschreibungen oder das Verzeichnis der Parteien und der Anwältinnen und Anwälte.",
    query_label: "Stichwörter oder Frage",
    query_placeholder: "Stichwörter, Frage oder exakte Wortfolge …",
    scope_label: "Suchbereich",
    advanced_search: "Erweiterte Suche",
    browse_directory: "Verzeichnis durchblättern",
    examples_case_law: "Rechtsprechung",
    examples_case_law_queries: |_| {
        &[
            "Arzthaftung Spitalinfektion",
            "übermässige Immissionen Nachbarn Entschädigung",
        ]
    },
    examples_texts: "Gesetze, Gesetzbücher, GAV",
    examples_texts_queries: |_| {
        &[
            "Gewährleistung versteckte Mängel Obligationenrecht",
            "Feiertage Gesamtarbeitsvertrag Bäckerei",
        ]
    },
    examples_notices: "Ausschreibungen, Unternehmensmeldungen",
    examples_notices_queries: |juris| match juris {
        "fr" => &["voirie communale", "redressement judiciaire"],
        _ => &[],
    },
    examples_directory_queries: |juris| match juris {
        "fr" => &["SNCF"],
        _ => &[],
    },
    mcp_eyebrow: "MCP-Connector",
    mcp_pitch: "Verbinden Sie Ihre KI (Claude, ChatGPT, Perplexity, Mistral …) mit dem französischen Recht.",
    mcp_setup: "Einrichten",
    stat_decisions: "Entscheide",
    stat_texts: "Erlasse",
};

fn headline(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("Das {adjective} Recht, "),
        None => "Das Recht, ".to_string(),
    }
}
