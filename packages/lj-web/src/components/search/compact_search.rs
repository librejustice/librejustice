//! `CompactSearch` (port de `compact-search.tsx`) + point d'ancrage des modules
//! privés de la tranche recherche.
//!
//! Le `mod.rs` de `search/` est figé par la substrate et n'expose que les
//! composants publics. Les helpers internes de la tranche (état URL, parsing,
//! highlight, mode IA, fetch) sont donc rattachés ici via `#[path]` : ils
//! deviennent des sous-modules de `compact_search`, accessibles depuis les
//! autres composants via `super::compact_search::…`.
//!
//! L'`<input>` est rendu inline (mêmes classes que `ui::Input`) plutôt que via
//! le composant `Input` : ce dernier ne propage ni la valeur ni `on:input` au
//! champ interne (slots `ViewFn` `Send + Sync` seulement), or on a besoin d'un
//! champ contrôlé. Le DOM/les classes restent ceux d'`Input` (parité visuelle).

#[path = "ai_mode.rs"]
pub mod ai_mode;
#[path = "highlight.rs"]
pub mod highlight;
#[path = "query_state.rs"]
pub mod query_state;
#[path = "search_data.rs"]
pub mod search_data;
#[path = "search_state.rs"]
pub mod search_state;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use self::ai_mode::use_ai_mode;
use super::search_submit::{SearchSubmit, SubmitSize};

/// Texte courant de la barre de recherche, partagé via contexte avec le rail de
/// filtres. Cocher un filtre applique CE texte comme `q` (même non soumis), pour
/// ajuster requête + filtres d'un seul geste. Fourni par `SearchPage` ; lu par
/// `CompactSearch` (qui l'édite) et `FilterRail` (qui le lit à la mutation).
#[derive(Clone, Copy)]
pub struct DraftQuery(pub RwSignal<String>);

/// Barre de recherche compacte (page /recherche). Soumet vers `/recherche?q=…`
/// en conservant la source et les filtres courants.
#[component]
pub fn CompactSearch() -> impl IntoView {
    let query_map = use_query_map();
    let navigate = use_navigate();

    let initial = query_map.get_untracked().get("q").unwrap_or_default();
    // Signal partagé avec le rail (contexte `DraftQuery`) ; fallback local si la
    // barre est montée hors page recherche (pas de provider).
    let query = use_context::<DraftQuery>()
        .map(|d| d.0)
        .unwrap_or_else(|| RwSignal::new(initial));
    let (ai_mode, set_ai_mode) = use_ai_mode();
    // Corpus actif par la route (`/textes` vs `/recherche`) : le mode IA
    // (rerank + résumés) n'existe que pour les décisions — toggle masqué et
    // jamais soumis côté textes ; le placeholder suit.
    let is_textes = Signal::derive(query_state::on_textes_path);
    let show_ai = Signal::derive(move || !is_textes.get());

    // Re-sync `q` (back/forward). `Memo` sur le SEUL param `q` : ne notifie qu'au
    // changement effectif de `q`. Une mutation de filtre (le rail navigue en
    // replace=true en conservant `q`) ne re-déclenche donc pas le set — sinon
    // cocher un filtre réécrirait le champ avec l'ancien `q` de l'URL, écrasant
    // le texte saisi mais pas encore soumis.
    let url_q = Memo::new(move |_| query_map.get().get("q").unwrap_or_default());
    Effect::new(move |_| {
        query.set(url_q.get());
    });

    let submit_nav = navigate.clone();
    let submit = move || {
        let trimmed = query.get_untracked().trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let qs = query_state::with_query(
            &query_map.get_untracked(),
            &trimmed,
            ai_mode.get_untracked() && show_ai.get_untracked(),
        );
        submit_nav(&query_state::search_href(&qs), Default::default());
    };

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        submit();
    };

    // `navigate` stocké en `StoredValue` (Copy) pour pouvoir reconstruire le
    // handler `on:click` à chaque rendu du `<Show>` (children doit être `Fn`).
    let clear_nav = StoredValue::new(navigate.clone());
    let on_clear = move |_| {
        query.set(String::new());
        let mut map = query_map.get_untracked().clone();
        map.remove("q");
        map.remove("page");
        let qs = map.to_query_string();
        let qs = qs.strip_prefix('?').unwrap_or(&qs).to_string();
        let target = query_state::search_href(&qs);
        clear_nav.with_value(|nav| nav(&target, Default::default()));
    };

    view! {
        <form on:submit=on_submit class="flex w-full items-center gap-2">
            <div class="group flex h-11 w-full min-w-0 flex-1 items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 transition-colors has-[:focus-visible]:border-[var(--color-ink)]">
                <span class="text-[var(--color-ink-subtle)]" aria-hidden="true">
                    <SearchIcon />
                </span>
                <input
                    aria-label="Rechercher"
                    name="q"
                    size="1"
                    placeholder=move || {
                        if is_textes.get() {
                            "Rechercher dans les codes et lois…"
                        } else {
                            "Rechercher dans la jurisprudence…"
                        }
                    }
                    prop:value=move || query.get()
                    on:input=move |ev| query.set(event_target_value(&ev))
                    class="h-full min-w-0 flex-1 bg-transparent text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-subtle)]"
                />
                <Show when=move || !query.get().is_empty()>
                    <span class="text-[var(--color-ink-subtle)]">
                        <button
                            type="button"
                            aria-label="Effacer la recherche"
                            on:click=on_clear
                            class="flex items-center justify-center rounded text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]"
                        >
                            <ClearIcon />
                        </button>
                    </span>
                </Show>
            </div>
            <SearchSubmit
                ai_mode=ai_mode
                on_toggle=set_ai_mode
                size=SubmitSize::Md
                show_ai=show_ai
            />
        </form>
    }
}

#[component]
fn ClearIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 20 20" class="h-4 w-4">
            <path
                d="M6 6l8 8M14 6l-8 8"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
            />
        </svg>
    }
}

#[component]
fn SearchIcon() -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 20 20" class="h-4 w-4">
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
