use crate::i18n::AccountUi;
use crate::i18n::Run::{Local, Text};

pub const ACCOUNT: AccountUi = AccountUi {
    auth_error,
    unknown_error: "Erreur inconnue.",
    password_hint: "Au moins 8 caractères, dont une minuscule, une majuscule et un chiffre.",
    login_title: "Connexion - LibreJustice",
    login: "Connexion",
    create_account: "Créer un compte",
    login_intro: "Accédez à l'ensemble des fonctionnalités LibreJustice.",
    or: "ou",
    email: "E-mail",
    password: "Mot de passe",
    forgot_password: "Mot de passe oublié ?",
    create: "Créer le compte",
    sign_in: "Se connecter",
    email_link: "Recevoir un lien de connexion par e-mail",
    resend_label: "Renvoyer l'e-mail de confirmation",
    resend: "Renvoyer l'e-mail",
    to_sign_in: "Déjà un compte ? Se connecter",
    to_sign_up: "Pas encore de compte ? Créer",
    terms_notice: &[
        Text("En continuant, vous acceptez les "),
        Local("/terms", "conditions d'utilisation"),
        Text(" et prenez connaissance de la "),
        Local("/privacy", "politique de confidentialité"),
        Text("."),
    ],
    report_problem: "Un problème ? Le signaler",
    forgot_heading: "Mot de passe oublié",
    forgot_intro: "Recevez par e-mail un lien de réinitialisation.",
    send_link: "Envoyer le lien",
    back_to_login: "Retour à la connexion",
    link_heading: "Connexion par e-mail",
    link_intro: "Recevez un lien qui ouvre votre session. Sans mot de passe à retenir.",
    sent_heading: "E-mail envoyé",
    sent_recovery: "Si un compte existe pour cette adresse, un lien de réinitialisation vous a été envoyé. Ouvrez-le sur ce navigateur.",
    sent_link: "Un lien de connexion vous a été envoyé. Ouvrez-le sur ce navigateur : il vaut mot de passe, une fois et pour une heure.",
    sent_confirmation: "Vérifiez votre boîte mail et cliquez sur le lien de confirmation pour activer votre compte.",
    continue_google: "Continuer avec Google",
    continue_microsoft: "Continuer avec Microsoft",
    link_invalid: "Lien incomplet ou invalide.",
    link_spent: "Ce lien a peut-être expiré ou déjà été utilisé. ",
    new_link: "Demander un nouveau lien",
    link_opens: "Ce lien ouvre votre session sur ce navigateur.",
    sign_me_in: "Me connecter",
    reset_title: "Réinitialiser le mot de passe - LibreJustice",
    reset_heading: "Réinitialiser le mot de passe",
    link_incomplete: "Lien incomplet : ouvrez celui reçu par courriel.",
    new_password: "Nouveau mot de passe",
    confirm_password: "Confirmer le mot de passe",
    update: "Mettre à jour",
    email_change_title: "Changement d'adresse - LibreJustice",
    email_change_heading: "Changement d'adresse",
    back_to_profile: "Retour au profil",
    email_change_sent: "Demande confirmée. Un lien de vérification vient de partir vers la nouvelle adresse : ouvrez-le pour finaliser le changement.",
    email_change_intro: "Confirmez cette étape du changement d'adresse de votre compte.",
    confirm: "Confirmer",
    confirm_title: "Confirmation - LibreJustice",
    confirmation: "Confirmation",
    confirm_intro: "Confirmez votre adresse pour activer votre compte et ouvrir votre session.",
    confirm_address: "Confirmer mon adresse",
    authorize_title: "Autoriser l'accès - LibreJustice",
    authorize_heading: "Autoriser l'accès",
    authorize_asks: " demande l'accès en lecture à LibreJustice en votre nom.",
    access_granted: "Accès accordé :",
    grants: &[
        "Recherche dans les décisions de justice",
        "Lecture du texte intégral des décisions",
        "Votre adresse e-mail",
    ],
    authorize: "Autoriser",
    deny: "Refuser",
    external_app: "Une application externe",
};

fn auth_error(code: &str) -> Option<&'static str> {
    Some(match code {
        "auth_required" => "E-mail ou mot de passe incorrect.",
        "mot_de_passe_trop_faible" => {
            "Mot de passe trop faible : au moins 8 caractères, dont une minuscule, une majuscule et un chiffre."
        }
        "mot_de_passe_trop_long" => "Mot de passe trop long (72 octets au plus).",
        "mots_de_passe_differents" => "Les mots de passe ne correspondent pas.",
        "email_invalide" => "Adresse e-mail invalide.",
        "email_jetable" => "Les adresses jetables ne sont pas autorisées.",
        "lien_invalide_ou_expire" => "Ce lien a expiré ou a déjà servi. Demandez-en un nouveau.",
        "mail_non_configure" | "envoi_refuse" => {
            "L'envoi du courriel a échoué. Réessayez dans un instant."
        }
        "session_rejected" => "Votre session a expiré. Reconnectez-vous.",
        "dernier_acces" => {
            "Impossible de retirer le dernier moyen de connexion : ajoutez d'abord un mot de passe ou une autre connexion."
        }
        "meme_adresse" => "C'est déjà l'adresse de votre compte.",
        "email_deja_utilise" => {
            "Cette adresse est déjà utilisée par un autre compte. Connectez-vous à ce compte, puis liez cette connexion depuis votre profil."
        }
        "google_deja_lie" => "Ce compte Google est déjà lié à un autre compte.",
        "microsoft_deja_lie" => "Ce compte Microsoft est déjà lié à un autre compte.",
        "google_sans_email" | "microsoft_sans_email" => {
            "Ce compte n'expose pas d'adresse e-mail : impossible de créer un compte LibreJustice avec."
        }
        "id_token_invalide" | "etat_invalide" => "La connexion n'a pas abouti. Réessayez.",
        "tour_expire" => "Cette connexion a expiré ou a déjà servi. Relancez-la.",
        "google_non_configure" | "microsoft_non_configure" => {
            "Cette connexion n'est pas disponible pour le moment."
        }
        "sans_email" => "Votre compte n'a pas d'adresse e-mail.",
        "nom_trop_long" => "Nom affiché trop long (80 caractères au plus).",
        "confirmation_suppression" => "Tapez « SUPPRIMER » pour confirmer la suppression.",
        "email_not_confirmed" => {
            "E-mail non confirmé. Cliquez sur « Renvoyer l'e-mail » pour recevoir un nouveau lien."
        }
        "access_denied" => "Accès refusé.",
        _ => return None,
    })
}
