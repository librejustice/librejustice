//! Front LibreJustice (Leptos 0.8 SSR + hydration).
//!
//! La cible serveur compile sous `ssr` (lib consommée par le binaire
//! `lj-server`) ; la cible wasm sous `hydrate`, pilotée EXCLUSIVEMENT par
//! cargo-leptos. Les deux features sont exclusives ; `default = ["ssr"]` garde le
//! build host workspace vert.

// Les vues Leptos imbriquees (routes -> page -> composants) generent des types
// `RenderHtml`/`AnyView` monomorphises tres profonds ; la limite par defaut (128)
// deborde au calcul de layout des futures `resolve`. 512 couvre l'arbre actuel.
#![recursion_limit = "512"]

pub mod api;
pub mod app;
pub mod auth;
pub mod components;
pub mod config;
pub mod dom;
pub mod helpers;
pub mod incident;
pub mod pages;
pub mod query;
pub mod reads;
pub mod seo;

/// Entry point of the anonymisation workbench mounted as an MCP App view, its
/// files fetched from `origin`.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn mount_workbench(origin: String) {
    console_error_panic_hook::set_once();
    crate::pages::redact::mcp_app::mount(origin);
}

/// Point d'entree wasm : cargo-leptos appelle `hydrate()` cote client pour
/// rattacher l'arbre Leptos au DOM emis par le SSR.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    crate::reads::note_document();
    leptos::mount::hydrate_body(crate::app::App);
    crate::reads::report_landing();
    crate::reads::count_actions();
    // Témoin lu par la sonde d'hydratation du shell.
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("__LJ_HYDRATED"),
        &wasm_bindgen::JsValue::TRUE,
    );
    // Stamped here, never read when the probe fires: a clamped poll wakes minutes late.
    let instant = web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or_default();
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("__LJ_HYDRATED_AT"),
        &wasm_bindgen::JsValue::from_f64(instant),
    );
}
