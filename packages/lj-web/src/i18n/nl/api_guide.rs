use crate::i18n::ApiGuideUi;
use crate::i18n::Run::{Code, File, Local, Text};

pub const API_GUIDE: ApiGuideUi = ApiGuideUi {
    title: "Publieke API - LibreJustice",
    description: "De REST-API van LibreJustice: zoeken in rechtspraak en regelgeving, met een gratis sleutel.",
    eyebrow: "Publieke API · bèta",
    heading: "Een API ",
    heading_law,
    intro: "De API geeft in REST toegang tot dezelfde databank als de site en de MCP-connector. De rechtspraak en de regelgeving worden elke dag bijgewerkt vanuit de officiële bronnen, en de toegangssleutel is gratis.",
    key_heading: "Een sleutel aanmaken",
    key_steps: &[
        Text("Maak een account aan en daarna een sleutel vanuit "),
        Local("/profile", "uw profiel"),
        Text(". Het geheim wordt maar één keer getoond. Een sleutel kan op elk moment worden ingetrokken."),
    ],
    example,
    key_header: &[
        Text("De header "),
        Code("x-lj-api-key"),
        Text(" wordt aanvaard in plaats van "),
        Code("Bearer"),
        Text("."),
    ],
    served_heading: "Wat de API levert",
    routes: &[
        (
            "GET /api/search",
            "Beslissingen zoeken, met de parameternamen van de MCP-tool `search_decisions`: `query`, `limit`, `offset`, filters op rechtscollege en datum.",
        ),
        (
            "GET /api/search-textes",
            "Artikelen zoeken in de regelgeving. `query`, `code` om tot één tekst te beperken, `as_of` om te zoeken in de versies die op die dag golden.",
        ),
        (
            "GET /api/decision/{id}",
            "Een volledige beslissing, met haar tekst en haar metadata.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}",
            "Een tekst zonder artikelen geeft zijn volledige tekst terug; een tekst met artikelen geeft zijn inhoudsopgave. Het rechtsstelsel is zijn ISO-code in kleine letters (`fr`, `be`, `eu`); de tekst wordt aangeduid met zijn slug, zijn naam of zijn afkorting.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{article}",
            "Een artikel in zijn geldende versie, of zoals het luidde op de dag die in `as_of` wordt meegegeven.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{path}",
            "Een onderdeel van de tekst en zijn artikelen, aangeduid met het pad uit de inhoudsopgave (`livre-i/titre-v`).",
        ),
        (
            "GET /api/article/{LEGIARTI…}",
            "Hetzelfde artikel, aangeduid met zijn Légifrance-identificatiecode, die ook een tekst of een afdeling kan aanduiden.",
        ),
    ],
    beta: &[
        Text("De API is in bèta: de namen van parameters en velden kunnen nog veranderen. Het volledige contract, parameters en antwoorden, staat in OpenAPI: "),
        File("/openapi.json", "openapi.json"),
        Text(". Hebt u iets anders nodig, "),
        Local("/feedback", "laat het ons weten"),
        Text("."),
    ],
    rate_heading: "Debiet en goed gebruik",
    rate: &[
        Text("Uw API-oproepen, uw MCP-oproepen en uw zoekopdrachten op de site worden op dezelfde teller aangerekend. Wie de grens overschrijdt, krijgt een "),
        Code("429"),
        Text(" met een "),
        Code("Retry-After"),
        Text(". Gaat uw legitieme gebruik boven die grens, schrijf ons dan in plaats van ze te omzeilen: de gegevens zijn openbaar en gratis, we willen dat zo houden, en daarom verdelen we de middelen zorgvuldig over alle gebruikers."),
    ],
    terms: &[
        Text("Op de API zijn de "),
        Local("/terms", "gebruiksvoorwaarden"),
        Text(" van de dienst van toepassing."),
    ],
    assistant_heading: "Liever een assistent?",
    assistant: &[
        Text("Is de gebruiker een AI en niet uw code, dan doet de "),
        Local("/mcp-guide", "MCP-connector"),
        Text(" hetzelfde werk zonder dat u een zoekopdracht schrijft: de assistent voert de zoekopdrachten zelf na elkaar uit."),
    ],
};

fn heading_law(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("voor {adjective} recht."),
        None => "voor het recht.".to_string(),
    }
}

fn example(base: &str) -> String {
    format!(
        "curl -H 'Authorization: Bearer ljk_uw_sleutel' \\\n  \
         '{base}/api/search?query=inreisverbod+gezinsleven+artikel+8+EVRM&limit=3'"
    )
}
