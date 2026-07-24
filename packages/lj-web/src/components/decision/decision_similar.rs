//! Décisions similaires (KNN) + barre d'actions. Port de `decision-similar.tsx`.
//!
//! Sauter vers une voisine propage l'origine recherche (`from_search`) via la
//! graine de nav (`components::decision_bar`), sans `resultNav` (une voisine
//! n'est pas une position de résultat) : le bouton retour pointe toujours vers
//! la liste d'origine. Port du `fromSearch` propagé par `location.state` RR.
use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{DecisionDetail, SimilarDecisionHit};

use crate::components::decision_bar::{use_decision_bar, use_result_nav, ResultNavSeed};
use crate::components::ui::{Badge, BadgeTone};
use crate::helpers::{format_decision_jurisdiction, format_iso_date};
use crate::pages::decision_page::labels::significance_badge;
use crate::seo::decision::first_sentence;

use super::decision_header::DecisionActions;

#[component]
pub fn DecisionSimilar(
    detail: DecisionDetail,
    hits: Vec<SimilarDecisionHit>,
    #[prop(default = None)] error: Option<String>,
    #[prop(optional)] loading: bool,
) -> impl IntoView {
    let loading_view = loading.then(|| {
        view! {
            <ul class="flex flex-col gap-3" aria-hidden="true">
                {(0..4)
                    .map(|_| {
                        view! {
                            <li class="h-16 animate-pulse rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50" />
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    });

    let error_msg = error.clone();
    let error_view = (!loading)
        .then(|| error_msg.clone())
        .flatten()
        .map(|msg| {
            view! {
                <p class="rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50 p-3 text-sm text-[var(--color-ink-muted)]">
                    {msg}
                </p>
            }
            .into_any()
        });

    let empty_view = (!loading && error.is_none() && hits.is_empty()).then(|| {
        view! {
            <p class="rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50 p-3 text-sm text-[var(--color-ink-muted)]">
                "Aucun voisin suffisamment proche pour cette décision."
            </p>
        }
        .into_any()
    });

    let hits_view = (!hits.is_empty()).then(|| {
        let cards = hits
            .into_iter()
            .map(|hit| view! { <SimilarCard hit=hit /> })
            .collect_view();
        view! { <ul class="flex flex-col gap-3">{cards}</ul> }.into_any()
    });

    view! {
        <aside
            aria-label="Décisions similaires"
            class="flex flex-col gap-4 lg:sticky lg:top-20 lg:self-start"
        >
            <DecisionActions detail=detail />
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Décisions similaires"</h2>
            {loading_view}
            {error_view}
            {empty_view}
            {hits_view}
        </aside>
    }
}

#[component]
fn SimilarCard(hit: SimilarDecisionHit) -> impl IntoView {
    let mut title_parts: Vec<String> = vec![format_decision_jurisdiction(
        hit.jurisdiction_type,
        hit.jurisdiction_name.as_deref(),
    )];
    if let Some(date) = hit.date_lecture.as_deref() {
        title_parts.push(format_iso_date(Some(date)));
    }
    if let Some(docket) = hit.docket_numbers.as_ref().and_then(|d| d.first()) {
        title_parts.push(docket.clone());
    }
    let title = title_parts.join(", ");
    let href = format!("/decision/{}", hit.id);

    // Saut vers une voisine : conserve l'origine recherche, abandonne la nav
    // inter-résultats (parité `buildState` des voisines).
    let bar = use_decision_bar();
    let seed = use_result_nav();
    let on_navigate = move |_| {
        let from_search = bar.get_untracked().and_then(|b| b.from_search);
        seed.set(Some(ResultNavSeed {
            nav: None,
            from_search,
        }));
    };

    let summary_view = hit.summary.as_deref().map(|s| {
        let snippet = first_sentence(s);
        view! {
            <p class="line-clamp-3 text-sm leading-snug text-[var(--color-ink-muted)]">{snippet}</p>
        }
        .into_any()
    });

    view! {
        <li class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3">
            <div class="flex flex-col gap-2">
                <h3 class="font-sans text-sm leading-snug text-[var(--color-ink)]">
                    <A
                        href=href
                        on:click=on_navigate
                        attr:class="no-underline transition-colors hover:text-[var(--color-accent)]"
                    >
                        {title}
                    </A>
                </h3>
                {summary_view}
                <SimilarMeta hit=hit />
            </div>
        </li>
    }
}

#[component]
fn SimilarMeta(hit: SimilarDecisionHit) -> impl IntoView {
    // Badges depuis les tags référentiels servis (ADR 0146). `procedure` absente =
    // procédure ordinaire (pas de badge). Portée notable (ADR 0167) en lieu et
    // place du badge publication brut, comme sur les cartes résultat.
    let solution = hit.solution.clone();
    let procedure = hit.procedure.clone();
    let pub_badge = significance_badge(&hit.publication_codes);

    if solution.is_none() && procedure.is_none() && pub_badge.is_none() {
        return ().into_any();
    }

    let solution_badge = solution.map(|t| {
        view! {
            <Badge tone=BadgeTone::Outline>{t.label}</Badge>
        }
        .into_any()
    });
    let procedure_badge = procedure.map(|t| {
        view! {
            <Badge tone=BadgeTone::Accent>{t.label}</Badge>
        }
        .into_any()
    });
    let publication_badge_view = pub_badge.map(|label| {
        view! {
            <Badge tone=BadgeTone::Neutral>{label}</Badge>
        }
        .into_any()
    });

    view! {
        <div class="flex flex-wrap gap-1.5">
            {solution_badge}
            {procedure_badge}
            {publication_badge_view}
        </div>
    }
    .into_any()
}
