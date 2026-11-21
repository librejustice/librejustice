use crate::i18n::ProfileUi;
use crate::i18n::Run::{Strong, Text};

pub const PROFILE: ProfileUi = ProfileUi {
    title: "Profiel - LibreJustice",
    heading: "Profiel",
    done,
    sent,
    identity: "Identiteit",
    email_address: "E-mailadres",
    display_name: "Weergavenaam",
    anonymous: "Anoniem",
    save: "Opslaan",
    change_email: "E-mailadres wijzigen: er gaat een bevestigingslink naar het huidige adres, daarna naar het nieuwe",
    new_email_placeholder: "nieuw@adres.be",
    request_change: "Wijziging aanvragen",
    connections: "Inlogmethoden",
    password: "Wachtwoord",
    change: "Wijzigen",
    create: "Aanmaken",
    api_keys: "API-sleutels",
    delete_heading: "Mijn account verwijderen",
    delete_warning: &[
        Text("Deze handeling verwijdert uw account "),
        Strong("definitief"),
        Text(". Herstellen is niet mogelijk."),
    ],
    delete_type: "Typ ",
    delete_word: "VERWIJDEREN",
    delete_to_confirm: " om te bevestigen",
    delete_forever: "Definitief verwijderen",
    unlink: "Ontkoppelen",
    link: "Koppelen",
    session_expired: "Uw sessie is verlopen.",
    sign_in_again: "Opnieuw inloggen",
    keys_intro: "Een sleutel geeft toegang tot de publieke API, dezelfde zoekfunctie als de MCP-connector, in REST. Hij is gratis en kan op elk moment worden ingetrokken.",
    keys_docs: "De documentatie bekijken",
    key_name_placeholder: "Naam van de sleutel (facultatief)",
    create_key: "Een sleutel aanmaken",
    copy_key_now: "Kopieer deze sleutel nu: hij wordt niet meer getoond.",
    key_failed: "De sleutel kon niet worden verwerkt: ",
    revoke: "Intrekken",
    default_key_label,
};

fn default_key_label(day: u32, month: u32, year: u32) -> String {
    format!("Sleutel van {day:02}/{month:02}/{year}")
}

fn done(code: &str) -> &'static str {
    match code {
        "nom" => "Profiel bijgewerkt.",
        "google" => "Google-account gekoppeld.",
        "microsoft" => "Microsoft-account gekoppeld.",
        "deliaison" => "Inlogmethode verwijderd.",
        "email" => "E-mailadres bijgewerkt.",
        _ => "Gedaan.",
    }
}

fn sent(code: &str) -> &'static str {
    match code {
        "password" => "E-mail verzonden: open de link erin om het wachtwoord te kiezen.",
        "email_change" => {
            "E-mail verzonden naar uw huidige adres: bevestig daar de aanvraag tot wijziging."
        }
        _ => "E-mail verzonden.",
    }
}
