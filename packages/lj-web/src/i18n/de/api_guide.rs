use crate::i18n::ApiGuideUi;
use crate::i18n::Run::{Code, File, Local, Text};

pub const API_GUIDE: ApiGuideUi = ApiGuideUi {
    title: "Öffentliche API - LibreJustice",
    description: "Die REST-API von LibreJustice: Suche in Rechtsprechung und Erlassen, kostenloser Schlüssel.",
    eyebrow: "Öffentliche API · Beta",
    heading: "Eine API ",
    heading_law,
    intro: "Die API bietet per REST Zugriff auf dieselbe Datenbank wie die Website und der MCP-Connector. Rechtsprechung und Erlasse werden täglich aus den offiziellen Quellen aktualisiert, und der Zugangsschlüssel ist kostenlos.",
    key_heading: "Schlüssel erstellen",
    key_steps: &[
        Text("Registrieren Sie sich und erstellen Sie danach in "),
        Local("/profile", "Ihrem Profil"),
        Text(" einen Schlüssel. Der geheime Schlüsselwert wird nur einmal angezeigt. Ein Schlüssel kann jederzeit widerrufen werden."),
    ],
    example,
    key_header: &[
        Text("Der Header "),
        Code("x-lj-api-key"),
        Text(" wird anstelle von "),
        Code("Bearer"),
        Text(" akzeptiert."),
    ],
    served_heading: "Was die API liefert",
    routes: &[
        (
            "GET /api/search",
            "Suche nach Entscheiden, mit den Parameternamen des MCP-Tools `search_decisions`: `query`, `limit`, `offset`, Filter nach Gericht und Datum.",
        ),
        (
            "GET /api/search-textes",
            "Suche nach Artikeln im Bestand der Erlasse. `query`, `code`, um auf einen Erlass einzugrenzen, `as_of`, um in den an diesem Tag geltenden Fassungen zu suchen.",
        ),
        (
            "GET /api/decision/{id}",
            "Ein vollständiger Entscheid mit Text und Metadaten.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}",
            "Ein Erlass ohne Artikel liefert seinen Text, ein Erlass mit Artikeln sein Inhaltsverzeichnis. Die Rechtsordnung wird mit ihrem ISO-Code in Kleinbuchstaben angegeben (`fr`, `be`, `eu`), der Erlass mit seinem Slug, seinem Namen oder seiner Abkürzung.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{article}",
            "Ein Artikel in seiner geltenden Fassung oder in der Fassung, die an dem in `as_of` übergebenen Tag galt.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{path}",
            "Eine Gliederungseinheit des Erlasses mit ihren Artikeln, bezeichnet mit dem Pfad aus dem Inhaltsverzeichnis (`livre-i/titre-v`).",
        ),
        (
            "GET /api/article/{LEGIARTI…}",
            "Derselbe Artikel, adressiert über seine Légifrance-Kennung, die auch einen Erlass oder einen Abschnitt bezeichnen kann.",
        ),
    ],
    beta: &[
        Text("Die API ist in der Beta-Phase: Namen von Parametern und Feldern können sich noch ändern. Die vollständige Schnittstellenbeschreibung mit Parametern und Antworten liegt im Format OpenAPI vor: "),
        File("/openapi.json", "openapi.json"),
        Text(". Wenn Sie für Ihre Nutzung etwas anderes brauchen, "),
        Local("/feedback", "sagen Sie es uns"),
        Text("."),
    ],
    rate_heading: "Kontingent und faire Nutzung",
    rate: &[
        Text("Ihre API-Aufrufe, Ihre MCP-Aufrufe und Ihre Suchen auf der Website zählen auf dasselbe Kontingent. Bei einer Überschreitung antwortet die API mit "),
        Code("429"),
        Text(" und "),
        Code("Retry-After"),
        Text(". Wenn Ihre legitime Nutzung diese Obergrenze übersteigt, schreiben Sie uns, statt sie zu umgehen. Der Bestand ist öffentlich und kostenlos, und das soll so bleiben, weshalb wir darauf achten, die Ressourcen unter allen Nutzerinnen und Nutzern aufzuteilen."),
    ],
    terms: &[
        Text("Für die API gelten die "),
        Local("/terms", "Nutzungsbedingungen"),
        Text(" des Dienstes."),
    ],
    assistant_heading: "Lieber ein Assistent?",
    assistant: &[
        Text("Wenn nicht Ihr Code, sondern eine KI die Daten abfragt, erledigt der "),
        Local("/mcp-guide", "MCP-Connector"),
        Text(" dieselbe Arbeit, ohne dass Sie eine Anfrage schreiben: Der Assistent führt die Suchen selbst nacheinander aus."),
    ],
};

fn heading_law(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("für das {adjective} Recht."),
        None => "für das Recht.".to_string(),
    }
}

fn example(base: &str) -> String {
    format!(
        "curl -H 'Authorization: Bearer ljk_ihr_schluessel' \\\n  \
         '{base}/api/search?query=Nachbarrecht+Immissionen&limit=3'"
    )
}
