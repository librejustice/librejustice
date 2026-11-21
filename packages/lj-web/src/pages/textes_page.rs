//! Text search page `/norm-search` (laws and regulations), a page separate from
//! `/decision-search`: the article corpus is served by a BM25 only engine, with its
//! own filters; no cross search.
//!
//! Rendered client side like `/decision-search`: unindexed route, same interactive
//! islands (bar, dropdowns). A dynamic `<Title>` reflecting the URL's `q` query;
//! inherits the root description.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::components::client_only::ClientOnly;
use crate::components::layout::{CONTENT_COL, GRID_3COL, SHELL};
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::TextesView;

#[component]
pub fn TextesPage() -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    crate::pages::cache_public_5_min();
    let query_map = use_query_map();
    // `leptos_meta` garde cette clôture dans un contexte de durée de vie
    // applicative et la réévalue APRÈS le démontage de la route, pour restaurer
    // le titre — `query_map`, propriété de la route, est alors déjà libéré. Un
    // `get()` y panique ; `try_get` rend `None` et on sert le titre nu.
    let title = move || {
        let q = query_map
            .try_get()
            .and_then(|params| params.get("q"))
            .unwrap_or_default();
        let q = q.trim();
        if q.is_empty() {
            ui.texts_title.to_string()
        } else {
            (ui.texts_title_for)(q)
        }
    };
    view! {
        <Title text=title />
        <ClientOnly fallback=|| {
            view! { <TextesPageSkeleton /> }
        }>
            <TextesPageBody />
        </ClientOnly>
    }
}

/// Squelette statique du fallback SSR/1er rendu : même grille que le corps
/// (rail vide + colonne contenu, gabarit `/decision-search`), barre inerte +
/// placeholder de liste. Aucun signal dépendant de `window`/`localStorage` →
/// hydratation sans divergence.
#[component]
fn TextesPageSkeleton() -> impl IntoView {
    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().search.texts_page_title}</h1>
            <div class=GRID_3COL>
                <div class="hidden lg:block"></div>
                <div class=CONTENT_COL>
                    <div class="h-12 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/40"></div>
                    <div class="flex flex-col gap-4">
                        {(0..4)
                            .map(|_| {
                                view! {
                                    <div class="h-20 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/30"></div>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn TextesPageBody() -> impl IntoView {
    let query_map = use_query_map();
    // Texte de la barre partagé avec les filtres (même mécanique que
    // `/decision-search`) : une mutation de filtre applique le texte non soumis.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));

    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().search.texts_page_title}</h1>
            <TextesView />
        </div>
    }
}
