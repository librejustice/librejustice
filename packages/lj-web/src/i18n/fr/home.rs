use crate::i18n::HomeUi;

pub const HOME: HomeUi = HomeUi {
    eyebrow: "Accès libre et gratuit",
    headline,
    headline_accent: "pour tous.",
    tagline: "Cherchez dans les décisions de justice, les lois, les codes, les conventions collectives et les marchés publics, ou dans l'annuaire des parties et des avocats.",
    query_label: "Mots-clés ou question",
    query_placeholder: "Mots-clés, question ou expression exacte…",
    scope_label: "Portée de la recherche",
    advanced_search: "Recherche avancée",
    browse_directory: "Parcourir l'annuaire",
    examples_case_law: "Jurisprudence",
    examples_case_law_queries: |juris| match juris {
        "dj" => &[
            "requête en annulation des opérations électorales",
            "conformité de la loi organique à la Constitution",
        ],
        _ => &[
            "responsabilité médicale infection nosocomiale",
            "trouble anormal de voisinage indemnisation",
        ],
    },
    examples_texts: "Lois, codes, conventions collectives",
    examples_texts_queries: |juris| match juris {
        "dj" => &[
            "contrat de travail période d'essai",
            "sociétés de la zone franche",
        ],
        _ => &[
            "garantie des vices cachés code civil",
            "jours fériés convention collective de la boulangerie",
        ],
    },
    examples_notices: "Marchés publics, vie des entreprises",
    examples_notices_queries: |juris| match juris {
        "fr" => &["voirie communale", "redressement judiciaire"],
        _ => &[],
    },
    examples_directory_queries: |juris| match juris {
        "fr" => &["SNCF"],
        _ => &[],
    },
    mcp_eyebrow: "Connecteur MCP",
    mcp_pitch: "Connectez votre IA (Claude, ChatGPT, Perplexity, Mistral…) au droit français.",
    mcp_setup: "Configurer",
    stat_decisions: "Décisions",
    stat_texts: "Textes normatifs",
};

fn headline(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("Le droit {adjective}, "),
        None => "Le droit, ".to_string(),
    }
}
