//! Sticky decision bar: shared context and search-to-decision state handoff.
//!
//! - `DecisionBarSignal`: what the `TopBar` shows (title, nav, back).
//! - [`ResultNavSeed`]: the context of one visit, pushed with its history entry
//!   by the link or the navigation that opens the decision, and read by
//!   `DecisionHeader` on mount. Back, forward and reload give an entry back its
//!   own seed; any other link opens a visit without one.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use serde::{Deserialize, Serialize};

/// Origine « recherche » d'une consultation : permet au bouton retour de revenir
/// à la liste exacte (query string) et de restaurer le scroll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FromSearch {
    /// The search page, under its jurisdiction.
    pub path: String,
    /// Query string complète (`?q=…&page=…`), telle quelle.
    pub search: String,
    /// Position de scroll à restaurer au retour (px).
    pub scroll_y: f64,
    /// The query whose terms the decision highlights, `None` when the search
    /// served references. Kept out of the URL, which history, logs and shared
    /// links keep.
    pub highlight: Option<String>,
}

/// Navigation inter-résultats : la liste servie, dont se dérivent le rang, le
/// compte et précédent/suivant au chemin courant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultNav {
    pub hit_paths: Vec<String>,
}

/// État affiché par la barre sticky.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionBarState {
    pub title: String,
    pub path: String,
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

/// The context of a visit, carried by its history entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultNavSeed {
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

pub type DecisionBarSignal = RwSignal<Option<DecisionBarState>>;
pub type ResultNavSignal = RwSignal<Option<ResultNavSeed>>;
/// Graines consommées, par id de décision. Filet du back/forward navigateur :
/// décision → article de loi → retour arrière remonte la page SANS graine, le
/// one-shot ayant été consommé à la première visite. Clé = id : une décision
/// atteinte sans graine ne récupère que SON dernier contexte, jamais celui d'une
/// autre.
pub type SeedMemorySignal = StoredValue<std::collections::HashMap<String, ResultNavSeed>>;
/// Position de scroll à restaurer sur la page résultats au retour depuis une
/// décision. Posée par le bouton retour de la barre décision, consommée UNE fois
/// par `ResultsBody` à son montage, qui défère l'application au `requestAnimationFrame`
/// (après le layout de la liste, avant le premier paint) → la page peint
/// directement à la bonne position, sans saut.
pub type RestoreScrollSignal = RwSignal<Option<f64>>;

/// The anonymisation workbench on screen, whose controls the `TopBar` carries
/// while a document is open.
pub type WorkbenchSignal = RwSignal<Option<crate::pages::redact::Session>>;

/// Fournit les signaux au contexte (appelé par `AppShell`, ancêtre de la
/// `TopBar` et des pages routées — donc persistant à travers les navigations).
pub fn provide_decision_bar_contexts() {
    provide_context::<DecisionBarSignal>(RwSignal::new(None));
    provide_context::<RestoreScrollSignal>(RwSignal::new(None));
    provide_context::<WorkbenchSignal>(RwSignal::new(None));
}

/// Signal de la barre décision (ce que la `TopBar` lit).
pub fn use_decision_bar() -> DecisionBarSignal {
    expect_context::<DecisionBarSignal>()
}

/// Signal du scroll à restaurer (posé par le bouton retour, consommé au montage
/// de la liste de résultats).
pub fn use_restore_scroll() -> RestoreScrollSignal {
    expect_context::<RestoreScrollSignal>()
}

/// The search on screen, as the origin of a visit opened from it. Empty on the
/// server, where no click happens.
pub fn search_origin(highlight: Option<String>) -> FromSearch {
    #[cfg(feature = "hydrate")]
    let (path, search, scroll_y) = web_sys::window()
        .map(|w| {
            (
                w.location().pathname().unwrap_or_default(),
                w.location().search().unwrap_or_default(),
                w.scroll_y().unwrap_or(0.0),
            )
        })
        .unwrap_or_default();
    #[cfg(not(feature = "hydrate"))]
    let (path, search, scroll_y) = (String::new(), String::new(), 0.0);
    FromSearch {
        path,
        search,
        scroll_y,
        highlight,
    }
}

/// Hands `seed` to the router through the clicked link, which pushes it with
/// the history entry of the visit. The link's own `on:click` runs before the
/// router's listener on `window`, as long as event delegation stays off.
pub fn carry_seed(ev: &leptos::ev::MouseEvent, seed: &ResultNavSeed) {
    #[cfg(feature = "hydrate")]
    if let (Some(link), Ok(json)) = (ev.current_target(), serde_json::to_string(seed)) {
        let _ = js_sys::Reflect::set(&link, &"state".into(), &json.into());
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = (ev, seed);
}

/// Navigation options that push `seed` with the history entry of the visit.
pub fn seeded(seed: &ResultNavSeed) -> NavigateOptions {
    #[cfg(feature = "hydrate")]
    let state = leptos_router::location::State::new(
        serde_json::to_string(seed)
            .ok()
            .map(wasm_bindgen::JsValue::from),
    );
    #[cfg(not(feature = "hydrate"))]
    let state = {
        let _ = seed;
        Default::default()
    };
    NavigateOptions {
        state,
        ..Default::default()
    }
}

/// The seed of the visit to `path`. The router writes the history entry once
/// the route is ready, which can come after the page mounts: until then, the
/// state it was handed is the visit's.
#[cfg(feature = "hydrate")]
pub fn visit_seed(path: &str, location: &leptos_router::location::Location) -> ResultNavSeed {
    let window = web_sys::window().expect("a browser window");
    let state = if window.location().pathname().ok().as_deref() == Some(path) {
        window
            .history()
            .and_then(|h| h.state())
            .unwrap_or(wasm_bindgen::JsValue::NULL)
    } else {
        location.state.get_untracked().to_js_value()
    };
    state
        .as_string()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}
