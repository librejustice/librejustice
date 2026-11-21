use crate::i18n::AccountUi;
use crate::i18n::Run::{Local, Text};

pub const ACCOUNT: AccountUi = AccountUi {
    auth_error,
    unknown_error: "Onbekende fout.",
    password_hint: "Minstens 8 tekens, waaronder een kleine letter, een hoofdletter en een cijfer.",
    login_title: "Inloggen - LibreJustice",
    login: "Inloggen",
    create_account: "Account aanmaken",
    login_intro: "Krijg toegang tot alle functies van LibreJustice.",
    or: "of",
    email: "E-mailadres",
    password: "Wachtwoord",
    forgot_password: "Wachtwoord vergeten?",
    create: "Account aanmaken",
    sign_in: "Inloggen",
    email_link: "Een inloglink per e-mail ontvangen",
    resend_label: "De bevestigingsmail opnieuw verzenden",
    resend: "E-mail opnieuw verzenden",
    to_sign_in: "Al een account? Inloggen",
    to_sign_up: "Nog geen account? Aanmaken",
    terms_notice: &[
        Text("Door verder te gaan, aanvaardt u de "),
        Local("/terms", "gebruiksvoorwaarden"),
        Text(" en neemt u kennis van het "),
        Local("/privacy", "privacybeleid"),
        Text("."),
    ],
    report_problem: "Een probleem? Meld het",
    forgot_heading: "Wachtwoord vergeten",
    forgot_intro: "Ontvang per e-mail een link om uw wachtwoord opnieuw in te stellen.",
    send_link: "Link verzenden",
    back_to_login: "Terug naar inloggen",
    link_heading: "Inloggen per e-mail",
    link_intro: "Ontvang een link waarmee u inlogt. Zonder wachtwoord om te onthouden.",
    sent_heading: "E-mail verzonden",
    sent_recovery: "Als er een account bestaat voor dit adres, werd er een link verzonden om het wachtwoord opnieuw in te stellen. Open hem in deze browser.",
    sent_link: "We hebben u een inloglink gestuurd. Open hem in deze browser: hij geldt als wachtwoord, één keer en een uur lang.",
    sent_confirmation: "Kijk in uw mailbox en klik op de bevestigingslink om uw account te activeren.",
    continue_google: "Doorgaan met Google",
    continue_microsoft: "Doorgaan met Microsoft",
    link_invalid: "Onvolledige of ongeldige link.",
    link_spent: "Deze link is misschien verlopen of al gebruikt. ",
    new_link: "Een nieuwe link aanvragen",
    link_opens: "Deze link logt u in deze browser in.",
    sign_me_in: "Inloggen",
    reset_title: "Wachtwoord opnieuw instellen - LibreJustice",
    reset_heading: "Wachtwoord opnieuw instellen",
    link_incomplete: "Onvolledige link: open de link die u per e-mail ontving.",
    new_password: "Nieuw wachtwoord",
    confirm_password: "Wachtwoord bevestigen",
    update: "Bijwerken",
    email_change_title: "Wijziging van e-mailadres - LibreJustice",
    email_change_heading: "Wijziging van e-mailadres",
    back_to_profile: "Terug naar het profiel",
    email_change_sent: "Aanvraag bevestigd. Er is net een verificatielink naar het nieuwe adres vertrokken: open hem om de wijziging af te ronden.",
    email_change_intro: "Bevestig deze stap in de wijziging van het e-mailadres van uw account.",
    confirm: "Bevestigen",
    confirm_title: "Bevestiging - LibreJustice",
    confirmation: "Bevestiging",
    confirm_intro: "Bevestig uw adres om uw account te activeren en u aan te melden.",
    confirm_address: "Mijn adres bevestigen",
    authorize_title: "Toegang toestaan - LibreJustice",
    authorize_heading: "Toegang toestaan",
    authorize_asks: " vraagt in uw naam leestoegang tot LibreJustice.",
    access_granted: "Toegang die wordt verleend:",
    grants: &[
        "Zoeken in rechterlijke beslissingen",
        "Lezen van de volledige tekst van de beslissingen",
        "Uw e-mailadres",
    ],
    authorize: "Toestaan",
    deny: "Weigeren",
    external_app: "Een externe toepassing",
};

fn auth_error(code: &str) -> Option<&'static str> {
    Some(match code {
        "auth_required" => "E-mailadres of wachtwoord onjuist.",
        "mot_de_passe_trop_faible" => {
            "Wachtwoord te zwak: minstens 8 tekens, waaronder een kleine letter, een hoofdletter en een cijfer."
        }
        "mot_de_passe_trop_long" => "Wachtwoord te lang (maximaal 72 bytes).",
        "mots_de_passe_differents" => "De wachtwoorden komen niet overeen.",
        "email_invalide" => "Ongeldig e-mailadres.",
        "email_jetable" => "Wegwerpadressen zijn niet toegestaan.",
        "lien_invalide_ou_expire" => "Deze link is verlopen of al gebruikt. Vraag een nieuwe aan.",
        "mail_non_configure" | "envoi_refuse" => {
            "De e-mail kon niet worden verzonden. Probeer het zo meteen opnieuw."
        }
        "session_rejected" => "Uw sessie is verlopen. Log opnieuw in.",
        "dernier_acces" => {
            "De laatste inlogmethode kan niet worden verwijderd: voeg eerst een wachtwoord of een andere inlogmethode toe."
        }
        "meme_adresse" => "Dit is al het adres van uw account.",
        "email_deja_utilise" => {
            "Dit adres wordt al door een ander account gebruikt. Log in bij dat account en koppel deze inlogmethode vervolgens vanuit uw profiel."
        }
        "google_deja_lie" => "Dit Google-account is al aan een ander account gekoppeld.",
        "microsoft_deja_lie" => "Dit Microsoft-account is al aan een ander account gekoppeld.",
        "google_sans_email" | "microsoft_sans_email" => {
            "Dit account geeft geen e-mailadres vrij: er kan geen LibreJustice-account mee worden aangemaakt."
        }
        "id_token_invalide" | "etat_invalide" => "Het inloggen is niet gelukt. Probeer het opnieuw.",
        "tour_expire" => "Deze inlogpoging is verlopen of al gebruikt. Begin opnieuw.",
        "google_non_configure" | "microsoft_non_configure" => {
            "Deze inlogmethode is momenteel niet beschikbaar."
        }
        "sans_email" => "Uw account heeft geen e-mailadres.",
        "nom_trop_long" => "Weergavenaam te lang (maximaal 80 tekens).",
        "confirmation_suppression" => "Typ het gevraagde woord om de verwijdering te bevestigen.",
        "email_not_confirmed" => {
            "E-mailadres niet bevestigd. Klik op \"E-mail opnieuw verzenden\" om een nieuwe link te ontvangen."
        }
        "access_denied" => "Toegang geweigerd.",
        _ => return None,
    })
}
