//! Gardes de session des pages rendues serveur : ce qui dépend de la session se
//! décide avant le rendu, jamais dans un effet client.
//!
//! `/authorize-mcp` et `/profile` exigent une session — sans elle, l'écran est
//! un formulaire qui échouerait après le clic : redirection vers `/login`,
//! l'URL complète en `next`. `/login` exige l'inverse : un porteur de session
//! n'a rien à y faire, il repart en 303 vers son `next`.
//!
//! Le consentement MCP y perd `form-action` : Chrome oppose la directive à
//! toute la chaîne de redirection du POST, où seul le premier saut est connu.

use axum::extract::State;
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Redirect, Response};

use crate::oauth::quote_plus;
use crate::oidc_auth::destination_interne;
use crate::session;
use crate::state::AppState;

pub async fn session_gates(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let chemin = req.uri().path();
    let exige_session = chemin == "/authorize-mcp" || chemin == "/profile";
    if !exige_session && chemin != "/login" {
        return next.run(req).await;
    }
    let porteur = match session::from_headers(req.headers()) {
        Some((token, _)) => matches!(
            session::resolve(&state, &token).await,
            session::Verdict::Porteur { .. }
        ),
        None => false,
    };
    if exige_session && !porteur {
        let cible = match req.uri().query() {
            Some(q) => format!("{chemin}?{q}"),
            None => chemin.to_string(),
        };
        return Redirect::to(&format!("/login?next={}", quote_plus(&cible))).into_response();
    }
    if chemin == "/login" && porteur {
        let demande = req.uri().query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(cle, _)| cle == "next")
                .map(|(_, valeur)| valeur.into_owned())
        });
        return Redirect::to(&destination_interne(demande.as_deref())).into_response();
    }
    let consentement = chemin == "/authorize-mcp";
    let mut reponse = next.run(req).await;
    if consentement {
        retirer_form_action(&mut reponse);
    }
    reponse
}

/// Laisse partir le retour du formulaire de consentement, quel que soit le
/// nombre de sauts. Ouvrir la seule origine du `redirect_uri` enregistré ne
/// suffit pas : le callback du client rebondit parfois vers l'app appelante, et
/// Chrome refuse ce second saut. Le POST part, le code est émis, le navigateur
/// reste sur la page, et le jeton n'est jamais échangé.
fn retirer_form_action(reponse: &mut Response) {
    let Some(csp) = reponse
        .headers()
        .get(CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return;
    };
    if let Ok(valeur) = HeaderValue::from_str(&sans_form_action(&csp)) {
        reponse
            .headers_mut()
            .insert(CONTENT_SECURITY_POLICY, valeur);
    }
}

/// Ôte la directive `form-action`, les autres intactes.
fn sans_form_action(csp: &str) -> String {
    csp.split(';')
        .map(str::trim)
        .filter(|directive| !directive.starts_with("form-action"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seule_form_action_s_en_va() {
        let csp = "script-src 'self'; form-action 'self' https://chatgpt.com; img-src 'self'";
        assert_eq!(sans_form_action(csp), "script-src 'self'; img-src 'self'");
    }

    #[test]
    fn csp_sans_form_action_reste_intacte() {
        let csp = "script-src 'self'; frame-ancestors 'none'";
        assert_eq!(sans_form_action(csp), csp);
    }

    /// The served CSP keeps no `form-action`, whatever its spelling.
    #[test]
    fn la_csp_du_consentement_ne_contraint_aucun_saut() {
        let csp =
            "script-src 'self' 'wasm-unsafe-eval' 'nonce-x'; style-src 'self' 'unsafe-inline'; \
                   connect-src 'self'; object-src 'none'; base-uri 'self'; \
                   form-action 'self' https://chatgpt.com; frame-ancestors 'none'";
        let allegee = sans_form_action(csp);
        assert!(!allegee.contains("form-action"));
        assert!(allegee.contains("base-uri 'self'"));
        assert!(allegee.contains("frame-ancestors 'none'"));
    }
}
