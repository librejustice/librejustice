//! Connexion par un fournisseur OIDC, code flow serveur. Google et Microsoft.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use opentelemetry::KeyValue;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

use crate::disposable::{disposable_domain, log_refusal};
use crate::error::{ApiError, Result};
use crate::locale;
use crate::oauth::{form_urlencode, token_urlsafe};
use crate::session;
use crate::state::AppState;

const EMETTEURS_GOOGLE: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];
const EMETTEUR_MICROSOFT: &str = "https://login.microsoftonline.com/{tid}/v2.0";

const ETAT_SECONDES: i64 = 600;
const DESTINATION_MAX: usize = 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
const NOM_MAX: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fournisseur {
    Google,
    Microsoft,
}

impl Fournisseur {
    const fn cle(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }

    const fn autorisation_url(self) -> &'static str {
        match self {
            Self::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    const fn token_url(self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }

    const fn jwks_url(self) -> &'static str {
        match self {
            Self::Google => "https://www.googleapis.com/oauth2/v3/certs",
            Self::Microsoft => "https://login.microsoftonline.com/common/discovery/v2.0/keys",
        }
    }

    const fn cookie_etat(self) -> &'static str {
        match self {
            Self::Google => "lj_google_oauth",
            Self::Microsoft => "lj_microsoft_oauth",
        }
    }

    fn identifiants(self, state: &AppState) -> Result<(&str, &str)> {
        let (id, secret) = match self {
            Self::Google => (
                state.settings.google_client_id.as_str(),
                state.settings.google_client_secret.as_str(),
            ),
            Self::Microsoft => (
                state.settings.microsoft_client_id.as_str(),
                state.settings.microsoft_client_secret.as_str(),
            ),
        };
        if id.is_empty() || secret.is_empty() {
            return Err(ApiError::Unavailable(format!(
                "{}_non_configure",
                self.cle()
            )));
        }
        Ok((id, secret))
    }
}

fn compter_tour(fournisseur: Fournisseur, phase: &'static str) {
    lj_telemetry::meter()
        .u64_counter("auth.provider.flow")
        .with_description("Tours de connexion OIDC, par fournisseur et par bout du tour.")
        .build()
        .add(
            1,
            &[
                KeyValue::new("provider", fournisseur.cle()),
                KeyValue::new("phase", phase),
            ],
        );
}

/// A scanner hits `/start` cold, without `Sec-Fetch-Site: same-origin`, and never
/// comes back: counted as a start, it would drag the funnel ratio under its threshold.
fn phase_de_depart(entetes: &HeaderMap) -> &'static str {
    match entetes
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some("same-origin") => "depart",
        _ => "depart_froid",
    }
}

pub fn router(fournisseur: Fournisseur) -> Router<AppState> {
    Router::new()
        .route(
            "/start",
            get(
                move |state: State<AppState>, entetes: HeaderMap, q: Query<StartQuery>| {
                    start(fournisseur, state, entetes, q)
                },
            ),
        )
        .route(
            "/callback",
            get(
                move |state: State<AppState>, entetes: HeaderMap, q: Query<CallbackQuery>| {
                    callback(fournisseur, state, entetes, q)
                },
            ),
        )
}

#[derive(Debug, Deserialize)]
struct StartQuery {
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

async fn start(
    fournisseur: Fournisseur,
    State(state): State<AppState>,
    entetes: HeaderMap,
    Query(q): Query<StartQuery>,
) -> Result<Response> {
    let client_id = match fournisseur.identifiants(&state) {
        Ok((id, _)) => id,
        Err(ApiError::Unavailable(code)) => {
            let cible = format!("/login?{}", form_urlencode(&[("error", code.as_str())]));
            return Ok(Redirect::to(&cible).into_response());
        }
        Err(e) => return Err(e),
    };
    let etat = token_urlsafe(32);
    let lien = q.link.as_deref().is_some_and(|v| !v.is_empty());
    let destination = if lien {
        "/profile".to_string()
    } else {
        destination_interne(q.next.as_deref())
    };
    let redirect_uri = uri_de_retour(&state, fournisseur);

    let url = url::Url::parse_with_params(
        fournisseur.autorisation_url(),
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("prompt", "select_account"),
            ("state", etat.as_str()),
        ],
    )
    .map_err(|e| ApiError::Internal(format!("URL d'autorisation {}: {e}", fournisseur.cle())))?;

    let valeur = if lien {
        format!("{etat}.{}.lien", URL_SAFE_NO_PAD.encode(&destination))
    } else {
        format!("{etat}.{}", URL_SAFE_NO_PAD.encode(&destination))
    };
    let cookie = session::set_transient(fournisseur.cookie_etat(), &valeur, ETAT_SECONDES);
    let mut resp = Redirect::to(url.as_str()).into_response();
    poser_cookie(&mut resp, cookie)?;
    if !crate::usage::is_first_party(&entetes, &state.settings) {
        compter_tour(fournisseur, phase_de_depart(&entetes));
    }
    Ok(resp)
}

async fn callback(
    fournisseur: Fournisseur,
    State(state): State<AppState>,
    entetes: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Result<Response> {
    let (client_id, client_secret) = fournisseur.identifiants(&state)?;

    let etat_invalide = || ApiError::BadRequest("etat_invalide".into());
    let cookie = entetes
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|entete| session::read_cookie(entete, fournisseur.cookie_etat()))
        .ok_or_else(etat_invalide)?;
    let (etat, reste) = cookie.split_once('.').ok_or_else(etat_invalide)?;
    if q.state.as_deref() != Some(etat) {
        return Err(etat_invalide());
    }
    let (destination, lien) = match reste.split_once('.') {
        Some((d, "lien")) => (d, true),
        _ => (reste, false),
    };
    let destination = URL_SAFE_NO_PAD
        .decode(destination)
        .ok()
        .and_then(|octets| String::from_utf8(octets).ok());
    let destination = destination_interne(destination.as_deref());
    let effacement = session::set_transient(fournisseur.cookie_etat(), "", 0);

    let Some(code) = q.code.as_deref() else {
        let mut resp = Redirect::to(&destination).into_response();
        poser_cookie(&mut resp, effacement)?;
        return Ok(resp);
    };

    let redirect_uri = uri_de_retour(&state, fournisseur);
    let id_token =
        match echanger_code(fournisseur, code, client_id, client_secret, &redirect_uri).await {
            Ok(jeton) => jeton,
            Err(ApiError::BadRequest(code)) => return rebond(&code, &destination, effacement),
            Err(e) => return Err(e),
        };
    let identite = verifier_id_token(fournisseur, &id_token, client_id).await?;
    if !crate::usage::is_first_party(&entetes, &state.settings) {
        compter_tour(fournisseur, "retour");
    }

    if lien {
        let mut resp = attacher(&state, &entetes, &identite).await?;
        poser_cookie(&mut resp, effacement)?;
        return Ok(resp);
    }

    let sub = match rapprocher(&state, &identite).await {
        Ok(sub) => sub,
        Err(ApiError::BadRequest(code)) => return rebond(&code, &destination, effacement),
        Err(e) => return Err(e),
    };
    locale::retenir(
        &state,
        &sub,
        &locale::du_navigateur(&entetes),
        identite.locale.as_deref(),
    )
    .await?;
    let token = session::open(&state, &sub).await?;

    let mut resp = Redirect::to(&destination).into_response();
    poser_cookie(&mut resp, session::set_cookie(&token))?;
    poser_cookie(&mut resp, effacement)?;
    Ok(resp)
}

/// `next` porte l'idempotence du rejeu : `page_gates` renvoie en 303 vers lui
/// un appelant qui tient déjà une session, sans qu'il voie l'écran ni l'erreur.
fn cible_de_rebond(code: &str, destination: &str) -> String {
    let mut params = vec![("error", code)];
    if destination != "/" {
        params.push(("next", destination));
    }
    format!("/login?{}", form_urlencode(&params))
}

/// Une erreur attendue du tour rebondit sur `/login` avec son code et l'état
/// effacé : l'utilisateur relance depuis un écran, il ne lit pas un JSON.
fn rebond(code: &str, destination: &str, effacement: String) -> Result<Response> {
    let mut resp = Redirect::to(&cible_de_rebond(code, destination)).into_response();
    poser_cookie(&mut resp, effacement)?;
    Ok(resp)
}

async fn attacher(state: &AppState, entetes: &HeaderMap, identite: &Identite) -> Result<Response> {
    let porteur = match session::from_headers(entetes) {
        Some((token, _)) => match session::resolve(state, &token).await {
            session::Verdict::Porteur { sub, .. } => Some(sub),
            _ => None,
        },
        None => None,
    };
    let Some(sub) = porteur else {
        return Ok(Redirect::to("/login?next=%2Fprofile").into_response());
    };

    let provider = identite.fournisseur.cle();
    let conn = state.pool.get().await?;
    conn.execute(
        "INSERT INTO user_identities (provider, provider_sub, user_sub) \
         VALUES ($1, $2, $3) ON CONFLICT (provider, provider_sub) DO NOTHING",
        &[&provider, &identite.sub, &sub],
    )
    .await?;
    let ligne = conn
        .query_one(
            "SELECT user_sub FROM user_identities WHERE provider = $1 AND provider_sub = $2",
            &[&provider, &identite.sub],
        )
        .await?;
    let cible = if ligne.get::<_, String>("user_sub") == sub {
        format!("/profile?done={provider}")
    } else {
        format!("/profile?error={provider}_deja_lie")
    };
    Ok(Redirect::to(&cible).into_response())
}

async fn echanger_code(
    fournisseur: Fournisseur,
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<String> {
    #[derive(Deserialize)]
    struct Jetons {
        id_token: String,
    }

    let cle = fournisseur.cle();
    let amont = |e: reqwest::Error| ApiError::BadGateway(format!("échange du code {cle}: {e}"));
    let res = http()?
        .post(fournisseur.token_url())
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(amont)?;
    if !res.status().is_success() {
        let statut = res.status();
        let corps = res.text().await.unwrap_or_default();
        tracing::warn!(fournisseur = cle, %statut, corps = %corps, "code exchange refused");
        return Err(refus_echange(cle, &corps));
    }
    Ok(res.json::<Jetons>().await.map_err(amont)?.id_token)
}

/// `invalid_grant` dit que le code a déjà servi ou a expiré : un callback
/// périmé rouvert, pas une panne du fournisseur. Le séparer rend un rebond à
/// qui n'a qu'à relancer son tour, là où un 502 lui opposait une impasse.
fn refus_echange(cle: &str, corps: &str) -> ApiError {
    let motif = serde_json::from_str::<Value>(corps)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned));
    match motif.as_deref() {
        Some("invalid_grant") => ApiError::BadRequest("tour_expire".into()),
        _ => ApiError::BadGateway(format!("{cle}_token")),
    }
}

async fn verifier_id_token(
    fournisseur: Fournisseur,
    id_token: &str,
    client_id: &str,
) -> Result<Identite> {
    let cle_fournisseur = fournisseur.cle();
    let refus = |detail: String| {
        tracing::warn!(fournisseur = cle_fournisseur, detail, "id_token refused");
        ApiError::BadRequest("id_token_invalide".into())
    };
    let entete = decode_header(id_token).map_err(|e| refus(e.to_string()))?;
    let kid = entete
        .kid
        .ok_or_else(|| refus("en-tête sans kid".to_string()))?;

    let jwks: JwkSet = http()?
        .get(fournisseur.jwks_url())
        .send()
        .await
        .map_err(|e| ApiError::BadGateway(format!("JWKS {cle_fournisseur}: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::BadGateway(format!("JWKS {cle_fournisseur}: {e}")))?;
    let cle = cle_rsa(&jwks, &kid).ok_or_else(|| refus(format!("kid {kid} hors JWKS")))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    if fournisseur == Fournisseur::Google {
        validation.set_issuer(&EMETTEURS_GOOGLE);
    }
    let claims = decode::<Claims>(id_token, &cle, &validation)
        .map_err(|e| refus(format!("{:?}", e.kind())))?
        .claims;
    if fournisseur == Fournisseur::Microsoft {
        let tid = claims.tid.as_deref().unwrap_or_default();
        if !guid(tid) || claims.iss != EMETTEUR_MICROSOFT.replace("{tid}", tid) {
            return Err(refus(format!("émetteur {} pour tid {tid}", claims.iss)));
        }
    }
    identite(fournisseur, claims)
}

fn guid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn cle_rsa(jwks: &JwkSet, kid: &str) -> Option<DecodingKey> {
    match &jwks.find(kid)?.algorithm {
        AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e).ok(),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    #[serde(default)]
    tid: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    xms_edov: Option<Value>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    locale: Option<String>,
}

fn edov(claim: Option<&Value>) -> bool {
    match claim {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true" || s == "1",
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Identite {
    fournisseur: Fournisseur,
    sub: String,
    email: String,
    email_verifie: bool,
    nom: Option<String>,
    locale: Option<String>,
}

fn identite(fournisseur: Fournisseur, claims: Claims) -> Result<Identite> {
    let repli = match fournisseur {
        Fournisseur::Google => None,
        Fournisseur::Microsoft => claims.preferred_username.filter(|u| u.contains('@')),
    };
    let email = claims
        .email
        .or(repli)
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .ok_or_else(|| ApiError::BadRequest(format!("{}_sans_email", fournisseur.cle())))?;
    Ok(Identite {
        fournisseur,
        sub: claims.sub,
        email,
        email_verifie: match fournisseur {
            Fournisseur::Google => claims.email_verified,
            Fournisseur::Microsoft => edov(claims.xms_edov.as_ref()),
        },
        nom: claims
            .name
            .map(|n| n.trim().chars().take(NOM_MAX).collect::<String>())
            .filter(|n| !n.is_empty()),
        locale: claims.locale.as_deref().and_then(locale::normaliser),
    })
}

async fn rapprocher(state: &AppState, identite: &Identite) -> Result<String> {
    let provider = identite.fournisseur.cle();
    let mut conn = state.pool.get().await?;
    let connu = conn
        .query_opt(
            "SELECT user_sub FROM user_identities WHERE provider = $1 AND provider_sub = $2",
            &[&provider, &identite.sub],
        )
        .await?;
    if let Some(ligne) = connu {
        return Ok(ligne.get("user_sub"));
    }

    let tx = conn.transaction().await?;
    if identite.email_verifie {
        let adopte = tx
            .query_opt(
                "UPDATE users SET \
                   password_hash = CASE WHEN email_confirmed_at IS NULL \
                                        THEN NULL ELSE password_hash END, \
                   email_confirmed_at = coalesce(email_confirmed_at, now()) \
                 WHERE lower(email) = lower($1) RETURNING sub",
                &[&identite.email],
            )
            .await?;
        if let Some(ligne) = adopte {
            let sub: String = ligne.get("sub");
            tx.execute(
                "INSERT INTO user_identities (provider, provider_sub, user_sub) \
                 VALUES ($1, $2, $3)",
                &[&provider, &identite.sub, &sub],
            )
            .await?;
            tx.commit().await?;
            return Ok(sub);
        }
    }

    if let Some(listed) = disposable_domain(state, &identite.email).await? {
        log_refusal(&listed, provider);
        return Err(ApiError::BadRequest("email_jetable".into()));
    }
    let cree = tx
        .query_opt(
            "INSERT INTO users (sub, email, display_name, email_confirmed_at) \
             VALUES (gen_random_uuid()::text, $1, $2, CASE WHEN $3::bool THEN now() END) \
             ON CONFLICT (lower(email)) WHERE email IS NOT NULL DO NOTHING \
             RETURNING sub",
            &[&identite.email, &identite.nom, &identite.email_verifie],
        )
        .await?;
    let Some(ligne) = cree else {
        return Err(ApiError::BadRequest("email_deja_utilise".into()));
    };
    let sub: String = ligne.get("sub");
    tx.execute(
        "INSERT INTO user_identities (provider, provider_sub, user_sub) VALUES ($1, $2, $3)",
        &[&provider, &identite.sub, &sub],
    )
    .await?;
    tx.commit().await?;
    Ok(sub)
}

/// Declared verbatim in the Google and Microsoft consoles: the provider matches
/// the string, so the prefix does not move without the console moving first.
fn uri_de_retour(state: &AppState, fournisseur: Fournisseur) -> String {
    format!(
        "{}/internal/auth/{}/callback",
        state.settings.public_base_url.trim_end_matches('/'),
        fournisseur.cle()
    )
}

pub(crate) fn destination_interne(next: Option<&str>) -> String {
    let interne = |n: &str| {
        n.starts_with('/')
            && !n.starts_with("//")
            && !n.starts_with("/\\")
            && n.len() <= DESTINATION_MAX
            && n.bytes().all(|b| b.is_ascii_graphic())
    };
    match next {
        Some(n) if interne(n) => n.to_string(),
        _ => "/".to_string(),
    }
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("client HTTP: {e}")))
}

fn poser_cookie(resp: &mut Response, valeur: String) -> Result<()> {
    let (nom, valeur) = session::header(valeur)?;
    resp.headers_mut().append(nom, valeur);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TID_MSA: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

    #[test]
    fn destination_interne_garde_les_chemins_du_site() {
        for chemin in ["/", "/profil", "/recherche?q=bail&page=2", "/texte/code#a1"] {
            assert_eq!(destination_interne(Some(chemin)), chemin);
        }
    }

    #[test]
    fn destination_interne_ramene_le_reste_a_l_accueil() {
        let trop_long = format!("/{}", "a".repeat(DESTINATION_MAX));
        for mauvais in [
            "//evil.tld",
            "/\\evil.tld",
            "https://evil.tld/",
            "profil",
            "",
            "/accents/été",
            "/saut\nde-ligne",
            trop_long.as_str(),
        ] {
            assert_eq!(destination_interne(Some(mauvais)), "/", "{mauvais:?}");
        }
        assert_eq!(destination_interne(None), "/");
    }

    #[test]
    fn seul_un_clic_depuis_nos_pages_entre_dans_l_entonnoir() {
        let mut entetes = HeaderMap::new();
        assert_eq!(phase_de_depart(&entetes), "depart_froid");
        for absent in ["none", "same-site", "cross-site"] {
            entetes.insert("sec-fetch-site", absent.parse().expect("en-tête"));
            assert_eq!(phase_de_depart(&entetes), "depart_froid", "{absent}");
        }
        entetes.insert("sec-fetch-site", "same-origin".parse().expect("en-tête"));
        assert_eq!(phase_de_depart(&entetes), "depart");
    }

    /// Only `invalid_grant` restarts the round; any other refusal is a provider failure.
    #[test]
    fn seul_invalid_grant_renvoie_relancer_le_tour() {
        assert!(matches!(
            refus_echange("google", r#"{"error":"invalid_grant","error_description":"Bad Request"}"#),
            ApiError::BadRequest(code) if code == "tour_expire"
        ));
        for amont in [
            r#"{"error":"invalid_client"}"#,
            r#"{"error":"temporarily_unavailable"}"#,
            r#"{"pas_de_champ_error":1}"#,
            "<html>502 Bad Gateway</html>",
            "",
        ] {
            assert!(
                matches!(refus_echange("google", amont), ApiError::BadGateway(m) if m == "google_token"),
                "{amont:?}"
            );
        }
    }

    /// Sans `next`, un rejeu abouti repart sur l'accueil au lieu de sa page.
    #[test]
    fn le_rebond_emporte_la_destination_sauf_la_racine() {
        assert_eq!(
            cible_de_rebond("tour_expire", "/"),
            "/login?error=tour_expire"
        );
        let cible = cible_de_rebond("tour_expire", "/decision/abc");
        assert!(
            cible.starts_with("/login?error=tour_expire&next="),
            "{cible}"
        );
        assert!(cible.ends_with("%2Fdecision%2Fabc"), "{cible}");
    }

    fn claims(valeur: serde_json::Value) -> Claims {
        serde_json::from_value(valeur).expect("claims")
    }

    fn google(mut valeur: serde_json::Value) -> Claims {
        valeur["iss"] = json!("https://accounts.google.com");
        claims(valeur)
    }

    fn microsoft(mut valeur: serde_json::Value) -> Claims {
        valeur["iss"] = json!(format!("https://login.microsoftonline.com/{TID_MSA}/v2.0"));
        claims(valeur)
    }

    #[test]
    fn identite_lue_des_claims() {
        let id = identite(
            Fournisseur::Google,
            google(json!({
                "sub": "123456789012345678901",
                "email": "  Jean.Dupont@Example.FR ",
                "email_verified": true,
                "name": "Jean Dupont",
                "locale": "fr",
                "picture": "https://lh3.googleusercontent.com/a/x",
            })),
        )
        .expect("identité complète");
        assert_eq!(
            id,
            Identite {
                fournisseur: Fournisseur::Google,
                sub: "123456789012345678901".to_string(),
                email: "jean.dupont@example.fr".to_string(),
                email_verifie: true,
                nom: Some("Jean Dupont".to_string()),
                locale: Some("fr".to_string()),
            }
        );
    }

    #[test]
    fn identite_sans_email_verifie_ni_nom() {
        let id = identite(
            Fournisseur::Google,
            google(json!({"sub": "42", "email": "a@b.fr"})),
        )
        .unwrap();
        assert!(!id.email_verifie);
        assert_eq!(id.nom, None);
        assert_eq!(id.locale, None);
    }

    #[test]
    fn identite_refuse_les_claims_sans_email() {
        for sans in [json!({"sub": "42"}), json!({"sub": "42", "email": " "})] {
            assert!(identite(Fournisseur::Google, google(sans)).is_err());
        }
    }

    #[test]
    fn identite_tronque_le_nom() {
        let id = identite(
            Fournisseur::Google,
            google(json!({"sub": "42", "email": "a@b.fr", "name": "é".repeat(200)})),
        )
        .unwrap();
        assert_eq!(id.nom.unwrap().chars().count(), NOM_MAX);
    }

    #[test]
    fn identite_microsoft_se_replie_sur_l_upn() {
        let id = identite(
            Fournisseur::Microsoft,
            microsoft(json!({"sub": "AAAA", "preferred_username": "Me@Cabinet.FR"})),
        )
        .unwrap();
        assert_eq!(id.email, "me@cabinet.fr");
        assert!(!id.email_verifie);
    }

    #[test]
    fn seul_edov_verifie_une_adresse_microsoft() {
        for (valeur, attendu) in [
            (json!(true), true),
            (json!("true"), true),
            (json!("1"), true),
            (json!(false), false),
            (json!(null), false),
        ] {
            let id = identite(
                Fournisseur::Microsoft,
                microsoft(json!({
                    "sub": "AAAA",
                    "email": "me@cabinet.fr",
                    "email_verified": true,
                    "xms_edov": valeur,
                })),
            )
            .unwrap();
            assert_eq!(id.email_verifie, attendu, "{valeur}");
        }
    }

    #[test]
    fn le_tid_doit_etre_un_guid() {
        assert!(guid(TID_MSA));
        for mauvais in [
            "",
            "common",
            "../../evil",
            "9188040d6c674c5bb11236a304b66dad",
            "9188040d-6c67-4c5b-b112-36a304b66da",
            "9188040g-6c67-4c5b-b112-36a304b66dad",
        ] {
            assert!(!guid(mauvais), "{mauvais:?}");
        }
    }
}
