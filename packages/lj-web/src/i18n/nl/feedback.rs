use crate::i18n::FeedbackUi;
use crate::i18n::Run::{Local, Text};

pub const FEEDBACK: FeedbackUi = FeedbackUi {
    title: "Een probleem melden - LibreJustice",
    heading: "Een probleem melden",
    intro: &[
        Text("Een pagina die niet werkt, een zoekopdracht die ontspoort, ontbrekende gegevens: meld het hier. De tekst van de beslissingen en de wetten wordt ongewijzigd overgenomen van "),
        Local("/sources", "de bron"),
        Text(", die als enige een fout erin kan rechtzetten."),
    ],
    subject: "Waarover gaat het?",
    subjects: [
        "Technisch probleem op de site",
        "Ontbrekende of onvolledige gegevens",
        "Account, inloggen, connector",
        "Iets anders",
    ],
    page: "Betrokken pagina",
    message: "Uw bericht",
    email: "Uw e-mailadres (facultatief)",
    email_placeholder: "u@voorbeeld.be",
    email_hint: "Zodat we u kunnen antwoorden. Bent u ingelogd, dan gebruiken we het adres van uw account.",
    send: "Verzenden",
    sent_heading: "Bericht verzonden",
    sent_body: "Bedankt. Hebt u uw adres achtergelaten, dan antwoorden we u.",
    back_to_page: "Terug naar de pagina",
    error,
};

fn error(code: &str) -> &'static str {
    match code {
        "message_trop_court" => "Beschrijf het probleem in iets meer woorden.",
        "message_trop_long" => "Bericht te lang: maximaal 4.000 tekens.",
        "adresse_invalide" => "Dit e-mailadres is niet geldig.",
        "trop_de_signalements" => {
            "Te veel meldingen kort na elkaar. Probeer het over een minuut opnieuw."
        }
        "envoi_impossible" => {
            "Uw bericht is niet vertrokken. Schrijf ons op contact@librejustice.fr."
        }
        _ => "Uw bericht kon niet worden verzonden.",
    }
}
