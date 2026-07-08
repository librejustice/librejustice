//! Selecteur de theme (Systeme / Clair / Sombre) — port de `theme-settings.tsx`.
//!
//! Pont localStorage `lj-theme` + `matchMedia` via le shim JS `theme.js`
//! (web-sys indisponible sans feature dans le crate fige). L'anti-FOUC inline
//! d'`app.rs` applique deja `.dark` avant le premier paint ; ce composant gere
//! le choix explicite et le suivi de `prefers-color-scheme` en mode Systeme.

use leptos::prelude::*;

/// Theme choisi (port du type `Theme`).
#[derive(Clone, Copy, PartialEq)]
enum Theme {
    System,
    Light,
    Dark,
}

// Conversions chaine <-> Theme : utilisees seulement par le pont localStorage
// (hydrate) ; au SSR le theme initial est `System` en dur.
#[cfg(feature = "hydrate")]
impl Theme {
    /// Valeur persistee (`"light"`/`"dark"`/`"system"`).
    fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_str(s: &str) -> Theme {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

#[cfg(feature = "hydrate")]
mod bridge {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/components/profile/theme.js")]
    extern "C" {
        #[wasm_bindgen(js_name = "readTheme")]
        pub fn read_theme() -> String;
        #[wasm_bindgen(js_name = "persistTheme")]
        pub fn persist_theme(theme: &str);
        #[wasm_bindgen(js_name = "applyTheme")]
        pub fn apply_theme(theme: &str);
        #[wasm_bindgen(js_name = "syncAuthTheme")]
        pub fn sync_auth_theme(authenticated: bool);
        #[wasm_bindgen(js_name = "onPrefersDarkChange")]
        fn on_prefers_dark_change(cb: &Closure<dyn FnMut()>) -> JsValue;
    }

    /// Abonne au changement de `prefers-color-scheme`. `Drop` desabonne.
    pub fn subscribe_prefers_dark(cb: impl FnMut() + 'static) -> PrefersDarkSubscription {
        let closure = Closure::new(cb);
        let unsubscribe = on_prefers_dark_change(&closure);
        PrefersDarkSubscription {
            _closure: closure,
            unsubscribe,
        }
    }

    pub struct PrefersDarkSubscription {
        _closure: Closure<dyn FnMut()>,
        unsubscribe: JsValue,
    }

    impl Drop for PrefersDarkSubscription {
        fn drop(&mut self) {
            if let Ok(f) = self.unsubscribe.clone().dyn_into::<js_sys::Function>() {
                let _ = f.call0(&JsValue::NULL);
            }
        }
    }
}

/// Bridge thème ↔ auth (appelé depuis `App` à l'hydratation) : pose/retire le
/// flag `lj-auth` et (ré)applique le thème selon la session. Sombre réservé aux
/// connectés (cf. anti-FOUC d'`app.rs`).
#[cfg(feature = "hydrate")]
pub fn sync_auth_theme(authenticated: bool) {
    bridge::sync_auth_theme(authenticated);
}

#[component]
pub fn ThemeSettings() -> impl IntoView {
    // Valeur initiale : lue du localStorage cote client, `System` au SSR.
    #[cfg(feature = "hydrate")]
    let initial = Theme::from_str(&bridge::read_theme());
    #[cfg(not(feature = "hydrate"))]
    let initial = Theme::System;

    let theme = RwSignal::new(initial);

    #[cfg(feature = "hydrate")]
    {
        // Persistance + application a chaque changement.
        Effect::new(move |_| {
            let t = theme.get();
            bridge::persist_theme(t.as_str());
            bridge::apply_theme(t.as_str());
        });
        // En mode Systeme : suivre `prefers-color-scheme`. L'abonnement vit dans
        // un `StoredValue` local (non-`Send`, normal cote wasm) : chaque passage
        // remplace l'abonnement precedent — le `Drop` de l'ancien desabonne
        // (port du `removeEventListener` du cleanup React). `on_cleanup` est
        // inutilisable ici (l'abonnement tient un `Closure` JS, pas `Send`).
        let sub_slot: StoredValue<
            Option<bridge::PrefersDarkSubscription>,
            leptos::reactive::owner::LocalStorage,
        > = StoredValue::new_local(None);
        Effect::new(move |_| {
            if theme.get() != Theme::System {
                sub_slot.set_value(None);
                return;
            }
            let subscription = bridge::subscribe_prefers_dark(move || {
                bridge::apply_theme("system");
            });
            sub_slot.set_value(Some(subscription));
        });
    }

    let options = [
        (Theme::System, "Système"),
        (Theme::Light, "Clair"),
        (Theme::Dark, "Sombre"),
    ];

    view! {
        <fieldset class="flex flex-col gap-2">
            <legend class="sr-only">"Thème de l'interface"</legend>
            <div class="flex gap-2">
                {options
                    .into_iter()
                    .map(|(value, label)| {
                        let active = move || theme.get() == value;
                        view! {
                            <button
                                type="button"
                                on:click=move |_| theme.set(value)
                                aria-pressed=move || active().to_string()
                                class=move || {
                                    crate::helpers::cn([
                                        "flex flex-1 items-center justify-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors",
                                        if active() {
                                            "border-[var(--color-ink)] bg-[var(--color-vellum)] text-[var(--color-ink)]"
                                        } else {
                                            "border-[var(--color-rule)] text-[var(--color-ink-muted)] hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                                        },
                                    ])
                                }
                            >
                                <ThemeIcon theme=value />
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <p class="text-xs text-[var(--color-ink-subtle)]">
                "Préférence enregistrée localement sur cet appareil."
            </p>
        </fieldset>
    }
}

/// Icone SVG inline d'une option de theme (paths verbatim du React).
#[component]
fn ThemeIcon(theme: Theme) -> impl IntoView {
    match theme {
        Theme::Light => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                aria-hidden="true"
            >
                <circle cx="10" cy="10" r="3.5"></circle>
                <path d="M10 2v2M10 16v2M18 10h-2M4 10H2M15.66 4.34l-1.41 1.41M5.75 14.25l-1.41 1.41M15.66 15.66l-1.41-1.41M5.75 5.75 4.34 4.34"></path>
            </svg>
        }
        .into_any(),
        Theme::Dark => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linejoin="round"
                aria-hidden="true"
            >
                <path d="M16.5 12.5A6.5 6.5 0 0 1 7.5 3.5a6.5 6.5 0 1 0 9 9Z"></path>
            </svg>
        }
        .into_any(),
        Theme::System => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                aria-hidden="true"
            >
                <rect x="2.5" y="3.5" width="15" height="11" rx="1.5"></rect>
                <path d="M7 17h6M10 14.5v2.5"></path>
            </svg>
        }
        .into_any(),
    }
}
