//! Le compte vu du profil : résumé, nom affiché, moyens d'accès —
//! identités liées, mot de passe —, changement d'adresse, suppression.
//!
//! Tout est formulaire HTML → 303 vers `/profile`, l'issue en query (`done=`,
//! `sent=`, `error=`). Deux invariants : un compte garde toujours au moins un
//! moyen d'accès (la déliaison du dernier est refusée), et un mot de passe ne
//! s'écrit que prouvé par un lien de mail — jamais depuis un formulaire de
//! session, qu'une session volée suffirait à soumettre.

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use lj_dtos::{AccountSummary, LinkedIdentity};
use serde::Deserialize;

use crate::accounts::{lien_mail, normaliser};
use crate::auth::RequiredUser;
use crate::disposable::{disposable_domain, log_refusal};
use crate::error::{ApiError, Result};
use crate::me::ts_to_rfc3339;
use crate::state::AppState;
use crate::{mail, me, session, usage};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_account))
        .route("/display-name", post(display_name))
        .route("/password-link", post(password_link))
        .route("/unlink", post(unlink))
        .route("/email-change", post(email_change_request))
        .route("/delete", post(delete))
}

/// Résumé du compte : identité, moyens d'accès.
pub async fn resume(state: &AppState, sub: &str) -> Result<AccountSummary> {
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "SELECT email, display_name, password_hash IS NOT NULL AS a_mot_de_passe \
               FROM users WHERE sub = $1",
            &[&sub],
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    let identites = conn
        .query(
            "SELECT provider, provider_sub, linked_at FROM user_identities \
              WHERE user_sub = $1 ORDER BY linked_at",
            &[&sub],
        )
        .await?;
    Ok(AccountSummary {
        email: ligne.get("email"),
        display_name: ligne.get("display_name"),
        has_password: ligne.get("a_mot_de_passe"),
        identities: identites
            .into_iter()
            .map(|l| LinkedIdentity {
                provider: l.get("provider"),
                provider_sub: l.get("provider_sub"),
                linked_at: ts_to_rfc3339(l.get("linked_at")),
            })
            .collect(),
    })
}

async fn get_account(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    Ok(Json(resume(&state, &sub).await?).into_response())
}

/// 303 vers `/profile`, l'issue en query.
fn retour_profil(cle: &str, valeur: &str) -> Response {
    Redirect::to(&format!(
        "/profile?{}",
        crate::oauth::form_urlencode(&[(cle, valeur)])
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct DisplayNameForm {
    #[serde(default)]
    display_name: String,
}

/// Nom affiché. Vide = anonyme (NULL).
async fn display_name(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Form(req): Form<DisplayNameForm>,
) -> Result<Response> {
    let nom = req.display_name.trim();
    if nom.chars().count() > 80 {
        return Ok(retour_profil("error", "nom_trop_long"));
    }
    let nom = (!nom.is_empty()).then_some(nom);
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET display_name = $1 WHERE sub = $2",
        &[&nom, &sub],
    )
    .await?;
    Ok(retour_profil("done", "nom"))
}

/// Envoie à l'adresse du compte le lien qui définit ou remplace le mot de passe :
/// le flux de réinitialisation, qui prouve la boîte avant d'écrire.
async fn password_link(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    entetes: HeaderMap,
) -> Result<Response> {
    let conn = state.pool.get().await?;
    let email: Option<String> = conn
        .query_one("SELECT email FROM users WHERE sub = $1", &[&sub])
        .await?
        .get("email");
    let Some(email) = email else {
        return Ok(retour_profil("error", "sans_email"));
    };
    let lien = lien_mail(&state, &sub, "recovery", "/reset-password", None).await?;
    let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
    mail::send_recovery(&state, &email, &lien, voie).await?;
    Ok(retour_profil("sent", "password"))
}

#[derive(Debug, Deserialize)]
pub struct UnlinkForm {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    provider_sub: String,
}

/// Délie une identité. Garde : jamais le dernier moyen d'accès — sans mot de
/// passe ni autre identité, le compte deviendrait injoignable.
async fn unlink(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Form(req): Form<UnlinkForm>,
) -> Result<Response> {
    let conn = state.pool.get().await?;
    let supprime = conn
        .execute(
            "DELETE FROM user_identities \
              WHERE user_sub = $1 AND provider = $2 AND provider_sub = $3 \
                AND (EXISTS (SELECT 1 FROM users u \
                              WHERE u.sub = $1 AND u.password_hash IS NOT NULL) \
                     OR EXISTS (SELECT 1 FROM user_identities a \
                                 WHERE a.user_sub = $1 \
                                   AND (a.provider, a.provider_sub) <> ($2, $3)))",
            &[&sub, &req.provider, &req.provider_sub],
        )
        .await?;
    if supprime == 0 {
        return Ok(retour_profil("error", "dernier_acces"));
    }
    Ok(retour_profil("done", "deliaison"))
}

#[derive(Debug, Deserialize)]
pub struct EmailChangeRequestForm {
    #[serde(default)]
    email: String,
}

/// Demande de changement d'adresse : premier jeton, envoyé à l'adresse
/// actuelle. La nouvelle adresse n'est pas vérifiée ici — dire
/// qu'elle est prise en ferait un oracle ; le refus tombe à l'écriture finale.
async fn email_change_request(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    entetes: HeaderMap,
    Form(req): Form<EmailChangeRequestForm>,
) -> Result<Response> {
    let nouvelle = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => return Ok(retour_profil("error", "email_invalide")),
    };
    if let Some(listed) = disposable_domain(&state, &nouvelle).await? {
        log_refusal(&listed, "email_change");
        return Ok(retour_profil("error", "email_jetable"));
    }
    let conn = state.pool.get().await?;
    let actuelle: Option<String> = conn
        .query_one("SELECT email FROM users WHERE sub = $1", &[&sub])
        .await?
        .get("email");
    let Some(actuelle) = actuelle else {
        return Ok(retour_profil("error", "sans_email"));
    };
    if actuelle.eq_ignore_ascii_case(&nouvelle) {
        return Ok(retour_profil("error", "meme_adresse"));
    }
    let lien = lien_mail(
        &state,
        &sub,
        "email_change_ancienne",
        "/email-change",
        Some(&nouvelle),
    )
    .await?;
    let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
    mail::send_email_change_ancienne(&state, &actuelle, &nouvelle, &lien, voie).await?;
    Ok(retour_profil("sent", "email_change"))
}

#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    #[serde(default)]
    confirm: String,
}

/// Suppression définitive. Le mot de confirmation se vérifie ici, pas seulement
/// à l'écran ; les sessions tombent avec la ligne (cascade), le cookie s'efface
/// dans la même réponse.
async fn delete(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Form(req): Form<DeleteForm>,
) -> Result<Response> {
    if !lj_dtos::ACCOUNT_DELETE_WORDS.contains(&req.confirm.trim()) {
        return Ok(retour_profil("error", "confirmation_suppression"));
    }
    me::delete_me(&state, &sub).await?;
    let mut resp = Redirect::to("/").into_response();
    for effacement in session::clear_cookies() {
        let (nom, valeur) = session::header(effacement)?;
        resp.headers_mut().append(nom, valeur);
    }
    Ok(resp)
}
