use crate::i18n::AccountUi;
use crate::i18n::Run::{Local, Text};

pub const ACCOUNT: AccountUi = AccountUi {
    auth_error,
    unknown_error: "Unbekannter Fehler.",
    password_hint: "Mindestens 8 Zeichen, davon ein Kleinbuchstabe, ein Grossbuchstabe und eine Ziffer.",
    login_title: "Anmeldung - LibreJustice",
    login: "Anmeldung",
    create_account: "Registrieren",
    login_intro: "Nutzen Sie alle Funktionen von LibreJustice.",
    or: "oder",
    email: "E-Mail",
    password: "Passwort",
    forgot_password: "Passwort vergessen?",
    create: "Konto erstellen",
    sign_in: "Anmelden",
    email_link: "Anmeldelink per E-Mail erhalten",
    resend_label: "Bestätigungs-E-Mail erneut senden",
    resend: "E-Mail erneut senden",
    to_sign_in: "Schon ein Konto? Anmelden",
    to_sign_up: "Noch kein Konto? Registrieren",
    terms_notice: &[
        Text("Wenn Sie fortfahren, akzeptieren Sie die "),
        Local("/terms", "Nutzungsbedingungen"),
        Text(" und nehmen die "),
        Local("/privacy", "Datenschutzerklärung"),
        Text(" zur Kenntnis."),
    ],
    report_problem: "Ein Problem? Hier melden",
    forgot_heading: "Passwort vergessen",
    forgot_intro: "Erhalten Sie per E-Mail einen Link zum Zurücksetzen.",
    send_link: "Link senden",
    back_to_login: "Zurück zur Anmeldung",
    link_heading: "Anmeldung per E-Mail",
    link_intro: "Erhalten Sie einen Link, der Sie anmeldet. Kein Passwort, das Sie sich merken müssen.",
    sent_heading: "E-Mail gesendet",
    sent_recovery: "Falls für diese Adresse ein Konto besteht, wurde Ihnen ein Link zum Zurücksetzen gesendet. Öffnen Sie ihn in diesem Browser.",
    sent_link: "Ihnen wurde ein Anmeldelink gesendet. Öffnen Sie ihn in diesem Browser: Er gilt als Passwort, einmal und eine Stunde lang.",
    sent_confirmation: "Prüfen Sie Ihr Postfach und klicken Sie auf den Bestätigungslink, um Ihr Konto zu aktivieren.",
    continue_google: "Weiter mit Google",
    continue_microsoft: "Weiter mit Microsoft",
    link_invalid: "Link unvollständig oder ungültig.",
    link_spent: "Dieser Link ist möglicherweise abgelaufen oder wurde bereits verwendet. ",
    new_link: "Neuen Link anfordern",
    link_opens: "Dieser Link meldet Sie in diesem Browser an.",
    sign_me_in: "Mich anmelden",
    reset_title: "Passwort zurücksetzen - LibreJustice",
    reset_heading: "Passwort zurücksetzen",
    link_incomplete: "Link unvollständig: Öffnen Sie den Link aus der E-Mail.",
    new_password: "Neues Passwort",
    confirm_password: "Passwort bestätigen",
    update: "Aktualisieren",
    email_change_title: "Adressänderung - LibreJustice",
    email_change_heading: "Adressänderung",
    back_to_profile: "Zurück zum Profil",
    email_change_sent: "Anfrage bestätigt. Soeben wurde ein Bestätigungslink an die neue Adresse gesendet: Öffnen Sie ihn, um die Änderung abzuschliessen.",
    email_change_intro: "Bestätigen Sie diesen Schritt der Adressänderung Ihres Kontos.",
    confirm: "Bestätigen",
    confirm_title: "Bestätigung - LibreJustice",
    confirmation: "Bestätigung",
    confirm_intro: "Bestätigen Sie Ihre Adresse, um Ihr Konto zu aktivieren und sich anzumelden.",
    confirm_address: "Meine Adresse bestätigen",
    authorize_title: "Zugriff erlauben - LibreJustice",
    authorize_heading: "Zugriff erlauben",
    authorize_asks: " beantragt in Ihrem Namen Lesezugriff auf LibreJustice.",
    access_granted: "Gewährter Zugriff:",
    grants: &[
        "Suche in den Gerichtsentscheiden",
        "Lesen des Volltexts der Entscheide",
        "Ihre E-Mail-Adresse",
    ],
    authorize: "Erlauben",
    deny: "Ablehnen",
    external_app: "Eine externe App",
};

fn auth_error(code: &str) -> Option<&'static str> {
    Some(match code {
        "auth_required" => "E-Mail oder Passwort falsch.",
        "mot_de_passe_trop_faible" => {
            "Passwort zu schwach: mindestens 8 Zeichen, davon ein Kleinbuchstabe, ein Grossbuchstabe und eine Ziffer."
        }
        "mot_de_passe_trop_long" => "Passwort zu lang (höchstens 72 Bytes).",
        "mots_de_passe_differents" => "Die Passwörter stimmen nicht überein.",
        "email_invalide" => "Ungültige E-Mail-Adresse.",
        "email_jetable" => "Wegwerfadressen sind nicht zugelassen.",
        "lien_invalide_ou_expire" => "Dieser Link ist abgelaufen oder wurde bereits verwendet. Fordern Sie einen neuen an.",
        "mail_non_configure" | "envoi_refuse" => {
            "Die E-Mail konnte nicht gesendet werden. Bitte versuchen Sie es gleich erneut."
        }
        "session_rejected" => "Ihre Sitzung ist abgelaufen. Bitte melden Sie sich erneut an.",
        "dernier_acces" => {
            "Die letzte Anmeldemethode kann nicht entfernt werden: Fügen Sie zuerst ein Passwort oder eine andere Anmeldemethode hinzu."
        }
        "meme_adresse" => "Das ist bereits die Adresse Ihres Kontos.",
        "email_deja_utilise" => {
            "Diese Adresse wird bereits von einem anderen Konto verwendet. Melden Sie sich bei diesem Konto an und verknüpfen Sie diese Anmeldemethode dann in Ihrem Profil."
        }
        "google_deja_lie" => "Dieses Google-Konto ist bereits mit einem anderen Konto verknüpft.",
        "microsoft_deja_lie" => "Dieses Microsoft-Konto ist bereits mit einem anderen Konto verknüpft.",
        "google_sans_email" | "microsoft_sans_email" => {
            "Dieses Konto gibt keine E-Mail-Adresse frei: Damit lässt sich kein LibreJustice-Konto erstellen."
        }
        "id_token_invalide" | "etat_invalide" => "Die Anmeldung ist fehlgeschlagen. Bitte versuchen Sie es erneut.",
        "tour_expire" => "Diese Anmeldung ist abgelaufen oder wurde bereits verwendet. Starten Sie sie erneut.",
        "google_non_configure" | "microsoft_non_configure" => {
            "Diese Anmeldemethode ist derzeit nicht verfügbar."
        }
        "sans_email" => "Ihr Konto hat keine E-Mail-Adresse.",
        "nom_trop_long" => "Angezeigter Name zu lang (höchstens 80 Zeichen).",
        "confirmation_suppression" => "Geben Sie «LÖSCHEN» ein, um die Löschung zu bestätigen.",
        "email_not_confirmed" => {
            "E-Mail-Adresse nicht bestätigt. Klicken Sie auf «E-Mail erneut senden», um einen neuen Link zu erhalten."
        }
        "access_denied" => "Zugriff verweigert.",
        _ => return None,
    })
}
