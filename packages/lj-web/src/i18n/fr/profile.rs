use crate::i18n::ProfileUi;
use crate::i18n::Run::{Strong, Text};

pub const PROFILE: ProfileUi = ProfileUi {
    title: "Profil - LibreJustice",
    heading: "Profil",
    done,
    sent,
    identity: "Identité",
    email_address: "Adresse e-mail",
    display_name: "Nom affiché",
    anonymous: "Anonyme",
    save: "Enregistrer",
    change_email: "Changer d'adresse e-mail : un lien de confirmation part vers l'adresse actuelle, puis vers la nouvelle",
    new_email_placeholder: "nouvelle@adresse.fr",
    request_change: "Demander le changement",
    connections: "Connexions",
    password: "Mot de passe",
    change: "Changer",
    create: "Créer",
    api_keys: "Clés d'API",
    delete_heading: "Supprimer mon compte",
    delete_warning: &[
        Text("Cette action supprime "),
        Strong("définitivement"),
        Text(" votre compte. Aucune restauration n'est possible."),
    ],
    delete_type: "Tapez ",
    delete_word: "SUPPRIMER",
    delete_to_confirm: " pour confirmer",
    delete_forever: "Supprimer définitivement",
    unlink: "Délier",
    link: "Lier",
    session_expired: "Votre session a expiré.",
    sign_in_again: "Se reconnecter",
    keys_intro: "Une clé donne accès à l'API publique, la même recherche que le connecteur MCP, en REST. Elle est gratuite et se révoque à tout moment.",
    keys_docs: "Voir la documentation",
    key_name_placeholder: "Nom de la clé (facultatif)",
    create_key: "Créer une clé",
    copy_key_now: "Copiez cette clé maintenant : elle ne sera plus affichée.",
    key_failed: "La clé n'a pas pu être traitée : ",
    revoke: "Révoquer",
    default_key_label,
};

fn default_key_label(day: u32, month: u32, year: u32) -> String {
    format!("Clé du {day:02}/{month:02}/{year}")
}

fn done(code: &str) -> &'static str {
    match code {
        "nom" => "Profil mis à jour.",
        "google" => "Compte Google lié.",
        "microsoft" => "Compte Microsoft lié.",
        "deliaison" => "Connexion retirée.",
        "email" => "Adresse e-mail mise à jour.",
        _ => "C'est fait.",
    }
}

fn sent(code: &str) -> &'static str {
    match code {
        "password" => "E-mail envoyé : ouvrez le lien qu'il contient pour choisir le mot de passe.",
        "email_change" => {
            "E-mail envoyé à votre adresse actuelle : confirmez-y la demande de changement."
        }
        _ => "E-mail envoyé.",
    }
}
