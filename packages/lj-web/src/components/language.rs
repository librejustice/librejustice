//! The reader's language: its row in the site menu, the versions a page names to crawlers, and
//! the line offering the browser's language. None shows while one language is open.

use leptos::prelude::*;
use leptos_meta::Link;
use leptos_router::hooks::use_location;

use crate::i18n::{address_default, address_jurisdiction, in_lang, use_reader_lang, use_ui, Lang};
use crate::seo::CANONICAL_BASE;

fn several_open() -> bool {
    Lang::open().nth(1).is_some()
}

/// The current address in `lang`.
fn current_in(lang: Lang) -> impl Fn() -> String + Clone + Send + Sync + 'static {
    let location = use_location();
    move || {
        let path = location.pathname.get();
        location.search.with(|search| match search.is_empty() {
            true => in_lang(&path, lang),
            false => in_lang(&format!("{path}?{search}"), lang),
        })
    }
}

/// Keeps `lang` in the cookie the bare addresses read, before the link loads its version.
fn keep_choice(lang: Lang) {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        if let Ok(document) = document().dyn_into::<web_sys::HtmlDocument>() {
            let _ = document.set_cookie(&format!(
                "{}={}; Path=/; Max-Age=31536000; SameSite=Lax; Secure",
                crate::i18n::LANG_COOKIE,
                lang.code()
            ));
        }
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = lang;
}

/// One row of the site menu, beside the theme: each open language, named in itself. A choice
/// loads its version, `rel="external"` keeping the router out.
#[component]
pub fn LanguagePicker() -> impl IntoView {
    several_open().then(|| {
        let current = use_reader_lang().get();
        let ui = &use_ui().shell;
        view! {
            <div class="flex items-center justify-between px-3 py-1.5">
                <span class="text-sm text-[var(--color-ink-muted)]">{ui.language}</span>
                <div role="group" aria-label=ui.language_group class="flex gap-1">
                    {Lang::open()
                        .map(|lang| {
                            let active = lang == current;
                            view! {
                                <a
                                    href=current_in(lang)
                                    rel="external"
                                    hreflang=lang.code()
                                    lang=lang.code()
                                    aria-label=lang.name()
                                    title=lang.name()
                                    aria-current=active.then_some("true")
                                    on:click=move |_| keep_choice(lang)
                                    class=crate::helpers::cn([
                                        "rounded-md border px-1.5 py-0.5 text-xs font-medium no-underline transition-colors",
                                        if active {
                                            "border-[var(--color-ink)] bg-[var(--color-vellum)] text-[var(--color-ink)]"
                                        } else {
                                            "border-transparent text-[var(--color-ink-muted)] hover:border-[var(--color-rule)] hover:text-[var(--color-ink)]"
                                        },
                                    ])
                                >
                                    {lang.code().to_uppercase()}
                                </a>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
        }
    })
}

/// Every version of the page, each under its `hreflang`, the address's default as `x-default`.
#[component]
pub fn LangAlternates() -> impl IntoView {
    several_open().then(|| {
        let location = use_location();
        move || {
            let path = location.pathname.get();
            let url = location.search.with(|search| match search.is_empty() {
                true => format!("{CANONICAL_BASE}{path}"),
                false => format!("{CANONICAL_BASE}{path}?{search}"),
            });
            let jurisdiction = address_jurisdiction(&path);
            Lang::open()
                .map(|lang| (lang.hreflang(jurisdiction), lang))
                .chain([("x-default".to_string(), address_default(&path))])
                .map(|(hreflang, lang)| {
                    view! { <Link rel="alternate" hreflang=hreflang href=in_lang(&url, lang) /> }
                })
                .collect_view()
        }
    })
}

/// A first visit whose browser asks first for another open language is offered it, in that
/// language, once.
#[component]
pub fn LangOffer() -> impl IntoView {
    let offered = RwSignal::new(None::<Lang>);
    #[cfg(feature = "hydrate")]
    {
        let current = use_reader_lang().get();
        Effect::new(move |_| {
            if let Some(lang) = offer::browser_asks(current) {
                offer::mark_offered();
                offered.set(Some(lang));
            }
        });
    }
    move || {
        offered.get().map(|lang| {
            let ui = &lang.ui().shell;
            view! {
                <div
                    lang=lang.tag()
                    class="flex items-center justify-center gap-3 border-b border-[var(--color-rule)] bg-[var(--color-vellum)] px-4 py-1.5 text-sm text-[var(--color-ink)]"
                >
                    <span>{ui.lang_offer}</span>
                    <a
                        href=current_in(lang)
                        rel="external"
                        hreflang=lang.code()
                        on:click=move |_| keep_choice(lang)
                        class="font-medium underline underline-offset-2"
                    >
                        {ui.lang_offer_accept}
                    </a>
                    <button
                        type="button"
                        on:click=move |_| offered.set(None)
                        aria-label=ui.lang_offer_dismiss
                        title=ui.lang_offer_dismiss
                        class="rounded px-1.5 text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
                    >
                        "×"
                    </button>
                </div>
            }
        })
    }
}

#[cfg(feature = "hydrate")]
mod offer {
    use crate::i18n::{Lang, LANG_COOKIE};

    const OFFERED: &str = "lj-lang-offered";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    /// The first open language the browser lists, when it is not the page's and the reader
    /// has neither chosen a language nor been offered one.
    pub fn browser_asks(current: Lang) -> Option<Lang> {
        use wasm_bindgen::JsCast;
        let window = web_sys::window()?;
        let cookies = window
            .document()?
            .dyn_into::<web_sys::HtmlDocument>()
            .ok()?
            .cookie()
            .ok()?;
        let chosen = cookies.split(';').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(name, _)| name.trim() == LANG_COOKIE)
        });
        let offered = storage().is_some_and(|s| s.get_item(OFFERED).ok().flatten().is_some());
        if chosen || offered {
            return None;
        }
        let asked = window
            .navigator()
            .languages()
            .iter()
            .filter_map(|tag| tag.as_string())
            .find_map(|tag| Lang::from_code(&tag.split('-').next()?.to_ascii_lowercase()))?;
        (asked != current).then_some(asked)
    }

    pub fn mark_offered() {
        if let Some(storage) = storage() {
            let _ = storage.set_item(OFFERED, "1");
        }
    }
}
