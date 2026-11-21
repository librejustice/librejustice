use crate::i18n::Run::{Local, Text};
use crate::i18n::{ToolCopy, ToolsUi};

pub const TOOLS: ToolsUi = ToolsUi {
    title: "Werkzeuge - LibreJustice",
    description: "Die Werkzeuge von LibreJustice: MCP-Connector, REST-API, Anonymisierung von Dokumenten und Einreichung von Entscheiden.",
    eyebrow: "Werkzeuge",
    heading_lead: "Was LibreJustice ",
    heading_em: "neben der Suche bietet.",
    available: "Verfügbar",
    beta: "Beta",
    mcp: ToolCopy {
        title: "MCP-Connector",
        body: "Claude, ChatGPT oder Ihr eigener Agent durchsuchen die Entscheide und Erlasse.",
    },
    api: ToolCopy {
        title: "Öffentliche API",
        body: "Eine kostenlose REST-API, um Entscheide und Erlasse aus Ihrem Code zu durchsuchen.",
    },
    redact: ToolCopy {
        title: "Anonymisierung",
        body: "Das Werkzeug erkennt Namen und andere identifizierende Angaben in einem PDF, einem Word-Dokument oder einem Bild, und Sie wählen aus, welche entfernt werden. All das, ohne dass die Datei Ihren Browser verlässt.",
    },
    contribute: ToolCopy {
        title: "Entscheid einreichen",
        body: "Reichen Sie einen Entscheid ein, der LibreJustice fehlt.",
    },
    contribute_title: "Entscheid einreichen - LibreJustice",
    contribute_intro: &[
        Text("Anonymisieren Sie ihn zuerst mit dem "),
        Local("/redact", "Anonymisierungswerkzeug"),
        Text("."),
    ],
    contribute_sent: "Danke, der Entscheid wurde eingereicht.",
    contribute_drop: "Entscheid hier ablegen",
    contribute_attest: "Ich habe ihn anonymisiert und darf ihn weitergeben.",
    send: "Abschicken",
    contribute_error,
};

fn contribute_error(code: &str) -> &'static str {
    match code {
        "format" => "Nur PDF- und Word-Dateien werden akzeptiert.",
        "trop_lourd" => "Diese Datei ist grösser als 5 MB.",
        "trop" => "Zu viele Einreichungen kurz hintereinander. Bitte versuchen Sie es in einer Minute erneut.",
        "case" => "Kreuzen Sie das Feld an, um den Entscheid einzureichen.",
        _ => "Die Übermittlung ist fehlgeschlagen. Bitte versuchen Sie es erneut.",
    }
}
