//! Annuaire des entités, point d'entrée vers les fiches `/entity/{juris}/{ns}/{id}`.
//! Deux surfaces :
//!
//! - `/annuaire` ([`AnnuairePage`]) : recherche + cartes de catégories (avec
//!   compteurs) + résultats quand `?q=` est présent.
//! - `/entity-index/{juris}/{kind}` ([`AnnuaireDirectoryPage`]): a paginated listing, or a name
//!   search when the category holds natural persons.
//!
//! Gabarit commun /decisions · /textes · /texte : conteneur 92rem, grille rail
//! (240px) + colonne contenu bornée 3xl ; le rail des catégories
//! ([`common::AnnuaireRail`]) vit dans la gouttière gauche des deux pages.
//!
//! Rendu SSR crawlable (routes en `PartiallyBlocked`, parité fiche entité) :
//! résultats de recherche et listing sont des ressources **bloquantes** (dans le
//! HTML initial) ; les compteurs (rail + cartes) sont streamés (non bloquants).
//! Transport des données via [`crate::api::ApiClient`] (in-process au SSR, HTTP
//! `/internal/entities/*` côté hydrate).

mod common;
mod directory;

pub use directory::AnnuaireDirectoryPage;

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::use_query_map;
use lj_dtos::AnnuaireStatsResponse;

use crate::components::layout::{CONTENT_COL, GRID_2COL, SHELL};
use crate::components::own_page_description;
use crate::helpers::group_thousands;
use crate::pages::annuaire::common::{
    entity_row, fetch_search, list_skeleton, max_decision_count, mini_bar, stats_resource,
    status_note, AnnuaireRail, Kind, SEARCH_MIN_QUERY,
};
use crate::pages::decision_page::data::sendable;
use crate::seo::CANONICAL_BASE;

/// Terme de recherche courant depuis `?q=` (vide si absent).
fn query_term() -> Signal<String> {
    let query = use_query_map();
    Signal::derive(move || query.read().get("q").unwrap_or_default().trim().to_string())
}

#[component]
pub fn AnnuairePage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let term = query_term();
    let stats = stats_resource();
    let ui = &crate::i18n::use_ui().directory;
    let url = format!("{CANONICAL_BASE}/entity-search");

    view! {
        <Title text=ui.title />
        <Meta name="description" content=ui.description />
        <crate::seo::Canonical href=url />

        <div class=format!("{SHELL} flex-1")>
            // Gabarit commun /decisions · /textes · /texte : gouttière 240px,
            // colonne contenu bornée 3xl.
            <div class=GRID_2COL>
                <div class="hidden lg:block">
                    <AnnuaireRail stats=stats />
                </div>
                <div class=CONTENT_COL>
                    <header class="flex flex-col gap-2">
                        <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                            {ui.directory}
                        </p>
                        <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.heading}</h1>
                        <p class="max-w-prose text-[var(--color-ink-muted)]">{ui.intro}</p>
                    </header>

                    <SearchForm term=term />

                    {move || {
                        let q = term.get();
                        if q.is_empty() {
                            view! { <CategoryGrid stats=stats /> }.into_any()
                        } else if q.chars().count() < SEARCH_MIN_QUERY {
                            status_note(ui.min_query)
                        } else {
                            view! { <SearchResults term=term /> }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

/// Champ de recherche : formulaire GET natif (SSR-friendly, fonctionne sans JS)
/// vers `/entity-search?q=`.
#[component]
fn SearchForm(term: Signal<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().directory;
    view! {
        <form method="get" action="/entity-search" role="search" class="flex w-full gap-2">
            <input
                type="search"
                name="q"
                value=move || term.get()
                placeholder=ui.search_placeholder
                autocomplete="off"
                aria-label=ui.search_label
                class="min-w-0 flex-1 rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
            />
            <button
                type="submit"
                class="shrink-0 rounded-lg border border-[var(--color-accent)] bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
            >
                {ui.search}
            </button>
        </form>
    }
}

/// Grille des catégories : cartes SSR crawlables liant `/entity-index/{juris}/{kind}` ;
/// double compteur en tuile (total du registre chargé + entités liées à des
/// décisions) et barre de proportion (part de la catégorie dans la somme des
/// registres), streamés — rien tant que les stats ne sont pas là.
#[component]
fn CategoryGrid(stats: Resource<Option<AnnuaireStatsResponse>>) -> impl IntoView {
    let cards = Kind::ALL
        .into_iter()
        .map(|kind| view! { <CategoryCard kind=kind stats=stats /> })
        .collect_view();

    view! {
        <section aria-label=crate::i18n::use_ui().directory.categories>
            <ul class="grid grid-cols-1 gap-4 sm:grid-cols-2">{cards}</ul>
        </section>
    }
}

#[component]
fn CategoryCard(kind: Kind, stats: Resource<Option<AnnuaireStatsResponse>>) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let ui = &crate::i18n::use_ui().directory;
    let copy = kind.copy(ui);
    let href = format!("/entity-index/fr/{}", kind.slug());
    // Chiffres + barre streamés d'un bloc (part de la catégorie dans la somme
    // des registres) ; stats absentes (chargement / erreur) ⇒ bloc vide, la
    // carte reste rendue.
    let figures = move || {
        Suspend::new(async move {
            stats.await.map(|s| {
                let counts = kind.stats(&s);
                let total = Kind::ALL
                    .into_iter()
                    .map(|k| k.stats(&s).registre)
                    .sum::<i64>()
                    .max(1);
                view! {
                    <span class="flex items-baseline gap-1.5">
                        <span class="font-sans text-2xl leading-none text-[var(--color-ink)] tabular-nums">
                            {group_thousands(numbers, counts.registre)}
                        </span>
                        <span class="text-xs text-[var(--color-ink-subtle)]">{ui.in_register}</span>
                    </span>
                    {mini_bar(counts.registre, total)}
                    <span class="text-xs tabular-nums text-[var(--color-ink-muted)]">
                        {(ui.linked_to_decisions)(&group_thousands(numbers, counts.contentieux), copy.linked)}
                    </span>
                }
            })
        })
    };
    view! {
        <li>
            <A
                href=href
                attr:class="group flex h-full flex-col gap-1 rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-4 no-underline transition-colors hover:border-[var(--color-accent)]"
            >
                <span class="font-sans text-lg text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                    {copy.plural}
                </span>
                <span class="text-sm text-[var(--color-ink-muted)]">{copy.tagline}</span>
                <span class="mt-auto flex flex-col gap-1.5 pt-3">
                    <Suspense fallback=|| ()>{figures}</Suspense>
                </span>
            </A>
        </li>
    }
}

/// Résultats de recherche (`?q=`), bloquants SSR (liste dans le HTML initial).
/// Erreur API rendue en note sobre.
#[component]
fn SearchResults(term: Signal<String>) -> impl IntoView {
    let results = Resource::new_blocking(move || term.get(), |q| sendable(fetch_search(q)));
    let ui = &crate::i18n::use_ui().directory;
    // The parent renders one instance per term.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| crate::reads::report_entity_search(term.get_untracked()));
    view! {
        <section aria-label=ui.results>
            <Suspense fallback=list_skeleton>
                {move || Suspend::new(async move {
                    match results.await {
                        Ok(items) if items.is_empty() => {
                            status_note(ui.no_match)
                        }
                        Ok(items) => {
                            let max = max_decision_count(&items);
                            let rows = items
                                .into_iter()
                                .map(|item| entity_row(ui, item, max))
                                .collect_view();
                            view! { <ul class="mt-2 flex flex-col gap-3">{rows}</ul> }.into_any()
                        }
                        Err(err) => {
                            status_note((ui.search_unavailable)(&err.failure.text(&crate::i18n::use_ui().shell)))
                        }
                    }
                })}
            </Suspense>
        </section>
    }
}
