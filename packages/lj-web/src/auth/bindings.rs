//! Bindings wasm-bindgen du shim `js/auth.js`. Compiles UNIQUEMENT sous
//! `hydrate` (cible wasm). Les fns JS renvoient des `Promise` -> on les await
//! via `wasm_bindgen_futures::JsFuture`.
#![cfg(feature = "hydrate")]

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/auth.js")]
extern "C" {
    #[wasm_bindgen(js_name = "getAccessToken")]
    fn js_get_access_token() -> js_sys::Promise;

    #[wasm_bindgen(js_name = "getUserEmail")]
    fn js_get_user_email() -> js_sys::Promise;

    #[wasm_bindgen(js_name = "hasSession")]
    fn js_has_session() -> js_sys::Promise;

    #[wasm_bindgen(js_name = "signInPassword", catch)]
    fn js_sign_in_password(email: &str, password: &str) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_name = "signInOauth", catch)]
    fn js_sign_in_oauth(provider: &str, redirect_to: &str) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_name = "signUp", catch)]
    fn js_sign_up(
        email: &str,
        password: &str,
        redirect_to: &str,
    ) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_name = "resetPassword", catch)]
    fn js_reset_password(email: &str, redirect_to: &str) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_name = "signOut")]
    fn js_sign_out() -> js_sys::Promise;

    #[wasm_bindgen(js_name = "onAuthStateChange")]
    fn js_on_auth_state_change(cb: &Closure<dyn FnMut(JsValue)>) -> JsValue;
}

/// Jeton d'acces courant, ou `None` (anonyme).
pub async fn get_access_token() -> Option<String> {
    let value = JsFuture::from(js_get_access_token()).await.ok()?;
    value.as_string()
}

/// Email de la session courante, ou `None` (anonyme).
pub async fn current_email() -> Option<String> {
    let value = JsFuture::from(js_get_user_email()).await.ok()?;
    value.as_string()
}

/// `true` si une session existe.
pub async fn has_session() -> bool {
    JsFuture::from(js_has_session())
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Connexion email/mot de passe. `Err(message)` = AuthError supabase traduit.
pub async fn sign_in_password(email: &str, password: &str) -> Result<(), String> {
    await_void(js_sign_in_password(email, password)).await
}

/// Connexion OAuth (PKCE).
pub async fn sign_in_oauth(provider: &str, redirect_to: &str) -> Result<(), String> {
    await_void(js_sign_in_oauth(provider, redirect_to)).await
}

/// Inscription email/mot de passe.
pub async fn sign_up(email: &str, password: &str, redirect_to: &str) -> Result<(), String> {
    await_void(js_sign_up(email, password, redirect_to)).await
}

/// Demande de reinitialisation de mot de passe.
pub async fn reset_password(email: &str, redirect_to: &str) -> Result<(), String> {
    await_void(js_reset_password(email, redirect_to)).await
}

/// Deconnexion.
pub async fn sign_out() {
    let _ = JsFuture::from(js_sign_out()).await;
}

/// Abonnement aux changements d'etat d'auth. `cb` recoit le nom de l'evenement.
/// Renvoie un garde : son `Drop` desabonne et libere le closure JS.
pub fn on_auth_state_change(mut cb: impl FnMut(String) + 'static) -> AuthSubscription {
    let closure = Closure::new(move |value: JsValue| {
        cb(value.as_string().unwrap_or_default());
    });
    let unsubscribe = js_on_auth_state_change(&closure);
    AuthSubscription {
        _closure: closure,
        unsubscribe,
    }
}

/// Garde d'abonnement : conserve le closure vivant et desabonne au `Drop`.
pub struct AuthSubscription {
    _closure: Closure<dyn FnMut(JsValue)>,
    unsubscribe: JsValue,
}

impl Drop for AuthSubscription {
    fn drop(&mut self) {
        if let Ok(f) = self.unsubscribe.clone().dyn_into::<js_sys::Function>() {
            let _ = f.call0(&JsValue::NULL);
        }
    }
}

/// Await une `Promise` qui ne renvoie rien d'utile, mappant l'erreur JS en
/// `String` (message de l'AuthError supabase, lu via `.message`).
async fn await_void(promise: Result<js_sys::Promise, JsValue>) -> Result<(), String> {
    let promise = promise.map_err(js_error_message)?;
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(js_error_message)
}

/// Extrait `.message` d'une valeur d'erreur JS, sinon sa repr debug.
fn js_error_message(err: JsValue) -> String {
    js_sys::Reflect::get(&err, &JsValue::from_str("message"))
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| err.as_string())
        .unwrap_or_else(|| format!("{err:?}"))
}
