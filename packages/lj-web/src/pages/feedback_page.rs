//! Page `/feedback` : formulaire natif vers `/internal/signalement`, qui rebondit ici
//! en 303 : `sent=1` ou `error=<code>`, la page d'origine dans `from`.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::components::prose;
use crate::components::ui::{DropdownSelect, SelectOption};

/// Alignées sur l'énumération de `lj-api::support`.
const SUJETS: [&str; 4] = ["technique", "manque", "compte", "autre"];

const CHAMP: &str = "w-full rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]";
const ETIQUETTE: &str = "text-xs uppercase tracking-[0.15em] text-[var(--color-ink-muted)]";

#[component]
pub fn FeedbackPage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let scope = crate::jurisdiction::use_reader_scope();
    let de = move || {
        let brut = query.with(|q| q.get("from").unwrap_or_default());
        if brut.starts_with('/') && !brut.starts_with("//") {
            brut
        } else {
            scope.path("/")
        }
    };
    let envoye = move || query.with(|q| q.get("sent").is_some());
    let erreur = move || query.with(|q| q.get("error"));
    let ui = &crate::i18n::use_ui().feedback;

    view! {
        <Title text=ui.title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-lg flex-1 flex-col px-4 py-16 sm:px-6">
            <Show
                when=envoye
                fallback=move || {
                    view! { <Formulaire de=de() erreur=erreur() /> }
                }
            >
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.sent_heading}</h1>
                <p class="mt-3 text-sm text-[var(--color-ink-muted)]">{ui.sent_body}</p>
                <p class="mt-8">
                    <a
                        class="text-sm text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href=de
                    >
                        <span aria-hidden="true">"←"</span>
                        " "
                        {ui.back_to_page}
                    </a>
                </p>
            </Show>
        </div>
    }
}

#[component]
fn Formulaire(de: String, erreur: Option<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().feedback;
    let motif = erreur.as_deref().map(ui.error).unwrap_or_default();
    let sujet = RwSignal::new(SUJETS[0].to_string());
    let sujets: Vec<SelectOption> = SUJETS
        .iter()
        .zip(ui.subjects)
        .map(|(value, label)| SelectOption {
            value: value.to_string(),
            label: label.to_string(),
        })
        .collect();
    view! {
        <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.heading}</h1>
        <p class="mt-3 text-sm text-[var(--color-ink-muted)]">
            {prose(ui.intro, "underline underline-offset-2 hover:text-[var(--color-ink)]")}
        </p>
        <p class="mt-4 min-h-5 text-sm text-red-600">{motif}</p>
        <form method="post" action="/internal/signalement" class="mt-2 flex flex-col gap-5">
            <div class="flex flex-col gap-1.5">
                <span class=ETIQUETTE>{ui.subject}</span>
                <input type="hidden" name="sujet" value=move || sujet.get() />
                <DropdownSelect
                    value=sujet
                    on_change=Callback::new(move |picked| sujet.set(picked))
                    options=sujets
                    aria_label=ui.subject
                    class="w-full"
                    field=true
                />
            </div>

            <label class="flex flex-col gap-1.5">
                <span class=ETIQUETTE>{ui.page}</span>
                <input type="text" name="url" value=de.clone() class=CHAMP />
            </label>

            <label class="flex flex-col gap-1.5">
                <span class=ETIQUETTE>{ui.message}</span>
                <textarea
                    name="message"
                    rows="6"
                    required
                    minlength="10"
                    maxlength="4000"
                    class=CHAMP
                ></textarea>
            </label>

            <label class="flex flex-col gap-1.5">
                <span class=ETIQUETTE>{ui.email}</span>
                <input type="email" name="email" placeholder=ui.email_placeholder class=CHAMP />
                <span class="text-xs text-[var(--color-ink-subtle)]">{ui.email_hint}</span>
            </label>

            // Leurre : hors du flux, sans étiquette, hors du parcours clavier.
            <input
                type="text"
                name="site"
                tabindex="-1"
                autocomplete="off"
                aria-hidden="true"
                class="absolute left-[-9999px] h-0 w-0 opacity-0"
            />

            <button
                type="submit"
                class="self-start rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
            >
                {ui.send}
            </button>
        </form>
        <p class="mt-8">
            <a
                class="text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                href=de
            >
                <span aria-hidden="true">"←"</span>
                " "
                {ui.back_to_page}
            </a>
        </p>
    }
}
