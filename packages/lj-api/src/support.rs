//! Signalement d'un problème : un formulaire du site, un mail dans la boîte.
//!
//! Rien n'est stocké — la boîte est le stockage, et le `Reply-To` adresse son
//! auteur. Succès comme erreur rebondissent en 303 sur `/feedback`.
//!
//! Le contexte est en en-tête : répondre cite le corps, jamais les en-têtes.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::Router;
use lettre::message::header::{ContentType, Header, HeaderName, HeaderValue};
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;

use crate::auth::OptionalUser;
use crate::oidc_auth::destination_interne;
use crate::ratelimit::Verdict;
use crate::state::AppState;

/// Le plancher écarte le « ça marche pas », qui ne se traite pas.
const MESSAGE_MIN: usize = 10;
const MESSAGE_MAX: usize = 4_000;

const CHEMIN_DANS_OBJET_MAX: usize = 120;

/// `Header` porte son nom en fonction associée : un type par en-tête, sans
/// autre voie, et `lettre` n'expose pas sa macro.
macro_rules! entete_texte {
    ($type:ident, $nom:literal) => {
        #[derive(Clone)]
        struct $type(String);

        impl Header for $type {
            fn name() -> HeaderName {
                HeaderName::new_from_ascii_str($nom)
            }

            fn parse(brut: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                Ok(Self(brut.to_owned()))
            }

            fn display(&self) -> HeaderValue {
                HeaderValue::new(Self::name(), self.0.clone())
            }
        }
    };
}

entete_texte!(Page, "X-LJ-Page");
entete_texte!(Session, "X-LJ-Session");

pub fn router() -> Router<AppState> {
    Router::new().route("/signalement", post(signalement))
}

/// Plafond journalier d'émission, toutes origines confondues : un seau par
/// appelant ne protège que du répéteur, celui-ci protège d'une foule.
#[derive(Clone)]
pub struct PlafondJournalier {
    plafond: u32,
    etat: Arc<Mutex<(i64, u32)>>,
}

impl PlafondJournalier {
    pub fn new(plafond: u32) -> Self {
        Self {
            plafond,
            etat: Arc::new(Mutex::new((0, 0))),
        }
    }

    /// Faux quand la journée est consommée.
    pub(crate) fn preleve(&self, maintenant: i64) -> bool {
        let jour = maintenant.div_euclid(86_400);
        let mut etat = self.etat.lock().expect("plafond journalier empoisonné");
        if etat.0 != jour {
            *etat = (jour, 0);
        }
        if etat.1 >= self.plafond {
            return false;
        }
        etat.1 += 1;
        true
    }
}

/// The subjects `/feedback` offers; `Form` refuses any other value.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Sujet {
    Technique,
    Manque,
    Compte,
    Autre,
}

impl Sujet {
    fn libelle(self) -> &'static str {
        match self {
            Sujet::Technique => "Problème technique sur le site",
            Sujet::Manque => "Donnée manquante ou incomplète",
            Sujet::Compte => "Compte, connexion, connecteur MCP",
            Sujet::Autre => "Autre",
        }
    }
}

#[derive(Deserialize)]
pub struct SignalementForm {
    sujet: Sujet,
    #[serde(default)]
    url: String,
    #[serde(default)]
    message: String,
    /// Ignorée d'un connecté, dont l'adresse vient du compte.
    #[serde(default)]
    email: String,
    /// Leurre : hors du flux et sans étiquette, un humain ne le voit pas.
    #[serde(default)]
    site: String,
}

async fn signalement(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    entetes: HeaderMap,
    Form(form): Form<SignalementForm>,
) -> Response {
    let de = destination_interne(Some(form.url.trim()));

    // Le succès, pour qu'un robot à qui l'on annonce son refus ne revienne pas.
    if !form.site.trim().is_empty() {
        tracing::info!(target: LOG_TARGET, "report to the decoy, dropped");
        return rebond(&de, "sent", "1");
    }

    let cle = crate::ratelimit::caller_key(user.as_deref(), &entetes);
    if state
        .budgets
        .take(crate::ratelimit::Cost::Signalement, cle.as_ref())
        .await
        == Verdict::Exhausted
    {
        return rebond(&de, "error", "trop_de_signalements");
    }

    let message = form.message.trim();
    match message.chars().count() {
        0..MESSAGE_MIN => return rebond(&de, "error", "message_trop_court"),
        n if n > MESSAGE_MAX => return rebond(&de, "error", "message_trop_long"),
        _ => {}
    }

    // Un connecté ne redonne pas son adresse : celle du compte est prouvée.
    let auteur: Option<Mailbox> = match &user {
        Some(sub) => crate::me::fetch_profile(&state.pool, sub)
            .await
            .ok()
            .and_then(|profil| profil.email)
            .and_then(|adresse| adresse.parse().ok()),
        None => match form.email.trim() {
            "" => None,
            saisie => match saisie.parse() {
                Ok(adresse) => Some(adresse),
                Err(_) => return rebond(&de, "error", "adresse_invalide"),
            },
        },
    };

    let Some(mailer) = state.mailer.clone() else {
        tracing::error!(target: LOG_TARGET, "report received, relay not configured");
        return rebond(&de, "error", "envoi_impossible");
    };

    let maintenant = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    if !state.signalements_du_jour.preleve(maintenant) {
        tracing::error!(
            target: LOG_TARGET,
            plafond = state.settings.signalements_par_jour,
            "daily report ceiling reached, message not sent: {message}"
        );
        return rebond(&de, "error", "envoi_impossible");
    }

    let mail = match rediger(
        &state.settings.mail_from,
        &state.settings.support_mailbox,
        form.sujet,
        &de,
        message,
        auteur.as_ref(),
        user.is_some(),
    ) {
        Ok(mail) => mail,
        Err(erreur) => {
            tracing::error!(target: LOG_TARGET, "report cannot be composed: {erreur}");
            return rebond(&de, "error", "envoi_impossible");
        }
    };

    if let Err(erreur) = mailer.send(mail).await {
        // Sans ce journal, le signalement disparaît avec la réponse SMTP.
        tracing::error!(
            target: LOG_TARGET,
            "report not delivered ({erreur}), page {de}, author {}, message: {message}",
            auteur.as_ref().map(|a| a.email.to_string()).unwrap_or_else(|| "anonyme".into()),
        );
        return rebond(&de, "error", "envoi_impossible");
    }

    tracing::info!(target: LOG_TARGET, page = %de, "signalement remis");
    rebond(&de, "sent", "1")
}

pub const LOG_TARGET: &str = "lj_api::support";

fn rediger(
    expediteur: &str,
    boite: &str,
    sujet: Sujet,
    page: &str,
    message: &str,
    auteur: Option<&Mailbox>,
    session: bool,
) -> Result<Message, String> {
    let expediteur: Mailbox = expediteur
        .parse()
        .map_err(|e| format!("expéditeur illisible : {e}"))?;
    let boite: Mailbox = boite
        .parse()
        .map_err(|e| format!("boîte de support illisible : {e}"))?;

    let mut bati = Message::builder()
        .from(expediteur)
        .to(boite)
        .subject(format!(
            "[Signalement] {} — {}",
            sujet.libelle(),
            objet_court(page)
        ))
        .header(ContentType::TEXT_PLAIN)
        .header(Page(page.to_owned()))
        .header(Session(if session { "oui" } else { "non" }.to_owned()));
    if let Some(adresse) = auteur {
        bati = bati.reply_to(adresse.clone());
    }
    bati.body(format!("{message}\n"))
        .map_err(|e| format!("message irrecevable : {e}"))
}

fn objet_court(page: &str) -> String {
    match page.char_indices().nth(CHEMIN_DANS_OBJET_MAX) {
        None => page.to_string(),
        Some((coupe, _)) => format!("{}…", &page[..coupe]),
    }
}

/// 303 vers `/feedback`, la page d'origine conservée pour le lien de retour.
fn rebond(de: &str, cle: &str, valeur: &str) -> Response {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair(cle, valeur)
        .append_pair("from", de)
        .finish();
    Redirect::to(&format!("/feedback?{query}")).into_response()
}

/// `None` quand le relais n'est pas configuré. Aucun TLS : le lien qui y mène
/// est déjà chiffré sous lui, et le relais n'annonce pas `STARTTLS`.
pub fn transport(settings: &crate::config::Settings) -> Option<AsyncSmtpTransport<Tokio1Executor>> {
    if settings.smtp_host.is_empty()
        || settings.smtp_user.is_empty()
        || settings.smtp_password.is_empty()
    {
        return None;
    }
    let identifiants = lettre::transport::smtp::authentication::Credentials::new(
        settings.smtp_user.clone(),
        settings.smtp_password.clone(),
    );
    Some(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.smtp_host)
            .port(settings.smtp_port)
            .credentials(identifiants)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_plafond_journalier_se_vide_puis_se_rouvre_le_lendemain() {
        let plafond = PlafondJournalier::new(2);
        assert!(plafond.preleve(0));
        assert!(plafond.preleve(3_600));
        assert!(!plafond.preleve(7_200));
        assert!(plafond.preleve(86_400));
    }

    #[test]
    fn une_adresse_a_saut_de_ligne_ne_fait_pas_un_reply_to() {
        // Sans ce refus, un formulaire anonyme injecte un `Bcc:` et le relais
        // devient un relais à spam signé de notre domaine.
        for saisie in [
            "a@b.fr\r\nBcc: victime@ailleurs.tld",
            "a@b.fr\nBcc: victime@ailleurs.tld",
            "pas une adresse",
            "",
        ] {
            assert!(saisie.parse::<Mailbox>().is_err(), "{saisie:?}");
        }
        assert!("marie.dupont@exemple.fr".parse::<Mailbox>().is_ok());
    }

    #[test]
    fn le_contexte_est_en_entete_et_le_corps_ne_porte_que_le_message() {
        let auteur: Mailbox = "marie.dupont@exemple.fr".parse().unwrap();
        let mail = rediger(
            "LibreJustice <contact@librejustice.fr>",
            "contact@librejustice.fr",
            Sujet::Manque,
            "/recherche?q=bail&page=2",
            "Le troisieme resultat n'a aucun rapport.",
            Some(&auteur),
            true,
        )
        .expect("message redigeable");
        let brut = String::from_utf8(mail.formatted()).expect("mail en UTF-8");
        let (entetes, corps) = brut.split_once("\r\n\r\n").expect("corps separe");

        assert!(
            entetes.contains("X-LJ-Page: /recherche?q=bail&page=2"),
            "{entetes}"
        );
        assert!(entetes.contains("X-LJ-Session: oui"), "{entetes}");
        assert!(entetes.contains("marie.dupont@exemple.fr"), "{entetes}");

        // Répondre cite le corps : ni la page ni l'adresse n'y sont.
        assert!(corps.contains("Le troisieme resultat"), "{corps}");
        assert!(!corps.contains("/recherche"), "{corps}");
        assert!(!corps.contains("exemple.fr"), "{corps}");
    }

    #[test]
    fn l_objet_tronque_un_chemin_trop_long() {
        let court = "/decision/a6eqTnpOTXdd";
        assert_eq!(objet_court(court), court);
        let long = format!("/texte/{}", "a".repeat(200));
        let coupe = objet_court(&long);
        assert!(coupe.ends_with('…'));
        assert_eq!(coupe.chars().count(), CHEMIN_DANS_OBJET_MAX + 1);
    }
}
