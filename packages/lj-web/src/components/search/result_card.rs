//! `ResultCard` (port de `result-card.tsx`). Numéral + titre lié + toggle
//! Extrait/Résumé + badges. Extrait et résumé arrivent tous deux dans le payload
//! de recherche (résumé garanti en base, joint au hot path — ADR 0051) : les deux
//! vues sont instantanées, sans aucun fetch.
//!
//! Navigation : lien `/decision/{id}`. Au clic, on pose la graine de navigation
//! (`ResultNavSeed` : `hit_ids`/position/total + origine recherche) consommée par
//! la barre décision de la page cible (port de `buildState`/`resultNav`/`fromSearch`,
//! cf. `components::decision_bar`).

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{FacetTag, SearchHit};

use crate::components::decision_bar::{
    use_result_nav, FromSearch, ResultNav, ResultNavSeed, ResultNavSignal,
};
use crate::helpers::{format_decision_jurisdiction, format_iso_date};

use super::compact_search::highlight::Highlighted;

/// Vue de contenu courante : extrait ou résumé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardView {
    Snippet,
    Summary,
}

/// Origine recherche capturée au clic : query string + scroll courant. Vide en
/// SSR (le clic n'y survient pas).
#[cfg(feature = "hydrate")]
fn navigation_origin() -> (String, f64) {
    web_sys::window()
        .map(|w| {
            (
                w.location().search().unwrap_or_default(),
                w.scroll_y().unwrap_or(0.0),
            )
        })
        .unwrap_or_default()
}

#[cfg(not(feature = "hydrate"))]
fn navigation_origin() -> (String, f64) {
    (String::new(), 0.0)
}

/// Pose la graine de navigation (origine recherche + position/total + `hit_ids`
/// pour prev/next). `hit_ids` est passé par référence (refcompté en amont) et non
/// via une valeur réactive : un `StoredValue` est lié à l'owner de `ResultList`,
/// disposé au re-render (revalidation SWR) → `get_value()` paniquerait dans ce
/// handler de clic différé, avortant la pose de graine.
fn set_nav_seed(seed: ResultNavSignal, position: i64, total: i64, hit_ids: &[String]) {
    let (search, scroll_y) = navigation_origin();
    seed.set(Some(ResultNavSeed {
        nav: Some(ResultNav {
            position,
            total,
            hit_ids: hit_ids.to_vec(),
        }),
        from_search: Some(FromSearch { search, scroll_y }),
    }));
}

#[component]
pub fn ResultCard(
    hit: SearchHit,
    index: usize,
    page: u32,
    #[prop(optional)] total: i64,
    page_size: u32,
    hit_ids: Arc<Vec<String>>,
    auto_load_summary: bool,
    animate: bool,
    /// Badge contextuel optionnel, en tête des badges de facettes (ex. rôle de
    /// l'entité sur sa fiche : Demandeur / Défendeur / Conseil).
    #[prop(optional_no_strip)]
    role_badge: Option<String>,
) -> impl IntoView {
    let view_sig = RwSignal::new(if auto_load_summary {
        CardView::Summary
    } else {
        CardView::Snippet
    });

    let position = (page as usize - 1) * page_size as usize + index + 1;
    let numeral = format!("{position:02}");
    let decision_href = format!("/decision/{}", hit.id);

    // Graine de navigation posée au clic : la barre décision de la page cible la
    // consomme (origine recherche + position/total + liste d'`id` pour prev/next).
    // Inerte en SSR (le clic ne s'y produit pas ; `navigation_origin` y est vide).
    let seed = use_result_nav();
    let seed_position = position as i64;
    let on_navigate_title = {
        let hit_ids = hit_ids.clone();
        move |_| set_nav_seed(seed, seed_position, total, &hit_ids)
    };
    let on_navigate_footer = {
        let hit_ids = hit_ids.clone();
        move |_| set_nav_seed(seed, seed_position, total, &hit_ids)
    };

    // Titre : `title_html` sinon fallback construit (juridiction, date, n° rôle).
    let title_html = if hit.title_html.is_empty() {
        let parts: Vec<String> = [
            Some(format_decision_jurisdiction(
                hit.jurisdiction_type,
                hit.jurisdiction_name.as_deref(),
            )),
            hit.date_lecture
                .as_deref()
                .map(|d| format_iso_date(Some(d))),
            hit.docket_numbers.as_ref().and_then(|d| d.first().cloned()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
        parts.join(", ")
    } else {
        hit.title_html.clone()
    };

    // Siège (chambre · formation/office) hors du titre (ADR 0170) — rendu à gauche
    // de la ligne méta, en regard des badges référentiels alignés à droite.
    let seat = hit.seat.clone().filter(|s| !s.trim().is_empty());

    // Extrait et résumé sont tous deux dans le payload (ADR 0051) : rendu direct.
    let embedded_summary = hit.summary.clone();
    let snippet = hit.best_chunk.snippet.clone();

    let hit_solution = hit.solution.clone();
    let hit_procedure = hit.procedure.clone();
    let hit_publication_codes = hit.publication_codes.clone();

    let style = if animate {
        Some(format!(
            "animation: var(--animate-rise); animation-delay: {}ms",
            index.min(8) * 35
        ))
    } else {
        None
    };

    let snippet_for_view = snippet.clone();
    let embedded_for_view = embedded_summary.clone();
    let content = move || {
        let v = view_sig.get();
        if v == CardView::Snippet {
            return view! {
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]">
                    <Highlighted text=snippet_for_view.clone() />
                </p>
            }
            .into_any();
        }
        // Vue résumé (embarqué dans le payload).
        match embedded_for_view.clone() {
            Some(s) => view! {
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]">{s}</p>
            }
            .into_any(),
            None => view! {
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-subtle)] italic">
                    "Résumé indisponible pour cette décision."
                </p>
            }
            .into_any(),
        }
    };

    let toggle_class = move |target: CardView| {
        let active = view_sig.get() == target;
        format!(
            "rounded px-2 py-0.5 text-xs font-medium transition-colors {}",
            if active {
                "bg-[var(--color-accent)] text-white"
            } else {
                "text-[var(--color-ink-subtle)] hover:text-[var(--color-ink)]"
            }
        )
    };
    let href_title = decision_href.clone();
    let href_footer = decision_href.clone();

    view! {
        <article
            class="group grid grid-cols-[auto_1fr] gap-x-6 border-t border-[var(--color-rule)] py-7"
            style=style
        >
            <span aria-hidden="true" class="hit-numeral pt-0.5">
                {numeral}
            </span>
            <div class="flex flex-col gap-2">
                <h3 class="font-sans text-lg leading-snug tracking-tight text-[var(--color-ink)]">
                    <A
                        href=href_title
                        on:click=on_navigate_title
                        attr:class="no-underline transition-colors hover:text-[var(--color-accent)]"
                    >
                        <Highlighted text=title_html />
                    </A>
                </h3>
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                    {seat
                        .map(|s| {
                            view! {
                                <p class="text-sm text-[var(--color-ink-subtle)]">{s}</p>
                            }
                        })}
                    <div class="ml-auto flex flex-wrap justify-end gap-1.5">
                        {role_badge
                            .map(|label| {
                                use crate::components::ui::{Badge, BadgeTone};
                                view! { <Badge tone=BadgeTone::Neutral>{label}</Badge> }
                            })}
                        <ResultMetaBadges
                            solution=hit_solution
                            procedure=hit_procedure
                            publication_codes=hit_publication_codes
                        />
                    </div>
                </div>
                {content}
                <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 pt-1">
                    <div class="flex items-center gap-1.5">
                        <button
                            type="button"
                            on:click=move |_| view_sig.set(CardView::Snippet)
                            class=move || toggle_class(CardView::Snippet)
                        >
                            "Extrait"
                        </button>
                        <button
                            type="button"
                            on:click=move |_| view_sig.set(CardView::Summary)
                            class=move || toggle_class(CardView::Summary)
                        >
                            "Résumé"
                        </button>
                    </div>
                    <A
                        href=href_footer
                        on:click=on_navigate_footer
                        attr:class="inline-flex items-center gap-1 text-sm font-medium text-[var(--color-ink)] no-underline transition-colors group-hover:text-[var(--color-accent)]"
                    >
                        "Consulter la décision"
                        <span
                            aria-hidden="true"
                            class="transition-transform group-hover:translate-x-0.5"
                        >
                            "→"
                        </span>
                    </A>
                </div>
            </div>
        </article>
    }
}

/// Badges depuis les tags référentiels servis (ADR 0146) : solution (sauf
/// AUTRE), voie, portée notable (majeure/importante, ADR 0167 — lecture
/// normalisée inter-ordres qui remplace le badge publication brut ; le détail
/// « Publié au bulletin » reste dans la Synthèse de la décision).
#[component]
fn ResultMetaBadges(
    solution: Option<FacetTag>,
    procedure: Option<FacetTag>,
    publication_codes: Vec<String>,
) -> impl IntoView {
    use crate::components::ui::{Badge, BadgeTone};
    use crate::pages::decision_page::labels::significance_badge;

    let solution_badge = solution.filter(|t| t.key != "AUTRE").map(|t| t.label);
    let procedure_badge = procedure.map(|t| t.label);
    let significance = significance_badge(&publication_codes);

    view! {
        {solution_badge
            .map(|label| view! { <Badge tone=BadgeTone::Outline>{label}</Badge> })}
        {procedure_badge.map(|label| view! { <Badge tone=BadgeTone::Accent>{label}</Badge> })}
        {significance.map(|label| view! { <Badge tone=BadgeTone::Neutral>{label}</Badge> })}
    }
}
