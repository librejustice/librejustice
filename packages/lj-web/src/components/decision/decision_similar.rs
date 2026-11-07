//! Décisions similaires, deux volets empilés : « Décisions de principe
//! similaires » (similaires sur le fond) et « Décisions d'espèce similaires »
//! (similaires en fait). Colonne de droite en desktop, qui défile avec la page.
use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::SimilarDecisionHit;

use crate::components::decision_bar::{carry_seed, use_decision_bar, FromSearch, ResultNavSeed};
use crate::components::ui::{Badge, BadgeTone, ExpandableSummary, Skeleton};
use crate::helpers::{format_decision_jurisdiction, format_iso_date};
use crate::pages::decision_page::labels::publication_badge;
use crate::seo::decision::first_sentence;

/// Doublure d'une liste de voisins : la carte réelle de [`SimilarCard`] avec un
/// titre sur deux lignes, un extrait sur trois et deux badges. Partagée avec
/// `DecisionSkeleton`.
#[component]
pub fn SimilarCardsSkeleton(#[prop(default = 4)] count: usize) -> impl IntoView {
    view! {
        <ul class="flex flex-col gap-3" aria-hidden="true">
            {(0..count)
                .map(|_| {
                    view! {
                        <li class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3">
                            <div class="flex flex-col gap-2">
                                <div class="flex flex-col gap-1.5">
                                    <Skeleton class="h-3.5 w-full" />
                                    <Skeleton class="h-3.5 w-2/3" />
                                </div>
                                <div class="flex flex-col gap-1.5">
                                    <Skeleton class="h-3 w-full" />
                                    <Skeleton class="h-3 w-11/12" />
                                    <Skeleton class="h-3 w-3/4" />
                                </div>
                                <div class="flex flex-wrap gap-1.5">
                                    <Skeleton rounded="rounded-full" class="h-5 w-20" />
                                    <Skeleton rounded="rounded-full" class="h-5 w-28" />
                                </div>
                            </div>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

#[component]
pub fn DecisionSimilar(
    principe: Vec<SimilarDecisionHit>,
    espece: Vec<SimilarDecisionHit>,
    #[prop(optional)] abstained: bool,
    #[prop(default = None)] error: Option<String>,
    #[prop(optional)] loading: bool,
) -> impl IntoView {
    let loading_view = loading.then(|| view! { <SimilarCardsSkeleton /> }.into_any());

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

    let empty_view =
        (!loading && error.is_none() && principe.is_empty() && espece.is_empty()).then(|| {
            let msg = if abstained {
                "La décision ne tranche aucune question de droit : aucune voisine n'est proposée."
            } else {
                "Aucun voisin suffisamment proche pour cette décision."
            };
            view! {
                <p class="rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50 p-3 text-sm text-[var(--color-ink-muted)]">
                    {msg}
                </p>
            }
            .into_any()
        });

    view! {
        <aside aria-label="Décisions similaires" class="flex flex-col gap-8">
            {loading_view}
            {error_view}
            {empty_view}
            <SimilarSection title="Décisions de principe similaires" hits=principe fold=Fold::Rule />
            <SimilarSection title="Décisions d'espèce similaires" hits=espece fold=Fold::Head />
        </aside>
    }
}

/// How a card folds its summary.
#[derive(Clone, Copy)]
enum Fold {
    /// To its first sentence.
    Head,
    /// To what follows the first sentence, where a principle's rule sits, the first one setting
    /// out the facts.
    Rule,
}

/// Un volet : titre + cartes. Rien si le volet est vide.
#[component]
fn SimilarSection(title: &'static str, hits: Vec<SimilarDecisionHit>, fold: Fold) -> impl IntoView {
    (!hits.is_empty()).then(|| {
        let cards = hits
            .into_iter()
            .map(|hit| view! { <SimilarCard hit=hit fold=fold /> })
            .collect_view();
        view! {
            <section class="flex flex-col gap-3">
                <h2 class="font-sans text-base text-[var(--color-ink)]">{title}</h2>
                <ul class="flex flex-col gap-3">{cards}</ul>
            </section>
        }
    })
}

#[component]
fn SimilarCard(hit: SimilarDecisionHit, fold: Fold) -> impl IntoView {
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
    let href = lj_dtos::decision_path(
        hit.jurisdiction_type,
        hit.jurisdiction_code.as_deref(),
        &hit.id,
    );

    // A jump to a neighbour keeps the search origin and drops prev/next and the
    // highlight: the query did not find the neighbour.
    let bar = use_decision_bar();
    let on_navigate = move |ev: leptos::ev::MouseEvent| {
        let from_search = bar
            .get_untracked()
            .and_then(|b| b.from_search)
            .map(|f| FromSearch {
                highlight: None,
                ..f
            });
        carry_seed(
            &ev,
            &ResultNavSeed {
                nav: None,
                from_search,
            },
        );
    };

    let summary_view = hit.summary.clone().map(|summary| {
        let first = first_sentence(&summary);
        let rest = summary[first.len()..].trim_start();
        let (lead, excerpt, full) = match fold {
            Fold::Rule if rest.chars().any(char::is_alphanumeric) => {
                (Some(first), rest.to_string(), rest.to_string())
            }
            Fold::Rule => (None, summary.clone(), summary),
            Fold::Head => (None, first, summary),
        };
        view! {
            <ExpandableSummary lead=lead excerpt=excerpt full=full clamp="line-clamp-3" />
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
    // Badges depuis les tags référentiels servis.
    let solution = hit.solution.clone();
    let pub_badge = publication_badge(&hit.publication_codes);

    if solution.is_none() && pub_badge.is_none() {
        return ().into_any();
    }

    let solution_badge = solution.map(|t| {
        view! {
            <Badge tone=BadgeTone::Outline>{t.label}</Badge>
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
            {publication_badge_view}
        </div>
    }
    .into_any()
}
