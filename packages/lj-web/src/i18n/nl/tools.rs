use crate::i18n::Run::{Local, Text};
use crate::i18n::{ToolCopy, ToolsUi};

pub const TOOLS: ToolsUi = ToolsUi {
    title: "Tools - LibreJustice",
    description: "De tools van LibreJustice: MCP-connector, REST-API, anonimisering van documenten en het insturen van beslissingen.",
    eyebrow: "Tools",
    heading_lead: "Wat LibreJustice doet ",
    heading_em: "naast het zoeken.",
    available: "Beschikbaar",
    beta: "Bèta",
    mcp: ToolCopy {
        title: "MCP-connector",
        body: "Claude, ChatGPT of uw eigen agent zoekt in de beslissingen en de wetgeving.",
    },
    api: ToolCopy {
        title: "Publieke API",
        body: "Een gratis REST-API om vanuit uw code beslissingen en wetgeving te doorzoeken.",
    },
    redact: ToolCopy {
        title: "Anonimisering",
        body: "De tool spoort namen en andere identificerende gegevens op in een PDF, een Word-document of een afbeelding, en u kiest welke u verwijdert. Het bestand verlaat daarbij nooit uw browser.",
    },
    contribute: ToolCopy {
        title: "Een beslissing insturen",
        body: "Stuur een beslissing in die nog ontbreekt in LibreJustice.",
    },
    contribute_title: "Een beslissing insturen - LibreJustice",
    contribute_intro: &[
        Text("Haal ze eerst door de "),
        Local("/redact", "anonimiseringstool"),
        Text("."),
    ],
    contribute_sent: "Bedankt, de beslissing is verzonden.",
    contribute_drop: "Sleep de beslissing hierheen",
    contribute_attest: "Ik heb ze geanonimiseerd en ik heb het recht ze te delen.",
    send: "Verzenden",
    contribute_error,
};

fn contribute_error(code: &str) -> &'static str {
    match code {
        "format" => "Alleen PDF- en Word-bestanden worden aanvaard.",
        "trop_lourd" => "Dit bestand is groter dan 5 MB.",
        "trop" => "Te veel inzendingen kort na elkaar. Probeer het over een minuut opnieuw.",
        "case" => "Vink het vakje aan om de beslissing te verzenden.",
        _ => "Het verzenden is niet gelukt. Probeer het opnieuw.",
    }
}
