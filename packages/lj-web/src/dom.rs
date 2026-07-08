//! Helpers DOM bindés à des shims JS, pour ce que les features web-sys héritées
//! de leptos n'exposent pas. Compile UNIQUEMENT sous `hydrate` (le module est
//! vide en SSR — les appels sont gatés `#[cfg(feature = "hydrate")]` côté
//! appelant, comme `spawn_local` dans `activity_page` / `DecisionActions`).
#![cfg(feature = "hydrate")]

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/clipboard.js")]
extern "C" {
    #[wasm_bindgen(js_name = "copyText")]
    fn js_copy_text(text: &str) -> js_sys::Promise;
}

/// Écrit `text` dans le presse-papier. `true` au succès, `false` si refusé
/// (port de `navigator.clipboard.writeText(...).then/catch`).
pub async fn copy_text(text: &str) -> bool {
    JsFuture::from(js_copy_text(text)).await.is_ok()
}
