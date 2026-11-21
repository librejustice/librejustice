//! `ResultCard` : numéral + titre lié + toggle Extrait/Résumé + badges. Extrait
//! et résumé arrivent tous deux dans le payload de recherche (résumé garanti en
//! base, joint au hot path) : les deux vues sont instantanées, sans aucun fetch.
//!
//! Navigation: a `/decision/{id}` link, whose click pushes the visit's seed
//! (`hit_paths`, search origin, query to highlight) with its history entry, read
//! by the target page's decision bar (see `components::decision_bar`).

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{FacetTag, SearchHit, UiAction};

use crate::components::decision_bar::{carry_seed, search_origin, ResultNav, ResultNavSeed};
use crate::components::ui::{shown_summary, SummaryBadge, SummaryFold};
use crate::helpers::{format_decision_jurisdiction, format_iso_date, renamed, retitle};

use super::compact_search::highlight::Highlighted;
use super::view_prefs::CardView;

/// The seed of a visit opened from this result list. `hit_paths` is borrowed
/// from the list's `Arc` rather than read from a reactive value: a `StoredValue`
/// tied to the owner of `ResultList` is disposed on re-render (SWR revalidation),
/// and reading it in a deferred click handler would panic.
fn result_seed(hit_paths: &[String], highlight: Option<String>) -> ResultNavSeed {
    ResultNavSeed {
        nav: Some(ResultNav {
            hit_paths: hit_paths.to_vec(),
        }),
        from_search: Some(search_origin(highlight)),
    }
}

#[component]
pub fn ResultCard(
    hit: SearchHit,
    index: usize,
    page: u32,
    page_size: u32,
    hit_paths: Arc<Vec<String>>,
    /// Vue voulue par la bascule d'en-tête. Une carte peut y déroger ; le
    /// prochain geste sur la bascule reprend la main sur toutes.
    #[prop(into)]
    default_view: Signal<CardView>,
    animate: bool,
    /// Badge contextuel optionnel, en tête des badges de facettes (ex. rôle de
    /// l'entité sur sa fiche : Demandeur / Défendeur / Conseil).
    #[prop(optional_no_strip)]
    role_badge: Option<String>,
    /// The query whose terms the opened decision highlights.
    #[prop(optional_no_strip)]
    highlight: Option<String>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let dates = &crate::i18n::use_ui().dates;
    // Dérogation locale, effacée dès que la bascule d'en-tête bouge : sans ça
    // une carte dérogée resterait sourde à une commande d'ensemble.
    let local = RwSignal::new(None::<CardView>);
    Effect::new(move |prev: Option<CardView>| {
        let wanted = default_view.get();
        if prev.is_some_and(|before| before != wanted) {
            local.set(None);
        }
        wanted
    });
    let view_sig = Signal::derive(move || local.get().unwrap_or_else(|| default_view.get()));

    let position = (page as usize - 1) * page_size as usize + index + 1;
    let numeral = format!("{position:02}");
    let decision_href = lj_dtos::decision_path(
        hit.jurisdiction_type,
        hit.jurisdiction_code.as_deref(),
        &hit.id,
    );

    // The visit's seed rides in its history entry, set on the link at click.
    let on_navigate_title = {
        let hit_paths = hit_paths.clone();
        let highlight = highlight.clone();
        move |ev: leptos::ev::MouseEvent| {
            carry_seed(&ev, &result_seed(&hit_paths, highlight.clone()));
        }
    };
    let on_navigate_footer = {
        let hit_paths = hit_paths.clone();
        move |ev: leptos::ev::MouseEvent| {
            carry_seed(&ev, &result_seed(&hit_paths, highlight.clone()));
        }
    };

    let named = hit
        .court_names
        .get(crate::i18n::use_reader_lang().get().code())
        .cloned();
    // A court named in the reader's language composes its title anew, the API's being French.
    let title_html = if hit.title_html.is_empty() || named.is_some() {
        let jurisdiction = match &named {
            Some(named) => named.name.clone(),
            None => format_decision_jurisdiction(
                hit.jurisdiction_type,
                hit.jurisdiction_name.as_deref(),
            ),
        };
        let parts: Vec<String> = [
            Some(jurisdiction),
            hit.date_lecture
                .as_deref()
                .map(|d| format_iso_date(dates, Some(d))),
            hit.docket_numbers.as_ref().and_then(|d| d.first().cloned()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
        parts.join(", ")
    } else {
        retitle(dates, &hit.title_html, hit.date_lecture.as_deref())
    };

    let seat = crate::i18n::use_ui()
        .referential
        .named_seat(hit.seat.clone(), hit.seat_parts.as_ref())
        .filter(|s| !s.trim().is_empty())
        .map(|seat| match &named {
            Some(named) => renamed(named, &seat, None),
            None => seat,
        });
    let matched_docket = hit.matched_docket.clone();

    // Extrait et résumé sont tous deux dans le payload : rendu direct.
    let shown = hit.summary.as_deref().map(|summary| {
        shown_summary(
            hit.jurisdiction_type,
            summary,
            hit.summary_quote.as_ref(),
            hit.summary_body.as_ref(),
        )
    });
    let jurisdiction_type = hit.jurisdiction_type;
    let snippet = hit.best_chunk.snippet.clone();

    let hit_solution = hit.solution.clone();
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
        // Summary view (embedded in the payload), the snippet when there is none.
        match shown.clone() {
            Some(shown) => view! {
                <SummaryFold
                    text=shown.text
                    class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]"
                >
                    <SummaryBadge jurisdiction_type court=shown.court class="mr-2 align-middle" />
                </SummaryFold>
            }
            .into_any(),
            None => view! {
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]">
                    <Highlighted text=snippet_for_view.clone() />
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
                    {matched_docket
                        .map(|n| {
                            view! {
                                <p class="text-sm text-[var(--color-ink-subtle)]">
                                    {ui.also_docket}
                                    <span class="font-medium text-[var(--color-ink)]">{n}</span>
                                </p>
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
                            publication_codes=hit_publication_codes
                        />
                    </div>
                </div>
                {content}
                <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 pt-1">
                    <div
                        class="flex items-center gap-1.5"
                        data-usage=UiAction::SearchCardView.as_str()
                    >
                        <button
                            type="button"
                            on:click=move |_| local.set(Some(CardView::Snippet))
                            class=move || toggle_class(CardView::Snippet)
                        >
                            {ui.excerpt}
                        </button>
                        <button
                            type="button"
                            on:click=move |_| local.set(Some(CardView::Summary))
                            class=move || toggle_class(CardView::Summary)
                        >
                            {ui.summary}
                        </button>
                    </div>
                    <A
                        href=href_footer
                        on:click=on_navigate_footer
                        attr:class="inline-flex items-center gap-1 text-sm font-medium text-[var(--color-ink)] no-underline transition-colors group-hover:text-[var(--color-accent)]"
                    >
                        {ui.open_decision}
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

/// Badges from the served referential tags: the solution (except AUTRE) and a
/// notable reach (major or important), read alike across orders. The
/// « Publié au bulletin » detail stays in the decision's Synthèse.
#[component]
fn ResultMetaBadges(solution: Option<FacetTag>, publication_codes: Vec<String>) -> impl IntoView {
    use crate::components::ui::{Badge, BadgeTone};
    use crate::pages::decision_page::labels::publication_badge;

    let ui = crate::i18n::use_ui();
    let solution_badge = solution
        .filter(|t| t.key != "AUTRE")
        .map(|t| crate::i18n::ReferentialUi::label(ui.referential.solution, &t));
    let publication = publication_badge(&publication_codes, &ui.decision);

    view! {
        {solution_badge
            .map(|label| view! { <Badge tone=BadgeTone::Outline>{label}</Badge> })}
        {publication.map(|label| view! { <Badge tone=BadgeTone::Neutral>{label}</Badge> })}
    }
}
