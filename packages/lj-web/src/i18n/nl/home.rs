use crate::i18n::HomeUi;

pub const HOME: HomeUi = HomeUi {
    eyebrow: "Vrij en gratis toegankelijk",
    headline,
    headline_accent: "voor iedereen.",
    tagline: "Zoek in rechterlijke beslissingen, wetten, wetboeken, collectieve arbeidsovereenkomsten en overheidsopdrachten, of in de gids van partijen en advocaten.",
    query_label: "Zoektermen of vraag",
    query_placeholder: "Zoektermen, vraag of exacte woordgroep…",
    scope_label: "Zoeken in",
    advanced_search: "Uitgebreid zoeken",
    browse_directory: "Bladeren in de gids",
    examples_case_law: "Rechtspraak",
    examples_case_law_queries: |juris| match juris {
        "nl" => &[
            "ontslag op staande voet dringende reden",
            "vreemdelingenbewaring schadevergoeding",
        ],
        _ => &[
            "bevel om het grondgebied te verlaten hoorrecht",
            "inreisverbod gezinsleven artikel 8 EVRM",
        ],
    },
    examples_texts: "Wetten, decreten, besluiten",
    examples_texts_queries: |juris| match juris {
        "nl" => &[
            "onrechtmatige daad schadevergoeding",
            "opzegging huurovereenkomst woonruimte",
        ],
        _ => &[
            "omgevingsvergunning beroepstermijn",
            "erfbelasting vrijstelling gezinswoning",
        ],
    },
    examples_notices: "Franse overheidsopdrachten en ondernemingen",
    examples_notices_queries: |_| &[],
    examples_directory_queries: |juris| match juris {
        "fr" => &["SNCF"],
        _ => &[],
    },
    mcp_eyebrow: "MCP-connector",
    mcp_pitch: "Verbind uw AI (Claude, ChatGPT, Perplexity, Mistral…) met het recht.",
    mcp_setup: "Instellen",
    stat_decisions: "Beslissingen",
    stat_texts: "Regelgevende teksten",
};

fn headline(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("{adjective} recht, "),
        None => "Het recht, ".to_string(),
    }
}
