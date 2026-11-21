//! Theme picker of the site menu. The choice lives in `localStorage["lj-theme"]`,
//! which the inline script of `app.rs` applies before the first paint.

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Theme {
    System,
    Light,
    Dark,
}

#[cfg(feature = "hydrate")]
impl Theme {
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
    const STORAGE_KEY: &str = "lj-theme";
    const PREFERS_DARK: &str = "(prefers-color-scheme: dark)";

    /// `None` when the browser refuses storage: the theme is then System on every load.
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    fn prefers_dark() -> bool {
        web_sys::window()
            .and_then(|w| w.match_media(PREFERS_DARK).ok().flatten())
            .is_some_and(|mq| mq.matches())
    }

    pub fn read_theme() -> String {
        let stored = storage().and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
        match stored.as_deref() {
            Some(v @ ("light" | "dark")) => v.to_string(),
            _ => "system".to_string(),
        }
    }

    /// `"system"` is the absence of a choice, so it removes the key.
    pub fn persist_theme(theme: &str) {
        let Some(storage) = storage() else {
            return;
        };
        let _ = if theme == "system" {
            storage.remove_item(STORAGE_KEY)
        } else {
            storage.set_item(STORAGE_KEY, theme)
        };
    }

    pub fn apply_theme(theme: &str) {
        let dark = theme == "dark" || (theme == "system" && prefers_dark());
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = root.class_list().toggle_with_force("dark", dark);
        }
    }
}

/// One row of the site menu: the label and three icon buttons.
#[component]
pub fn ThemePicker() -> impl IntoView {
    // System on both sides of hydration, the stored choice read once mounted:
    // the server cannot know it, and a first value that differs does not hydrate.
    let theme = RwSignal::new(Theme::System);

    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| theme.set(Theme::from_str(&bridge::read_theme())));
        let prefers_dark = leptos_use::use_preferred_dark();
        Effect::new(move |_| {
            prefers_dark.track();
            bridge::apply_theme(&bridge::read_theme());
        });
    }

    let choose = move |value: Theme| {
        theme.set(value);
        #[cfg(feature = "hydrate")]
        {
            bridge::persist_theme(value.as_str());
            bridge::apply_theme(value.as_str());
        }
    };

    let ui = &crate::i18n::use_ui().shell;
    let options = [
        (Theme::System, ui.theme_system),
        (Theme::Light, ui.theme_light),
        (Theme::Dark, ui.theme_dark),
    ];

    view! {
        <div class="flex items-center justify-between px-3 py-1.5">
            <span class="text-sm text-[var(--color-ink-muted)]">{ui.theme}</span>
            <div role="group" aria-label=ui.theme_group class="flex gap-1">
                {options
                    .into_iter()
                    .map(|(value, label)| {
                        let active = move || theme.get() == value;
                        view! {
                            <button
                                type="button"
                                on:click=move |_| choose(value)
                                aria-label=label
                                title=label
                                aria-pressed=move || active().to_string()
                                class=move || {
                                    crate::helpers::cn([
                                        "flex items-center justify-center rounded-md border p-1.5 transition-colors",
                                        if active() {
                                            "border-[var(--color-ink)] bg-[var(--color-vellum)] text-[var(--color-ink)]"
                                        } else {
                                            "border-transparent text-[var(--color-ink-muted)] hover:border-[var(--color-rule)] hover:text-[var(--color-ink)]"
                                        },
                                    ])
                                }
                            >
                                <ThemeIcon theme=value />
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

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
