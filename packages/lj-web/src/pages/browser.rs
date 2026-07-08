//! Bindings du shim auth consolide `js/auth.js` : operations auth page-level non
//! consommees par la substrate `src/auth/` (resend / updateUser / token de
//! session / event d'auth) + helpers DOM (location, navigation, confirm, fetch
//! OAuth). Compile UNIQUEMENT sous `hydrate` (cible wasm) — aucune feature
//! `web-sys` n'est disponible ici, d'ou le passage par JS.
#![cfg(feature = "hydrate")]

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use super::auth_errors::translate_auth_error;

#[wasm_bindgen(module = "/js/auth.js")]
extern "C" {
    #[wasm_bindgen(js_name = "resendSignup", catch)]
    fn js_resend_signup(email: &str) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_name = "updatePassword", catch)]
    fn js_update_password(password: &str) -> Result<js_sys::Promise, JsValue>;

    // `getAccessToken` / `onAuthStateChange` : memes exports que `src/auth/`, lus
    // ici sous des noms Rust page-level (le jeton sert d'indicateur de session).
    #[wasm_bindgen(js_name = "getAccessToken")]
    fn js_session_token() -> js_sys::Promise;

    #[wasm_bindgen(js_name = "onAuthStateChange")]
    fn js_on_auth_event(cb: &Closure<dyn FnMut(JsValue)>) -> JsValue;

    #[wasm_bindgen(js_name = "locationOrigin")]
    pub fn location_origin() -> String;

    #[wasm_bindgen(js_name = "locationPathSearch")]
    pub fn location_path_search() -> String;

    #[wasm_bindgen(js_name = "locationHash")]
    pub fn location_hash() -> String;

    #[wasm_bindgen(js_name = "clearLocationHash")]
    pub fn clear_location_hash();

    #[wasm_bindgen(js_name = "navigateHard")]
    pub fn navigate_hard(url: &str);

    #[wasm_bindgen(js_name = "confirmNative")]
    pub fn confirm_native(message: &str) -> bool;

    #[wasm_bindgen(js_name = "buildUrlWithParams")]
    pub fn build_url_with_params(base: &str, pairs_json: &str) -> String;

    #[wasm_bindgen(js_name = "oauthApprove", catch)]
    fn js_oauth_approve(
        base_url: &str,
        token: &str,
        body_json: &str,
    ) -> Result<js_sys::Promise, JsValue>;
}

/// Renvoi de l'email de confirmation. `Err` = message FR traduit.
pub async fn resend_signup(email: &str) -> Result<(), String> {
    await_void_translated(js_resend_signup(email)).await
}

/// Mise a jour du mot de passe courant. `Err` = message FR traduit.
pub async fn update_password(password: &str) -> Result<(), String> {
    await_void_translated(js_update_password(password)).await
}

/// Jeton de session courant, ou `None`.
pub async fn session_token() -> Option<String> {
    JsFuture::from(js_session_token()).await.ok()?.as_string()
}

/// `true` si une session existe (jeton non nul).
pub async fn has_session() -> bool {
    session_token().await.is_some()
}

/// POST OAuth /oauth/approve, renvoie le `code` ou un message d'erreur (texte de
/// la reponse).
pub async fn oauth_approve(base_url: &str, token: &str, body_json: &str) -> Result<String, String> {
    let promise = js_oauth_approve(base_url, token, body_json).map_err(js_error_message)?;
    JsFuture::from(promise)
        .await
        .map_err(js_error_message)
        .and_then(|v| v.as_string().ok_or_else(|| "réponse invalide".to_string()))
}

/// Abonnement aux changements d'etat d'auth (event seulement). `Drop` desabonne.
pub fn on_auth_event(mut cb: impl FnMut(String) + 'static) -> AuthEventSubscription {
    let closure = Closure::new(move |value: JsValue| {
        cb(value.as_string().unwrap_or_default());
    });
    let unsubscribe = js_on_auth_event(&closure);
    AuthEventSubscription {
        _closure: closure,
        unsubscribe,
    }
}

/// Garde d'abonnement : maintient le closure vivant, desabonne au `Drop`.
pub struct AuthEventSubscription {
    _closure: Closure<dyn FnMut(JsValue)>,
    unsubscribe: JsValue,
}

impl Drop for AuthEventSubscription {
    fn drop(&mut self) {
        if let Ok(f) = self.unsubscribe.clone().dyn_into::<js_sys::Function>() {
            let _ = f.call0(&JsValue::NULL);
        }
    }
}

/// Lit + parse le hash d'erreur d'auth du DOM. `None` si absent.
pub fn read_auth_hash_error() -> Option<super::auth_errors::AuthHashError> {
    super::auth_errors::parse_auth_hash_error(&location_hash())
}

/// Await une `Promise` qui ne renvoie rien, traduisant l'AuthError en FR.
async fn await_void_translated(promise: Result<js_sys::Promise, JsValue>) -> Result<(), String> {
    let promise = promise.map_err(translate_js_auth_error)?;
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(translate_js_auth_error)
}

/// Traduit un AuthError JS (`.code` + `.message`) en message FR.
fn translate_js_auth_error(err: JsValue) -> String {
    let code = js_sys::Reflect::get(&err, &JsValue::from_str("code"))
        .ok()
        .and_then(|v| v.as_string());
    let message = js_sys::Reflect::get(&err, &JsValue::from_str("message"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    translate_auth_error(code.as_deref(), &message)
}

/// Extrait `.message` d'une valeur d'erreur JS, sinon sa repr debug.
fn js_error_message(err: JsValue) -> String {
    js_sys::Reflect::get(&err, &JsValue::from_str("message"))
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| err.as_string())
        .unwrap_or_else(|| format!("{err:?}"))
}
