use crate::i18n::ProfileUi;
use crate::i18n::Run::{Strong, Text};

pub const PROFILE: ProfileUi = ProfileUi {
    title: "Profil - LibreJustice",
    heading: "Profil",
    done,
    sent,
    identity: "Identität",
    email_address: "E-Mail-Adresse",
    display_name: "Angezeigter Name",
    anonymous: "Anonym",
    save: "Speichern",
    change_email: "E-Mail-Adresse ändern: Ein Bestätigungslink geht zuerst an die aktuelle Adresse, dann an die neue",
    new_email_placeholder: "neue@adresse.ch",
    request_change: "Änderung beantragen",
    connections: "Anmeldemethoden",
    password: "Passwort",
    change: "Ändern",
    create: "Erstellen",
    api_keys: "API-Schlüssel",
    delete_heading: "Mein Konto löschen",
    delete_warning: &[
        Text("Diese Aktion löscht Ihr Konto "),
        Strong("endgültig"),
        Text(". Eine Wiederherstellung ist nicht möglich."),
    ],
    delete_type: "Geben Sie ",
    delete_word: "LÖSCHEN",
    delete_to_confirm: " ein, um zu bestätigen",
    delete_forever: "Endgültig löschen",
    unlink: "Trennen",
    link: "Verknüpfen",
    session_expired: "Ihre Sitzung ist abgelaufen.",
    sign_in_again: "Erneut anmelden",
    keys_intro: "Ein Schlüssel gibt Zugang zur öffentlichen API, also zur gleichen Suche wie der MCP-Connector, über REST. Er ist kostenlos und kann jederzeit widerrufen werden.",
    keys_docs: "Dokumentation ansehen",
    key_name_placeholder: "Name des Schlüssels (optional)",
    create_key: "Schlüssel erstellen",
    copy_key_now: "Kopieren Sie diesen Schlüssel jetzt: Er wird nicht mehr angezeigt.",
    key_failed: "Der Schlüssel konnte nicht verarbeitet werden: ",
    revoke: "Widerrufen",
    default_key_label,
};

fn default_key_label(day: u32, month: u32, year: u32) -> String {
    format!("Schlüssel vom {day:02}.{month:02}.{year}")
}

fn done(code: &str) -> &'static str {
    match code {
        "nom" => "Profil aktualisiert.",
        "google" => "Google-Konto verknüpft.",
        "microsoft" => "Microsoft-Konto verknüpft.",
        "deliaison" => "Anmeldemethode entfernt.",
        "email" => "E-Mail-Adresse aktualisiert.",
        _ => "Erledigt.",
    }
}

fn sent(code: &str) -> &'static str {
    match code {
        "password" => "E-Mail gesendet: Öffnen Sie den Link darin, um das Passwort festzulegen.",
        "email_change" => {
            "E-Mail an Ihre aktuelle Adresse gesendet: Bestätigen Sie darin die beantragte Änderung."
        }
        _ => "E-Mail gesendet.",
    }
}
