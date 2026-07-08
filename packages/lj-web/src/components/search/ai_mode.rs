//! Toggle « mode IA » persistant (port de `lib/ai-mode.ts`, ADR 0041).
//!
//! Source de vérité : l'URL (`?aiMode=true`). Le localStorage sert de mémoire
//! inter-sessions. Hook Leptos : `use_ai_mode()` renvoie `(Signal<bool>,
//! Callback<bool>)` synchronisé avec le query param `aiMode` ET le localStorage.
//!
//! NB : ce fichier est rattaché au module via `#[path]` depuis `compact_search`
//! (le `mod.rs` de `search/` est figé par la substrate et n'expose que les
//! composants ; on ajoute ici un module privé de la tranche sans le toucher).

use leptos::prelude::*;

/// Clé localStorage de la préférence mode IA (lue/écrite côté client seulement).
#[cfg(feature = "hydrate")]
const STORAGE_KEY: &str = "librejustice.aiMode";

/// `true` si la valeur de query param vaut `"true"` ou `"1"`. Port de
/// `isAiModeParam`.
pub fn is_ai_mode_param(value: Option<&str>) -> bool {
    matches!(value, Some("true") | Some("1"))
}

/// Lit la préférence stockée (localStorage). SSR : pas de `window` => `false`.
/// Accès via `js_sys::Reflect` sur `window.localStorage` (la feature web-sys
/// `Storage` n'est pas activée dans le workspace).
#[cfg(feature = "hydrate")]
pub fn read_stored() -> bool {
    local_storage_get(STORAGE_KEY).as_deref() == Some("1")
}

/// SSR : pas de localStorage (parité `typeof localStorage === undefined`).
#[cfg(feature = "ssr")]
pub fn read_stored() -> bool {
    false
}

/// Écrit/efface la préférence (port de `writeStoredAiMode`).
#[cfg(feature = "hydrate")]
pub fn write_stored(value: bool) {
    if value {
        local_storage_set(STORAGE_KEY, "1");
    } else {
        local_storage_remove(STORAGE_KEY);
    }
}

#[cfg(feature = "ssr")]
pub fn write_stored(_value: bool) {}

// ── Pont localStorage via js_sys (sans feature web-sys Storage) ──────────────

#[cfg(feature = "hydrate")]
fn local_storage() -> Option<js_sys::Object> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let storage = js_sys::Reflect::get(&window, &"localStorage".into()).ok()?;
    storage.dyn_into::<js_sys::Object>().ok()
}

#[cfg(feature = "hydrate")]
fn local_storage_get(key: &str) -> Option<String> {
    use wasm_bindgen::JsCast;
    let storage = local_storage()?;
    let get = js_sys::Reflect::get(&storage, &"getItem".into()).ok()?;
    let func: js_sys::Function = get.dyn_into().ok()?;
    let value = func.call1(&storage, &key.into()).ok()?;
    value.as_string()
}

#[cfg(feature = "hydrate")]
fn local_storage_set(key: &str, value: &str) {
    use wasm_bindgen::JsCast;
    if let Some(storage) = local_storage() {
        if let Ok(set) = js_sys::Reflect::get(&storage, &"setItem".into()) {
            if let Ok(func) = set.dyn_into::<js_sys::Function>() {
                let _ = func.call2(&storage, &key.into(), &value.into());
            }
        }
    }
}

#[cfg(feature = "hydrate")]
fn local_storage_remove(key: &str) {
    use wasm_bindgen::JsCast;
    if let Some(storage) = local_storage() {
        if let Ok(remove) = js_sys::Reflect::get(&storage, &"removeItem".into()) {
            if let Ok(func) = remove.dyn_into::<js_sys::Function>() {
                let _ = func.call1(&storage, &key.into());
            }
        }
    }
}

/// Hook : état `aiMode` synchronisé avec le query param `aiMode` et le
/// localStorage. Renvoie `(getter, setter)`.
///
/// - Init : `aiMode` URL présent => l'adopter ; sinon `read_stored()`.
/// - Effect (hydrate) : sync URL -> state au back/forward.
/// - Effect mount (hydrate) : si l'URL n'a pas `aiMode` mais le store dit ON et
///   `q` est présent, patcher l'URL (`replace`) pour servir un résultat IA.
/// - `set` : maj state + `write_stored` ; si `q` présent, refléter dans l'URL.
pub fn use_ai_mode() -> (Signal<bool>, Callback<bool>) {
    use leptos_router::hooks::use_query_map;

    let query = use_query_map();
    // Init aligné sur le SSR : l'URL seule, jamais le localStorage. Lire le store
    // ici désynchroniserait le SSR (`read_stored` = false, pas de `window`) du
    // client (préférence mémorisée) ; Leptos adopte alors le DOM SSR à
    // l'hydratation et ne re-patche PAS l'attribut tant que le signal ne CHANGE
    // pas → l'interrupteur resterait visuellement OFF alors que l'état interne est
    // ON. La préférence localStorage est donc appliquée post-hydratation par
    // l'effet de mount (un vrai `false → true` qui, lui, re-patche l'attribut).
    let initial = match query.get_untracked().get("aiMode") {
        Some(v) => is_ai_mode_param(Some(v.as_str())),
        None => false,
    };
    let state = RwSignal::new(initial);

    #[cfg(feature = "hydrate")]
    {
        use leptos_router::hooks::use_navigate;
        use leptos_router::NavigateOptions;

        // Sync URL -> state (back/forward).
        Effect::new(move |_| {
            let map = query.get();
            if let Some(v) = map.get("aiMode") {
                state.set(is_ai_mode_param(Some(v.as_str())));
            }
        });

        // Au mount (post-hydratation) : applique la préférence localStorage. Le
        // store ON sans `aiMode` dans l'URL => `state` passe false (valeur SSR) à
        // true, ce qui re-patche `aria-checked` (le mismatch d'hydratation n'est
        // pas réconcilié par Leptos, cf. init). Si `q` est présent on patche aussi
        // l'URL (replace) pour servir un résultat IA dès la 1re requête ; sans `q`
        // (landing) le toggle s'affiche ON, l'URL est posée au submit par le caller.
        let navigate = use_navigate();
        Effect::new(move |prev: Option<()>| {
            if prev.is_some() {
                return;
            }
            let map = query.get_untracked();
            if map.get("aiMode").is_none() && read_stored() {
                state.set(true);
                if map.get("q").is_some() {
                    let qs = super::query_state::with_param(&map, "aiMode", Some("true"), false);
                    navigate(
                        &super::query_state::search_href(&qs),
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
            }
        });
    }

    let setter = {
        #[cfg(feature = "hydrate")]
        {
            use leptos_router::hooks::use_navigate;
            use leptos_router::NavigateOptions;
            let navigate = use_navigate();
            Callback::new(move |value: bool| {
                state.set(value);
                write_stored(value);
                let map = query.get_untracked();
                if map.get("q").is_some() {
                    let qs = super::query_state::with_param(
                        &map,
                        "aiMode",
                        if value { Some("true") } else { None },
                        false,
                    );
                    navigate(
                        &super::query_state::search_href(&qs),
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
            })
        }
        #[cfg(feature = "ssr")]
        {
            Callback::new(move |value: bool| {
                state.set(value);
            })
        }
    };

    (state.into(), setter)
}
