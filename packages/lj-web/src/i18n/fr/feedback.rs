use crate::i18n::FeedbackUi;
use crate::i18n::Run::{Local, Text};

pub const FEEDBACK: FeedbackUi = FeedbackUi {
    title: "Signaler un problème - LibreJustice",
    heading: "Signaler un problème",
    intro: &[
        Text("Une page qui ne marche pas, une recherche qui déraille, une donnée manquante : dites-le ici. Le texte des décisions et des lois est repris tel quel de "),
        Local("/sources", "la source"),
        Text(", qui est seule à pouvoir corriger une erreur dedans."),
    ],
    subject: "De quoi s'agit-il ?",
    subjects: [
        "Problème technique sur le site",
        "Donnée manquante ou incomplète",
        "Compte, connexion, connecteur",
        "Autre",
    ],
    page: "Page concernée",
    message: "Votre message",
    email: "Votre e-mail (facultatif)",
    email_placeholder: "vous@exemple.fr",
    email_hint: "Pour qu'on puisse vous répondre. Si vous êtes connecté, c'est l'adresse de votre compte qui est utilisée.",
    send: "Envoyer",
    sent_heading: "Message envoyé",
    sent_body: "Merci. Si vous avez laissé votre adresse, on vous répond.",
    back_to_page: "Revenir à la page",
    error,
};

fn error(code: &str) -> &'static str {
    match code {
        "message_trop_court" => "Décrivez le problème en quelques mots de plus.",
        "message_trop_long" => "Message trop long : 4 000 caractères au maximum.",
        "adresse_invalide" => "Cette adresse e-mail n'est pas valide.",
        "trop_de_signalements" => "Trop de signalements coup sur coup. Réessayez dans une minute.",
        "envoi_impossible" => {
            "Votre message n'est pas parti. Écrivez-nous à contact@librejustice.fr."
        }
        _ => "Votre message n'a pas pu être envoyé.",
    }
}
