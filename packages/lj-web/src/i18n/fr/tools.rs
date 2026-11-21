use crate::i18n::Run::{Local, Text};
use crate::i18n::{ToolCopy, ToolsUi};

pub const TOOLS: ToolsUi = ToolsUi {
    title: "Outils - LibreJustice",
    description: "Les outils de LibreJustice : connecteur MCP, API REST, anonymisation de documents et versement de décisions.",
    eyebrow: "Outils",
    heading_lead: "Ce que LibreJustice fait ",
    heading_em: "en dehors de la recherche.",
    available: "Disponible",
    beta: "Bêta",
    mcp: ToolCopy {
        title: "Connecteur MCP",
        body: "Claude, ChatGPT ou votre propre agent cherchent dans les décisions et les textes.",
    },
    api: ToolCopy {
        title: "API publique",
        body: "Une API REST gratuite pour chercher les décisions et les textes depuis votre code.",
    },
    redact: ToolCopy {
        title: "Anonymisation",
        body: "L'outil repère les noms et autres données identifiantes dans un PDF, un document Word ou une image et vous choisissez celles à retirer. Tout ça sans que le fichier quitte votre navigateur.",
    },
    contribute: ToolCopy {
        title: "Verser une décision",
        body: "Envoyez une décision qui manque à LibreJustice.",
    },
    contribute_title: "Verser une décision - LibreJustice",
    contribute_intro: &[
        Text("Passez-la d'abord dans l'"),
        Local("/redact", "atelier d'anonymisation"),
        Text("."),
    ],
    contribute_sent: "Merci, la décision est envoyée.",
    contribute_drop: "Déposez la décision ici",
    contribute_attest: "Je l'ai anonymisée et j'ai le droit de la partager.",
    send: "Envoyer",
    contribute_error,
};

fn contribute_error(code: &str) -> &'static str {
    match code {
        "format" => "Seuls les fichiers PDF et Word sont acceptés.",
        "trop_lourd" => "Ce fichier dépasse 5 Mo.",
        "trop" => "Trop d'envois coup sur coup. Réessayez dans une minute.",
        "case" => "Cochez la case pour envoyer la décision.",
        _ => "L'envoi n'a pas abouti. Réessayez.",
    }
}
