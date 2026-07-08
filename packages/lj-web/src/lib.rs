//! Front LibreJustice (Leptos 0.8 SSR + hydration).
//!
//! La cible serveur compile sous `ssr` (lib consommée par le binaire `lj-server`,
//! ADR 0061) ; la cible wasm sous `hydrate`, pilotée EXCLUSIVEMENT par
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
pub mod pages;
pub mod query;
pub mod seo;

/// Point d'entree wasm : cargo-leptos appelle `hydrate()` cote client pour
/// rattacher l'arbre Leptos au DOM emis par le SSR.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(crate::app::App);
}
