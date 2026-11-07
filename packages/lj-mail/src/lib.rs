//! The house style of every mail: one layout, living in one place, and each
//! mail brings only its text. Sending stays with the caller.

const LAYOUT: &str = include_str!("../templates/mail/layout.html");
const NOTICE: &str = include_str!("../templates/mail/notice.html");

/// What a mail brings to the shared layout, subject included. The link travels
/// apart: it alone is built by the server.
pub struct Gabarit {
    pub sujet: String,
    pub corps: String,
    pub notice: String,
    pub bouton: String,
    /// First sentence of the footer. An auth mail is not answered, a newsletter
    /// is unsubscribed from: the house style cannot settle both.
    pub pied: String,
}

/// In the notice and not in the layout, which also renders mail carrying no
/// expiring link.
const VALIDITE: &str = "Lien valable 1 heure.";
/// `noreply@` has no mailbox: without the contact address here, an auth mail
/// would offer no way to reach anyone.
const PIED_AUTH: &str = "Message automatique, ne pas répondre. Pour nous joindre, \
     <a href=\"mailto:contact@librejustice.fr\" \
     style=\"color:#892122;text-decoration:underline;\">contact@librejustice.fr</a>.";

pub fn confirmation() -> Gabarit {
    Gabarit {
        sujet: "Votre inscription à LibreJustice".into(),
        corps: "Un compte LibreJustice a été créé avec cette adresse.".into(),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette inscription, \
             ignorez cet e-mail."
        ),
        bouton: "Confirmer l'adresse".into(),
        pied: PIED_AUTH.into(),
    }
}

pub fn connexion(compte: &str) -> Gabarit {
    Gabarit {
        sujet: "Votre connexion à LibreJustice".into(),
        corps: format!(
            "Ce lien ouvre la session du compte {}.",
            adresse_html(compte)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Personne ne s'est connecté."
        ),
        bouton: "Me connecter".into(),
        pied: PIED_AUTH.into(),
    }
}

pub fn recuperation(compte: &str) -> Gabarit {
    Gabarit {
        sujet: "Votre mot de passe LibreJustice".into(),
        corps: format!(
            "Ce lien définit le mot de passe du compte {}.",
            adresse_html(compte)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Votre compte n'a pas été modifié."
        ),
        bouton: "Choisir un mot de passe".into(),
        pied: PIED_AUTH.into(),
    }
}

pub fn changement_ancienne(nouvelle: &str) -> Gabarit {
    Gabarit {
        sujet: "Changement d'adresse de votre compte LibreJustice".into(),
        corps: format!(
            "Votre compte passerait à l'adresse {}. \
             Un second lien partira ensuite vers cette adresse.",
            adresse_html(nouvelle)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Votre adresse n'a pas été modifiée."
        ),
        bouton: "Confirmer la demande".into(),
        pied: PIED_AUTH.into(),
    }
}

pub fn changement_nouvelle() -> Gabarit {
    Gabarit {
        sujet: "Votre nouvelle adresse LibreJustice".into(),
        corps: "Un compte LibreJustice change d'adresse pour celle-ci. \
                Ce lien termine le changement."
            .into(),
        notice: format!(
            "{VALIDITE} Si ce compte n'est pas le vôtre, ignorez cet e-mail. \
             Rien ne sera modifié."
        ),
        bouton: "Confirmer l'adresse".into(),
        pied: PIED_AUTH.into(),
    }
}

/// A mail rendered without being sent, for proofreading.
pub struct Apercu {
    pub nom: &'static str,
    pub sujet: String,
    pub html: String,
}

/// Every auth mail, rendered with a sample address and link. The rendering comes
/// from here, or proofreading the house style would prove nothing about what is
/// sent.
pub fn apercus(exemple: &str, lien: &str) -> Vec<Apercu> {
    [
        ("Confirmation d'inscription", confirmation()),
        ("Lien de connexion", connexion(exemple)),
        ("Mot de passe", recuperation(exemple)),
        (
            "Changement d'adresse, ancienne",
            changement_ancienne(exemple),
        ),
        ("Changement d'adresse, nouvelle", changement_nouvelle()),
    ]
    .into_iter()
    .map(|(nom, gabarit)| Apercu {
        nom,
        sujet: gabarit.sujet.clone(),
        html: rendre_html(&gabarit, lien),
    })
    .collect()
}

/// Left bare, Gmail turns the address into an underlined blue `mailto:` that
/// competes with the button. An anchor set in
/// advance holds the style: a client does not link what is already a link.
fn adresse_html(brute: &str) -> String {
    let e = echapper(brute);
    format!(
        "<a href=\"mailto:{e}\" \
         style=\"color:#051223;text-decoration:none;font-weight:700;\">{e}</a>"
    )
}

/// The address is repeated in the body and comes from the request, so it is
/// escaped before entering HTML.
fn echapper(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Renders the mail in the shared layout, sending nothing.
pub fn rendre_html(gabarit: &Gabarit, lien: &str) -> String {
    // Empty, the notice block would still render its left rule and margin.
    let notice = if gabarit.notice.is_empty() {
        String::new()
    } else {
        NOTICE.replace("{{NOTICE}}", &gabarit.notice)
    };
    // `{{CORPS}}` last: it alone carries text from the request, so it alone
    // could inject a marker.
    LAYOUT
        .replace("{{SUJET}}", &gabarit.sujet)
        .replace("{{LIEN}}", lien)
        .replace("{{BOUTON}}", &gabarit.bouton)
        .replace("{{NOTICE_BLOC}}", &notice)
        .replace("{{PIED}}", &gabarit.pied)
        .replace("{{CORPS}}", &gabarit.corps)
}
