//! Rail gauche de `/decision-search` : synthèse cliquable des résultats
//! (histogramme des années, articles les plus cités, juridictions, solution).
//! Rendu dans tous les états, la requête vide comprise : le mode parcours sert
//! les facettes du fonds. Desktop uniquement, le conteneur `aside` de la page
//! étant `hidden lg:block` ;
//! sur mobile la volumétrie vit dans la rangée tri de la colonne résultats.
//! La volumétrie desktop, l'aide à la recherche et le pont croisé vivent dans
//! la colonne DROITE de la page (`search_page.rs`).
//!
//! Chaque clic passe par la MÊME mutation d'URL que les dropdowns de la barre
//! de filtres (`query_state` + `Nav`) : chip active, nettoyage par la chip ou
//! « Tout effacer », retour page 1. Les blocs lisent les facettes remontées de
//! la réponse (mises à jour sur Ok seulement) : pendant un refetch de filtre,
//! de tri ou de page ils restent affichés et interactifs, comme la barre de
//! filtres. Question changée en revanche (`loading`), le rail passe en attente :
//! les facettes en main résument les résultats de la question précédente. Leurs
//! nombres se taisent dès que le décompte porte sur d'autres filtres
//! (`StaleCounts`), histogramme compris : une hauteur de barre est un compte.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use leptos_router::params::ParamsMap;
use lj_dtos::{SearchFacets, UiAction};

use crate::components::ui::Skeleton;
use crate::helpers::group_thousands;

use super::compact_search::{query_state, DraftQuery};
use super::facet_widgets::{stale_counts, Nav};

/// Lignes par bloc de facettes.
const ROWS: usize = 5;
/// Années max de l'histogramme (les plus récentes).
const YEARS: usize = 20;

/// Map courante avec `q` réécrit par le texte (non soumis) de la barre — même
/// mécanique que `DecisionsFilterBar` : une mutation depuis le rail applique
/// requête et filtre d'un seul geste.
pub(super) fn effective_map(map: ParamsMap, draft: Option<RwSignal<String>>) -> ParamsMap {
    let mut map = map;
    if let Some(draft) = draft {
        let q = draft.get_untracked().trim().to_string();
        if q.is_empty() {
            map.remove("q");
        } else {
            map.replace("q".to_string(), q);
        }
    }
    map
}

#[component]
pub fn SearchRail(
    #[prop(into)] facets: Signal<Option<SearchFacets>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let ui = &crate::i18n::use_ui().search;
    let query_map = use_query_map();
    let nav = Nav::new();
    let draft = use_context::<DraftQuery>().map(|d| d.0);
    let stale = stale_counts();

    let toggle = Callback::new(move |(key, value): (&'static str, String)| {
        nav.go(query_state::toggle_multi(
            &effective_map(query_map.get_untracked(), draft),
            key,
            &value,
        ));
    });

    // ── Histogramme des années ────────────────────────────────────────────────
    let years = move || {
        // Une barre EST son compte : sa hauteur ment comme mentirait le nombre.
        if stale.get() {
            return skeleton_block(
                ui.rail_years,
                view! { <Skeleton class="h-10 w-full" /> }.into_any(),
            );
        }
        let Some(f) = facets.get() else {
            return ().into_any();
        };
        let mut years: Vec<(i32, i64)> = f
            .date_lecture_year
            .iter()
            .filter_map(|c| c.value.parse::<i32>().ok().map(|y| (y, c.count)))
            .collect();
        if years.len() < 2 {
            return ().into_any();
        }
        years.sort_by_key(|(y, _)| std::cmp::Reverse(*y));
        years.truncate(YEARS);
        years.reverse();
        let max = years.iter().map(|(_, c)| *c).max().unwrap_or(1).max(1);
        let (first, last) = (years[0].0, years[years.len() - 1].0);
        let map = query_map.get();
        let (from, to) = (map.get("dateFrom"), map.get("dateTo"));
        let bars = years
            .into_iter()
            .map(|(year, count)| {
                let year_from = format!("{year}-01-01");
                let year_to = format!("{year}-12-31");
                let active =
                    from.as_deref() == Some(&year_from) && to.as_deref() == Some(&year_to);
                let height = (count * 36 / max).max(3);
                let bar_class = if active {
                    "flex-1 rounded-t-[1px] bg-[var(--color-bordeaux)]"
                } else {
                    "flex-1 rounded-t-[1px] bg-[var(--color-rule)] transition-colors hover:bg-[var(--color-ink-subtle)]"
                };
                view! {
                    <button
                        type="button"
                        class=bar_class
                        style=format!("height:{height}px")
                        title=format!("{year} · {}", group_thousands(numbers, count))
                        on:click=move |_| {
                            let map = effective_map(query_map.get_untracked(), draft);
                            let qs = if active {
                                query_state::with_dates(&map, None, None)
                            } else {
                                query_state::with_dates(&map, Some(&year_from), Some(&year_to))
                            };
                            nav.go(qs);
                        }
                    ></button>
                }
            })
            .collect_view();
        view! {
            <div
                class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4"
                data-usage=UiAction::SearchRailYears.as_str()
            >
                <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.rail_years}
                </p>
                <div class="flex h-10 items-end gap-px">{bars}</div>
                <div class="flex justify-between font-mono text-[10px] text-[var(--color-ink-subtle)]">
                    <span>{first}</span>
                    <span>{last}</span>
                </div>
            </div>
        }
        .into_any()
    };

    // ── Blocs liste (textes cités, juridictions, solutions) ──────────────────
    let selected = move |key: &str| query_map.get().get_all(key).unwrap_or_default();

    let cited = move || {
        let Some(f) = facets.get() else {
            return ().into_any();
        };
        let picked = selected("legalArticle");
        let rows: Vec<RailRow> = f
            .legal_article_top
            .into_iter()
            .take(ROWS)
            .map(|a| RailRow {
                active: picked.contains(&a.value),
                label: a.label,
                count: a.count,
                value: a.value,
            })
            .collect();
        rail_list(
            stale,
            ui.rail_articles,
            "legalArticle",
            UiAction::SearchRailArticles,
            rows,
            toggle,
        )
    };

    let jurisdictions = move || {
        let Some(f) = facets.get() else {
            return ().into_any();
        };
        let picked = selected("seat");
        let mut types: Vec<_> = f
            .seat
            .into_iter()
            .filter(|c| {
                c.parent
                    .as_deref()
                    .is_some_and(|order| !order.contains('.'))
            })
            .collect();
        types.sort_by_key(|c| std::cmp::Reverse(c.count));
        let rows: Vec<RailRow> = types
            .into_iter()
            .take(ROWS)
            .map(|c| RailRow {
                active: picked.contains(&c.value),
                label: c.label,
                count: c.count,
                value: c.value,
            })
            .collect();
        rail_list(
            stale,
            ui.rail_courts,
            "seat",
            UiAction::SearchRailSeats,
            rows,
            toggle,
        )
    };

    let solutions = move || {
        let Some(f) = facets.get() else {
            return ().into_any();
        };
        let picked = selected("solution");
        let mut items = f.solution;
        items.sort_by_key(|c| std::cmp::Reverse(c.count));
        let rows: Vec<RailRow> = items
            .into_iter()
            .take(ROWS)
            .map(|c| RailRow {
                active: picked.contains(&c.value),
                label: c.label,
                count: c.count,
                value: c.value,
            })
            .collect();
        rail_list(
            stale,
            ui.rail_solution,
            "solution",
            UiAction::SearchRailSolutions,
            rows,
            toggle,
        )
    };

    move || {
        if loading.get() {
            return rail_skeleton();
        }
        view! {
            <div class=RAIL_COLUMN>
                {years}
                {cited}
                {jurisdictions}
                {solutions}
            </div>
        }
        .into_any()
    }
}

/// Rail en attente : mêmes titres, même gabarit, lignes muettes le temps que la
/// nouvelle question réponde.
fn rail_skeleton() -> AnyView {
    let ui = &crate::i18n::use_ui().search;
    view! {
        <div class=RAIL_COLUMN>
            {skeleton_block(ui.rail_years, view! { <Skeleton class="h-10 w-full" /> }.into_any())}
            {skeleton_block(ui.rail_articles, skeleton_rows(ROWS))}
            {skeleton_block(ui.rail_courts, skeleton_rows(ROWS))}
            {skeleton_block(ui.rail_solution, skeleton_rows(ROWS))}
        </div>
    }
    .into_any()
}

/// Colonne de rail : le premier bloc perd son filet haut (pas de séparateur au
/// sommet d'une colonne), les suivants gardent le leur.
pub(super) const RAIL_COLUMN: &str =
    "flex flex-col gap-4 [&>div:first-child]:border-t-0 [&>div:first-child]:pt-0";

/// Bloc de rail en attente : le titre, qui ne dépend pas de la réponse, et un
/// corps muet. Partagé par les deux moteurs — même gabarit de colonne.
pub(super) fn skeleton_block(title: &'static str, body: AnyView) -> AnyView {
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {body}
        </div>
    }
    .into_any()
}

/// `n` lignes muettes au gabarit d'une ligne de facette.
pub(super) fn skeleton_rows(n: usize) -> AnyView {
    (0..n)
        .map(|_| view! { <Skeleton class="h-4 w-full" /> })
        .collect_view()
        .into_any()
}

struct RailRow {
    label: String,
    count: i64,
    value: String,
    active: bool,
}

/// Bloc liste du rail : titre + lignes `libellé … compte`, cliquables. Le compte
/// se tait quand il porte sur l'ensemble d'avant le clic.
fn rail_list(
    stale: Signal<bool>,
    title: &'static str,
    key: &'static str,
    usage: UiAction,
    rows: Vec<RailRow>,
    toggle: Callback<(&'static str, String)>,
) -> AnyView {
    let numbers = &crate::i18n::use_ui().numbers;
    if rows.is_empty() {
        return ().into_any();
    }
    let rows = rows
        .into_iter()
        .map(|row| {
            let value = row.value;
            let count = row.count;
            let label_class = if row.active {
                "truncate text-[13px] leading-snug font-medium text-[var(--color-bordeaux)]"
            } else {
                "truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors group-hover:text-[var(--color-ink)]"
            };
            view! {
                <button
                    type="button"
                    class="group flex w-full items-baseline justify-between gap-3 text-left"
                    title=row.label.clone()
                    on:click=move |_| toggle.run((key, value.clone()))
                >
                    <span class=label_class>{row.label.clone()}</span>
                    <Show when=move || !stale.get()>
                        <span class="shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]">
                            {group_thousands(numbers, count)}
                        </span>
                    </Show>
                </button>
            }
        })
        .collect_view();
    view! {
        <div
            class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4"
            data-usage=usage.as_str()
        >
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {rows}
        </div>
    }
    .into_any()
}
