//! Rail gauche de `/recherche` : volumétrie + synthèse cliquable des résultats
//! (histogramme des années, textes les plus cités, juridictions, dispositif) +
//! aide à la recherche. Desktop uniquement — le conteneur `aside` de la page
//! est `hidden lg:block` ; sur mobile la volumétrie vit dans la rangée tri de
//! la colonne résultats.
//!
//! Chaque clic passe par la MÊME mutation d'URL que les dropdowns de la barre
//! de filtres (`query_state` + `Nav`) : chip active, nettoyage par la chip ou
//! « Tout effacer », retour page 1. Les blocs lisent les facettes remontées de
//! la réponse (mises à jour sur Ok seulement) : pendant un refetch ils restent
//! affichés et interactifs, comme la barre de filtres.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use leptos_router::params::ParamsMap;
use lj_dtos::{QueryMode, SearchFacets};

use crate::helpers::{format_results_count, group_thousands};

use super::compact_search::{query_state, DraftQuery};
use super::facet_widgets::{juridiction_root_value, Nav};
use super::syntax_hint::SyntaxHint;

/// Lignes par bloc de facettes.
const ROWS: usize = 5;
/// Années max de l'histogramme (les plus récentes).
const YEARS: usize = 20;

/// Map courante avec `q` réécrit par le texte (non soumis) de la barre — même
/// mécanique que `DecisionsFilterBar` : une mutation depuis le rail applique
/// requête et filtre d'un seul geste.
fn effective_map(map: ParamsMap, draft: Option<RwSignal<String>>) -> ParamsMap {
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
    #[prop(into)] query: Signal<String>,
    #[prop(into)] volume: Signal<Option<(i64, QueryMode)>>,
    #[prop(into)] facets: Signal<Option<SearchFacets>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    let draft = use_context::<DraftQuery>().map(|d| d.0);

    let toggle = Callback::new(move |(key, value): (&'static str, String)| {
        nav.go(query_state::toggle_multi(
            &effective_map(query_map.get_untracked(), draft),
            key,
            &value,
        ));
    });

    // ── Volumétrie ────────────────────────────────────────────────────────────
    let header = move || {
        if loading.get() {
            return view! {
                <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">
                    "Recherche en cours pour "
                    <em class="not-italic">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                    "…"
                </h2>
            }
            .into_any();
        }
        let Some((total, mode)) = volume.get() else {
            return ().into_any();
        };
        let mode_label = match mode {
            QueryMode::Lexical => "lexicale",
            QueryMode::Hybrid => "sémantique",
        };
        view! {
            <div class="flex flex-col gap-1">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {format_results_count(total)}
                </h2>
                <p class="text-sm leading-snug text-[var(--color-ink-subtle)]">
                    "pour "
                    <em class="text-[var(--color-ink)]">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                </p>
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {format!("Recherche {mode_label}")}
                </p>
            </div>
        }
        .into_any()
    };

    // ── Histogramme des années ────────────────────────────────────────────────
    let years = move || {
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
        let (from, to) = (map.get("from"), map.get("to"));
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
                        title=format!("{year} · {}", group_thousands(count))
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
            <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
                <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Années"
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
        let picked = selected("li");
        let mut items = f.legal_instrument;
        items.sort_by_key(|i| std::cmp::Reverse(i.count));
        let rows: Vec<RailRow> = items
            .into_iter()
            .take(ROWS)
            .map(|i| RailRow {
                active: picked.contains(&i.value),
                label: i.label,
                count: i.count,
                value: i.value,
            })
            .collect();
        rail_list("Textes les plus cités", "li", rows, toggle)
    };

    let jurisdictions = move || {
        let Some(f) = facets.get() else {
            return ().into_any();
        };
        let picked = selected("jur");
        let mut roots: Vec<_> = f
            .juridiction
            .into_iter()
            .filter(|c| c.parent.is_none())
            .collect();
        roots.sort_by_key(|c| std::cmp::Reverse(c.count));
        let rows: Vec<RailRow> = roots
            .into_iter()
            .take(ROWS)
            .map(|c| {
                let value = juridiction_root_value(&c.value);
                RailRow {
                    active: picked.contains(&value),
                    label: c.label,
                    count: c.count,
                    value,
                }
            })
            .collect();
        rail_list("Juridictions", "jur", rows, toggle)
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
        rail_list("Dispositif", "solution", rows, toggle)
    };

    view! {
        <Show when=move || !query.get().is_empty()>
            <div class="flex flex-col gap-4">
                {header}
                {years}
                {cited}
                {jurisdictions}
                {solutions}
                <div class="border-t border-[var(--color-rule)] pt-4">
                    <SyntaxHint />
                </div>
            </div>
        </Show>
    }
}

struct RailRow {
    label: String,
    count: i64,
    value: String,
    active: bool,
}

/// Bloc liste du rail : titre + lignes `libellé … compte`, cliquables.
fn rail_list(
    title: &'static str,
    key: &'static str,
    rows: Vec<RailRow>,
    toggle: Callback<(&'static str, String)>,
) -> AnyView {
    if rows.is_empty() {
        return ().into_any();
    }
    let rows = rows
        .into_iter()
        .map(|row| {
            let value = row.value;
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
                    <span class="shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]">
                        {group_thousands(row.count)}
                    </span>
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {rows}
        </div>
    }
    .into_any()
}
