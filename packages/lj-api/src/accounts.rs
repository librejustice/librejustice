//! Comptes et connexion, chez nous.
//!
//! Inscription, confirmation, connexion, déconnexion, réinitialisation. Tout est
//! formulaire HTML → 303 : une erreur attendue rebondit sur `/login` avec son
//! code en query — jamais l'adresse, qui ne se relit pas dans une URL. Le mot de
//! passe n'existe en clair que dans le corps de la requête ; en base, seul son
//! haché argon2id. Aucune structure qui le porte ne dérive `Debug` : un
//! `{form:?}` posé un jour sur un chemin d'erreur l'expédierait aux logs.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;

use crate::disposable::{disposable_domain, log_refusal};
use crate::error::{ApiError, Result};
use crate::oauth::{form_urlencode, random_bytes, sha256};
use crate::oidc_auth::destination_interne;
use crate::state::AppState;
use crate::{locale, mail, session, usage};

/// Durée de vie d'un lien de mail.
const LIEN_MINUTES: i32 = 60;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", post(signup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/confirm", post(confirm))
        .route("/resend", post(resend))
        .route("/link-request", post(link_request))
        .route("/link", post(link))
        .route("/reset-request", post(reset_request))
        .route("/reset", post(reset))
        .route("/email-change", post(email_change))
}

/// Formulaire d'inscription posté par `/login?mode=signup`.
#[derive(Deserialize)]
pub struct SignupForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    next: Option<String>,
}

/// Inscription. Le compte existe dès l'appel mais n'est pas confirmé, donc il ne
/// peut pas se connecter — c'est le lien du mail qui le confirme. Formulaire →
/// 303 vers l'écran « email envoyé », adresse neuve ou déjà prise : dire « ce
/// compte existe » ferait de l'inscription un oracle d'adresses.
async fn signup(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<SignupForm>,
) -> Result<Response> {
    let next = req.next.as_deref();
    let rebond = |code: &str| Ok(rebond_login(&[("mode", "signup"), ("error", code)], next));
    let email = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => return rebond("email_invalide"),
    };
    if let Some(listed) = disposable_domain(&state, &email).await? {
        log_refusal(&listed, "signup");
        return rebond("email_jetable");
    }
    match verifier_robustesse(&req.password) {
        Ok(()) => {}
        Err(ApiError::BadRequest(code)) => return rebond(&code),
        Err(e) => return Err(e),
    }
    let hash = hacher(&req.password)?;

    let conn = state.pool.get().await?;
    // Le prédicat de l'index partiel fait partie de la cible du conflit : sans
    // lui, Postgres ne sait pas quel index arbitre (42P10).
    let ligne = conn
        .query_opt(
            "INSERT INTO users (sub, email, password_hash) \
             VALUES (gen_random_uuid()::text, $1, $2) \
             ON CONFLICT (lower(email)) WHERE email IS NOT NULL DO NOTHING \
             RETURNING sub",
            &[&email, &hash],
        )
        .await?;

    let envoye = || Redirect::to("/login?sent=confirmation").into_response();
    // Un compte encore non confirmé reçoit tout de même son lien : c'est le cas
    // de qui s'inscrit deux fois sans avoir vu le premier mail.
    let sub = match ligne {
        Some(ligne) => ligne.get::<_, String>("sub"),
        None => {
            let attente = conn
                .query_opt(
                    "SELECT sub FROM users \
                      WHERE lower(email) = lower($1) AND email_confirmed_at IS NULL",
                    &[&email],
                )
                .await?;
            match attente {
                Some(l) => l.get::<_, String>("sub"),
                None => return Ok(envoye()),
            }
        }
    };
    locale::retenir(&state, &sub, &locale::du_navigateur(&entetes), None).await?;
    let lien = lien_mail(&state, &sub, "confirmation", "/confirm", None).await?;
    let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
    mail::send_confirmation(&state, &email, &lien, voie).await?;
    Ok(envoye())
}

/// Formulaire de connexion posté par `/login`.
#[derive(Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    next: Option<String>,
}

/// Connexion par mot de passe. Formulaire → 303 vers `next` (chemin interne)
/// avec le cookie de session ; un refus rebondit sur `/login` avec son code.
async fn login(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<LoginForm>,
) -> Result<Response> {
    let next = req.next.as_deref();
    let email = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => return Ok(rebond_login(&[("error", "email_invalide")], next)),
    };
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "SELECT sub, password_hash, email_confirmed_at IS NOT NULL AS confirme \
               FROM users WHERE lower(email) = lower($1)",
            &[&email],
        )
        .await?;

    // Compte inconnu, sans mot de passe (entré par Google, il n'en a jamais eu)
    // ou mot de passe faux : le même code pour les trois, sinon la connexion
    // devient un oracle d'adresses.
    let refus = || Ok(rebond_login(&[("error", "auth_required")], next));
    let Some(ligne) = ligne else {
        return refus();
    };
    let hash: Option<String> = ligne.get("password_hash");
    let Some(hash) = hash else {
        return refus();
    };
    if !verifier(&req.password, &hash) {
        return refus();
    }
    if !ligne.get::<_, bool>("confirme") {
        return Ok(rebond_login(&[("error", "email_not_confirmed")], next));
    }

    let sub: String = ligne.get("sub");
    // Le clair n'est sous la main qu'ici : c'est le seul moment où un haché
    // hérité de Supabase peut passer à argon2id.
    if !hash.starts_with("$argon2") {
        let neuf = hacher(&req.password)?;
        conn.execute(
            "UPDATE users SET password_hash = $2 WHERE sub = $1",
            &[&sub, &neuf],
        )
        .await?;
    }
    locale::retenir(&state, &sub, &locale::du_navigateur(&entetes), None).await?;
    let token = session::open(&state, &sub).await?;
    let mut resp = Redirect::to(&destination_interne(next)).into_response();
    let (nom, valeur) = session::header(session::set_cookie(&token))?;
    resp.headers_mut().append(nom, valeur);
    Ok(resp)
}

/// Déconnexion de ce navigateur. Les autres sessions du compte survivent.
async fn logout(State(state): State<AppState>, jar: Cookies) -> Result<Response> {
    if let Some(token) = jar.0 {
        session::close(&state, &token).await?;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    for effacement in session::clear_cookies() {
        let (nom, valeur) = session::header(effacement)?;
        resp.headers_mut().append(nom, valeur);
    }
    Ok(resp)
}

/// Formulaire de confirmation posté par `/confirm` (HTML pur, sans JS).
#[derive(Debug, Deserialize)]
pub struct ConfirmForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    next: Option<String>,
}

/// Confirmation d'inscription : le lien du mail vaut preuve de l'adresse, donc
/// elle ouvre aussi la session. Formulaire → 303 vers `next` (chemin interne) ;
/// un jeton mort rebondit sur la page avec son code. La consommation exige le
/// POST du bouton, jamais le GET du lien — que les scanners de boîtes suivent.
async fn confirm(State(state): State<AppState>, Form(req): Form<ConfirmForm>) -> Result<Response> {
    let sub = match consommer_jeton(&state, &req.token, "confirmation").await {
        Ok((sub, _)) => sub,
        Err(ApiError::BadRequest(_)) => {
            return Ok(Redirect::to("/confirm?error=lien_invalide_ou_expire").into_response())
        }
        Err(e) => return Err(e),
    };
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET email_confirmed_at = coalesce(email_confirmed_at, now()) WHERE sub = $1",
        &[&sub],
    )
    .await?;
    let token = session::open(&state, &sub).await?;
    let mut resp = Redirect::to(&destination_interne(req.next.as_deref())).into_response();
    let (nom, valeur) = session::header(session::set_cookie(&token))?;
    resp.headers_mut().append(nom, valeur);
    Ok(resp)
}

/// Renvoi de confirmation et demande de réinitialisation : l'adresse seule.
#[derive(Debug, Deserialize)]
pub struct EmailForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    next: Option<String>,
}

/// Renvoi du mail de confirmation. Réponse identique que le compte existe ou non.
async fn resend(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<EmailForm>,
) -> Result<Response> {
    let email = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => {
            return Ok(rebond_login(
                &[("error", "email_invalide")],
                req.next.as_deref(),
            ))
        }
    };
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "SELECT sub FROM users \
              WHERE lower(email) = lower($1) AND email_confirmed_at IS NULL",
            &[&email],
        )
        .await?;
    if let Some(ligne) = ligne {
        let sub: String = ligne.get("sub");
        let lien = lien_mail(&state, &sub, "confirmation", "/confirm", None).await?;
        let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
        mail::send_confirmation(&state, &email, &lien, voie).await?;
    }
    Ok(Redirect::to("/login?sent=confirmation").into_response())
}

async fn link_request(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<EmailForm>,
) -> Result<Response> {
    let next = req.next.as_deref();
    let email = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => {
            return Ok(rebond_login(
                &[("mode", "link"), ("error", "email_invalide")],
                next,
            ))
        }
    };

    let conn = state.pool.get().await?;
    // An account already on a disposable domain keeps signing in by link.
    if let Some(listed) = disposable_domain(&state, &email).await? {
        if conn
            .query_opt(
                "SELECT 1 FROM users WHERE lower(email) = lower($1)",
                &[&email],
            )
            .await?
            .is_none()
        {
            log_refusal(&listed, "link");
            return Ok(rebond_login(
                &[("mode", "link"), ("error", "email_jetable")],
                next,
            ));
        }
    }
    let ligne = conn
        .query_opt(
            "INSERT INTO users (sub, email) VALUES (gen_random_uuid()::text, $1) \
             ON CONFLICT (lower(email)) WHERE email IS NOT NULL DO NOTHING \
             RETURNING sub",
            &[&email],
        )
        .await?;
    let sub = match ligne {
        Some(ligne) => ligne.get::<_, String>("sub"),
        None => conn
            .query_one(
                "SELECT sub FROM users WHERE lower(email) = lower($1)",
                &[&email],
            )
            .await?
            .get("sub"),
    };

    locale::retenir(&state, &sub, &locale::du_navigateur(&entetes), None).await?;
    let lien = lien_mail(&state, &sub, "connexion", "/login-link", None).await?;
    let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
    mail::send_connexion(&state, &email, &lien, voie).await?;
    Ok(Redirect::to("/login?sent=connexion").into_response())
}

/// Formulaire de connexion par lien posté par `/login-link`.
#[derive(Debug, Deserialize)]
pub struct LinkForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    next: Option<String>,
}

async fn link(State(state): State<AppState>, Form(req): Form<LinkForm>) -> Result<Response> {
    let sub = match consommer_jeton(&state, &req.token, "connexion").await {
        Ok((sub, _)) => sub,
        Err(ApiError::BadRequest(_)) => {
            return Ok(Redirect::to("/login-link?error=lien_invalide_ou_expire").into_response())
        }
        Err(e) => return Err(e),
    };
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET email_confirmed_at = coalesce(email_confirmed_at, now()) WHERE sub = $1",
        &[&sub],
    )
    .await?;
    let token = session::open(&state, &sub).await?;
    let mut resp = Redirect::to(&destination_interne(req.next.as_deref())).into_response();
    let (nom, valeur) = session::header(session::set_cookie(&token))?;
    resp.headers_mut().append(nom, valeur);
    Ok(resp)
}

/// Demande de réinitialisation. Réponse identique que le compte existe ou non.
async fn reset_request(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<EmailForm>,
) -> Result<Response> {
    let email = match normaliser(&req.email) {
        Ok(email) => email,
        Err(_) => {
            return Ok(rebond_login(
                &[("mode", "reset"), ("error", "email_invalide")],
                req.next.as_deref(),
            ))
        }
    };
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "SELECT sub FROM users WHERE lower(email) = lower($1)",
            &[&email],
        )
        .await?;
    if let Some(ligne) = ligne {
        let sub: String = ligne.get("sub");
        let lien = lien_mail(&state, &sub, "recovery", "/reset-password", None).await?;
        let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
        mail::send_recovery(&state, &email, &lien, voie).await?;
    }
    Ok(Redirect::to("/login?sent=recovery").into_response())
}

/// Formulaire de réinitialisation posté par `/reset-password`.
#[derive(Deserialize)]
pub struct ResetPasswordForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    confirm: String,
}

/// Nouveau mot de passe par lien de mail. Ouvre la session, et coupe les autres :
/// une réinitialisation sert aussi à reprendre un compte compromis. Formulaire →
/// 303 : une erreur attendue rebondit sur la page, le jeton — encore valide —
/// dans la query pour réessayer ; un jeton mort rebondit sans jeton.
async fn reset(
    State(state): State<AppState>,
    Form(req): Form<ResetPasswordForm>,
) -> Result<Response> {
    let rebond = |code: &str, token: Option<&str>| {
        let mut url = format!("/reset-password?error={code}");
        if let Some(t) = token {
            url.push_str("&token=");
            url.push_str(t);
        }
        Ok(Redirect::to(&url).into_response())
    };
    if req.password != req.confirm {
        return rebond("mots_de_passe_differents", Some(&req.token));
    }
    match verifier_robustesse(&req.password) {
        Ok(()) => {}
        Err(ApiError::BadRequest(code)) => return rebond(&code, Some(&req.token)),
        Err(e) => return Err(e),
    }
    let sub = match consommer_jeton(&state, &req.token, "recovery").await {
        Ok((sub, _)) => sub,
        Err(ApiError::BadRequest(_)) => return rebond("lien_invalide_ou_expire", None),
        Err(e) => return Err(e),
    };
    ecrire_mot_de_passe(&state, &sub, &req.password).await?;

    let token = session::open(&state, &sub).await?;
    session::close_others(&state, &sub, &token).await?;
    let mut resp = Redirect::to("/decision-search").into_response();
    let (nom, valeur) = session::header(session::set_cookie(&token))?;
    resp.headers_mut().append(nom, valeur);
    Ok(resp)
}

/// Formulaire posté par `/email-change` : le jeton seul, il porte tout.
#[derive(Debug, Deserialize)]
pub struct EmailChangeForm {
    #[serde(default)]
    token: String,
}

/// Changement d'adresse en deux jetons. Le premier, envoyé à
/// l'adresse actuelle, prouve la demande et déclenche l'envoi du second à la
/// nouvelle adresse ; le second prouve sa possession et écrit le changement.
/// Une session volée ne suffit donc jamais à déplacer un compte.
async fn email_change(
    State(state): State<AppState>,
    entetes: HeaderMap,
    Form(req): Form<EmailChangeForm>,
) -> Result<Response> {
    let adresse_requise = |nouvelle: Option<String>| {
        nouvelle.ok_or_else(|| ApiError::Internal("jeton email_change sans adresse".into()))
    };
    match consommer_jeton(&state, &req.token, "email_change_ancienne").await {
        Ok((sub, nouvelle)) => {
            let nouvelle = adresse_requise(nouvelle)?;
            let lien = lien_mail(
                &state,
                &sub,
                "email_change_nouvelle",
                "/email-change",
                Some(&nouvelle),
            )
            .await?;
            let voie = mail::Voie::pour(usage::is_first_party(&entetes, &state.settings));
            mail::send_email_change_nouvelle(&state, &nouvelle, &lien, voie).await?;
            return Ok(Redirect::to("/email-change?sent=nouvelle").into_response());
        }
        Err(ApiError::BadRequest(_)) => {}
        Err(e) => return Err(e),
    }
    let (sub, nouvelle) = match consommer_jeton(&state, &req.token, "email_change_nouvelle").await {
        Ok((sub, nouvelle)) => (sub, adresse_requise(nouvelle)?),
        Err(ApiError::BadRequest(_)) => {
            return Ok(Redirect::to("/email-change?error=lien_invalide_ou_expire").into_response())
        }
        Err(e) => return Err(e),
    };
    let conn = state.pool.get().await?;
    // Le NOT EXISTS évite de violer l'index unique quand l'adresse a été prise
    // entre la demande et le clic : zéro ligne, jamais une 500.
    let ecrit = conn
        .execute(
            "UPDATE users SET email = $2, \
               email_confirmed_at = coalesce(email_confirmed_at, now()) \
             WHERE sub = $1 AND NOT EXISTS \
               (SELECT 1 FROM users x WHERE lower(x.email) = lower($2) AND x.sub <> $1)",
            &[&sub, &nouvelle],
        )
        .await?;
    if ecrit == 0 {
        return Ok(Redirect::to("/email-change?error=email_deja_utilise").into_response());
    }
    Ok(Redirect::to("/profile?done=email").into_response())
}

/// Cookie de session porté par la requête, extrait sans exiger de session.
pub struct Cookies(pub Option<String>);

impl axum::extract::FromRequestParts<AppState> for Cookies {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        Ok(Cookies(session::from_parts(parts).map(|(token, _)| token)))
    }
}

/// 303 vers `/login`, paramètres posés et `next` du formulaire conservé — le
/// rebond de toute erreur attendue des formulaires de la page.
fn rebond_login(params: &[(&str, &str)], next: Option<&str>) -> Response {
    let mut pairs = params.to_vec();
    if let Some(n) = next.filter(|n| !n.is_empty()) {
        pairs.push(("next", n));
    }
    Redirect::to(&format!("/login?{}", form_urlencode(&pairs))).into_response()
}

async fn ecrire_mot_de_passe(state: &AppState, sub: &str, password: &str) -> Result<()> {
    let hash = hacher(password)?;
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET password_hash = $2, \
           email_confirmed_at = coalesce(email_confirmed_at, now()) WHERE sub = $1",
        &[&sub, &hash],
    )
    .await?;
    Ok(())
}

/// Pose un jeton de mail et rend le lien complet. Le secret n'existe en clair que
/// dans ce lien : la table n'en garde que l'empreinte. `new_email` ne sert qu'au
/// changement d'adresse : l'adresse visée voyage avec le jeton.
pub(crate) async fn lien_mail(
    state: &AppState,
    sub: &str,
    purpose: &str,
    chemin: &str,
    new_email: Option<&str>,
) -> Result<String> {
    let token = crate::oauth::token_urlsafe(32);
    let conn = state.pool.get().await?;
    conn.execute(
        "INSERT INTO auth_email_tokens (token_sha256, user_sub, purpose, new_email, expires_at) \
         VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5::int))",
        &[
            &sha256(token.as_bytes()).to_vec(),
            &sub,
            &purpose,
            &new_email,
            &LIEN_MINUTES,
        ],
    )
    .await?;
    Ok(format!(
        "{}{chemin}?token={token}",
        state.settings.web_base_url.trim_end_matches('/')
    ))
}

/// Marque le jeton consommé et rend son porteur, avec l'adresse visée quand il
/// en porte une. Usage unique : le `used_at IS NULL` est dans le `WHERE` de
/// l'UPDATE, donc deux requêtes concurrentes ne peuvent pas le dépenser deux
/// fois.
async fn consommer_jeton(
    state: &AppState,
    token: &str,
    purpose: &str,
) -> Result<(String, Option<String>)> {
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "UPDATE auth_email_tokens SET used_at = now() \
              WHERE token_sha256 = $1 AND purpose = $2 \
                AND used_at IS NULL AND expires_at > now() \
             RETURNING user_sub, new_email",
            &[&sha256(token.as_bytes()).to_vec(), &purpose],
        )
        .await?;
    ligne
        .map(|l| {
            (
                l.get::<_, String>("user_sub"),
                l.get::<_, Option<String>>("new_email"),
            )
        })
        .ok_or_else(|| ApiError::BadRequest("lien_invalide_ou_expire".into()))
}

pub(crate) fn normaliser(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    // La validité réelle d'une adresse se prouve en y envoyant un lien, pas en
    // la parsant : on ne vérifie ici que la présence d'un `@` encadré.
    let plausible = email
        .split_once('@')
        .is_some_and(|(g, d)| !g.is_empty() && d.contains('.') && !d.ends_with('.'));
    if !plausible || email.len() > 254 {
        return Err(ApiError::BadRequest("email_invalide".into()));
    }
    Ok(email)
}

/// Même exigence que l'indication portée par le formulaire.
fn verifier_robustesse(password: &str) -> Result<()> {
    let assez_long = password.chars().count() >= 8;
    let varie = password.chars().any(char::is_lowercase)
        && password.chars().any(char::is_uppercase)
        && password.chars().any(|c| c.is_ascii_digit());
    if !assez_long || !varie {
        return Err(ApiError::BadRequest("mot_de_passe_trop_faible".into()));
    }
    // argon2id ne tronque pas, contrairement à bcrypt ; la borne subsiste pour
    // plafonner le travail qu'une requête peut nous imposer.
    if password.len() > 72 {
        return Err(ApiError::BadRequest("mot_de_passe_trop_long".into()));
    }
    Ok(())
}

/// `Argon2::default()` : argon2id, 19 Mio, t=2, p=1 — les paramètres OWASP.
fn hacher(password: &str) -> Result<String> {
    let sel = SaltString::encode_b64(&random_bytes(16))
        .map_err(|e| ApiError::Internal(format!("argon2 sel: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &sel)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::Internal(format!("argon2: {e}")))
}

/// Le schéma se lit dans le haché lui-même : `$argon2id$` pour ce qu'on écrit,
/// `$2a$` pour ce qui reste de l'import Supabase.
fn verifier(password: &str, hash: &str) -> bool {
    if hash.starts_with("$argon2") {
        PasswordHash::new(hash)
            .and_then(|attendu| Argon2::default().verify_password(password.as_bytes(), &attendu))
            .is_ok()
    } else {
        bcrypt::verify(password, hash).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_acceptes_et_refuses() {
        assert_eq!(normaliser(" Bob@Example.FR ").unwrap(), "bob@example.fr");
        for mauvais in ["bob", "@example.fr", "bob@example", "bob@example."] {
            assert!(
                normaliser(mauvais).is_err(),
                "{mauvais} aurait dû être refusé"
            );
        }
    }

    #[test]
    fn robustesse_du_mot_de_passe() {
        assert!(verifier_robustesse("Motdepasse1").is_ok());
        for faible in ["court1A", "minuscules1", "MAJUSCULES1", "SansChiffre"] {
            assert!(
                verifier_robustesse(faible).is_err(),
                "{faible} aurait dû être refusé"
            );
        }
        assert!(verifier_robustesse(&format!("Aa1{}", "x".repeat(70))).is_err());
    }

    /// A hash is written in argon2id, and a bcrypt `$2a$` hash still verifies.
    #[test]
    fn les_deux_schemas_se_verifient() {
        let ecrit = hacher("Motdepasse1").unwrap();
        assert!(ecrit.starts_with("$argon2id$"), "{ecrit}");
        assert!(verifier("Motdepasse1", &ecrit));
        assert!(!verifier("Autrepasse2", &ecrit));

        // Coût 4 : le minimum de bcrypt, pour ne pas payer 10 en test.
        let herite = bcrypt::hash("Motdepasse1", 4).unwrap();
        assert!(verifier("Motdepasse1", &herite));
        assert!(!verifier("Autrepasse2", &herite));
    }
}
