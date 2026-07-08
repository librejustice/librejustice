//! Page de recherche de textes `/textes` (lois et règlements, ADR 0114) —
//! page distincte de `/recherche` : le corpus articles est servi par un moteur
//! BM25 seul, avec ses propres filtres ; aucune recherche transverse.
//!
//! Rendue côté client comme `/recherche` (ADR 0063) : route non indexée,
//! mêmes îlots interactifs (barre, dropdowns). Hérite du title/description
//! racine.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::components::client_only::ClientOnly;
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::TextesView;

#[component]
pub fn TextesPage() -> impl IntoView {
    view! {
        <ClientOnly fallback=|| {
            view! { <TextesPageSkeleton /> }
        }>
            <TextesPageBody />
        </ClientOnly>
    }
}

/// Squelette statique du fallback SSR/1er rendu : même grille que le corps
/// (rail vide + colonne contenu, gabarit `/recherche`), barre inerte +
/// placeholder de liste. Aucun signal dépendant de `window`/`localStorage` →
/// hydratation sans divergence.
#[component]
fn TextesPageSkeleton() -> impl IntoView {
    view! {
        <div class="mx-auto w-full max-w-[92rem] px-4 py-8 sm:px-6 lg:px-8">
            <h1 class="sr-only">"Recherche de textes"</h1>
            <div class="grid grid-cols-1 gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                <div class="hidden lg:block"></div>
                <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
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
    // `/recherche`) : une mutation de filtre applique le texte non soumis.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));

    view! {
        <div class="mx-auto w-full max-w-[92rem] px-4 py-8 sm:px-6 lg:px-8">
            <h1 class="sr-only">"Recherche de textes"</h1>
            <TextesView />
        </div>
    }
}
