//! `HeroSearch` (port de `hero-search.tsx`). Îlot interactif de la landing :
//! eyebrow + titre + accroche + formulaire + chips d'exemples. Soumet vers
//! `/recherche?q=…` (+ `aiMode`).
//!
//! L'`<input>` est rendu inline (mêmes classes que `ui::Input`, taille `h-14`)
//! pour un champ contrôlé — cf. `compact_search` (l'`Input` substrate ne propage
//! ni valeur ni `on:input`).

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};

use super::compact_search::ai_mode::use_ai_mode;
use super::compact_search::query_state;
use super::search_submit::{SearchSubmit, SubmitSize};
use super::syntax_hint::SyntaxHint;

const EXAMPLES: &[&str] = &[
    "responsabilité médicale infection nosocomiale",
    "permis de construire ET illégalité externe",
    "OQTF SAUF rétention",
    "trouble anormal de voisinage indemnisation",
];

#[component]
pub fn HeroSearch() -> impl IntoView {
    let query_map = use_query_map();
    let navigate = use_navigate();
    let query = RwSignal::new(String::new());
    let (ai_mode, set_ai_mode) = use_ai_mode();

    let submit = move |value: String| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Depuis la landing, pas de filtres existants à conserver : on part d'un
        // map vide (parité `new URLSearchParams({ q })`).
        let qs = query_state::with_query(
            &leptos_router::params::ParamsMap::new(),
            &trimmed,
            ai_mode.get_untracked(),
        );
        let _ = &query_map;
        navigate(&query_state::search_href(&qs), Default::default());
    };

    let submit_form = submit.clone();
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        submit_form(query.get_untracked());
    };

    // Sous-blocs erasés en `AnyView` : casse la profondeur du type-tuple SSR
    // pour limiter la pression sur `recursion_limit` (cf. note tranche : la
    // crate a besoin de `#![recursion_limit = "512"]` côté substrate).
    let intro = view! {
        <div class="flex flex-col gap-4">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Accès libre et gratuit"
            </p>
            <h1
                class="text-balance font-sans text-3xl leading-[1.02] tracking-tight text-[var(--color-ink)] sm:text-5xl lg:text-[3.85rem]"
                style="font-variation-settings: 'wght' 300"
            >
                "La jurisprudence française, "
                <em
                    class="not-italic text-[var(--color-accent)]"
                    style="font-variation-settings: 'wght' 750"
                >
                    "pour tous."
                </em>
            </h1>
            <p class="max-w-2xl text-base leading-relaxed text-[var(--color-ink-muted)] sm:text-lg">
                "Recherchez dans les décisions de justice françaises rendues publiques."
            </p>
        </div>
    }
    .into_any();

    let form = view! {
        <form on:submit=on_submit class="flex flex-col gap-2.5">
            <div class="flex flex-col gap-2 sm:flex-row">
                <div class="group flex h-14 w-full min-w-0 items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 text-base transition-colors has-[:focus-visible]:border-[var(--color-ink)] sm:flex-1">
                    <span class="text-[var(--color-ink-subtle)]" aria-hidden="true">
                        <SearchIcon />
                    </span>
                    <input
                        aria-label="Mots-clés ou question"
                        size="1"
                        placeholder="Mots-clés, question, ou expression exacte…"
                        autofocus
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        class="h-full min-w-0 flex-1 bg-transparent text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-subtle)]"
                    />
                </div>
                <SearchSubmit ai_mode=ai_mode on_toggle=set_ai_mode size=SubmitSize::Lg />
            </div>
            <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
                <SyntaxHint />
                <div class="flex items-center gap-4">
                    <A
                        href="/recherche"
                        attr:class="flex items-center gap-1.5 text-xs text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-ink)] hover:underline"
                    >
                        <svg viewBox="0 0 14 14" class="h-3.5 w-3.5" fill="none" aria-hidden="true">
                            <path
                                d="M2 7h10M7 2l5 5-5 5"
                                stroke="currentColor"
                                stroke-width="1.4"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                            />
                        </svg>
                        "Recherche avancée"
                    </A>
                </div>
            </div>
        </form>
    }
    .into_any();

    let examples = view! {
        <div class="flex flex-col gap-2.5">
            <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Exemples de requêtes"
            </p>
            <ul class="flex flex-wrap gap-2">
                {EXAMPLES
                    .iter()
                    .map(|example| {
                        let submit = submit.clone();
                        let ex = (*example).to_string();
                        view! {
                            <li>
                                <button
                                    type="button"
                                    on:click=move |_| submit(ex.clone())
                                    class="rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-1 text-sm text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                                >
                                    {*example}
                                </button>
                            </li>
                        }
                    })
                    .collect::<Vec<_>>()}
            </ul>
        </div>
    }
    .into_any();

    view! {
        <section class="mx-auto flex max-w-3xl flex-col gap-6 px-4 pt-8 pb-5 sm:px-6 sm:pt-10 sm:pb-4 lg:pt-12 lg:pb-4">
            {intro}
            {form}
            {examples}
        </section>
    }
}

#[component]
fn SearchIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 20 20" class="h-5 w-5">
            <circle cx="9" cy="9" r="6" fill="none" stroke="currentColor" stroke-width="1.6" />
            <path
                d="M14 14l4 4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
            />
        </svg>
    }
}
