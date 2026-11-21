use crate::i18n::FeedbackUi;
use crate::i18n::Run::{Local, Text};

pub const FEEDBACK: FeedbackUi = FeedbackUi {
    title: "Problem melden - LibreJustice",
    heading: "Problem melden",
    intro: &[
        Text("Eine Seite, die nicht funktioniert, eine Suche, die aus dem Ruder läuft, fehlende Daten: Melden Sie es hier. Der Text der Entscheide und Gesetze wird unverändert von "),
        Local("/sources", "der Quelle"),
        Text(" übernommen, und nur sie kann einen Fehler darin berichtigen."),
    ],
    subject: "Worum geht es?",
    subjects: [
        "Technisches Problem auf der Website",
        "Fehlende oder unvollständige Daten",
        "Konto, Anmeldung, Connector",
        "Sonstiges",
    ],
    page: "Betroffene Seite",
    message: "Ihre Nachricht",
    email: "Ihre E-Mail-Adresse (optional)",
    email_placeholder: "name@beispiel.ch",
    email_hint: "Damit wir Ihnen antworten können. Wenn Sie angemeldet sind, wird die Adresse Ihres Kontos verwendet.",
    send: "Abschicken",
    sent_heading: "Nachricht abgeschickt",
    sent_body: "Danke. Wenn Sie Ihre Adresse angegeben haben, antworten wir Ihnen.",
    back_to_page: "Zurück zur Seite",
    error,
};

fn error(code: &str) -> &'static str {
    match code {
        "message_trop_court" => "Beschreiben Sie das Problem mit ein paar Worten mehr.",
        "message_trop_long" => "Nachricht zu lang: höchstens 4\u{2019}000 Zeichen.",
        "adresse_invalide" => "Diese E-Mail-Adresse ist ungültig.",
        "trop_de_signalements" => {
            "Zu viele Meldungen kurz nacheinander. Bitte versuchen Sie es in einer Minute erneut."
        }
        "envoi_impossible" => {
            "Ihre Nachricht wurde nicht gesendet. Schreiben Sie uns an contact@librejustice.fr."
        }
        _ => "Ihre Nachricht konnte nicht gesendet werden.",
    }
}
