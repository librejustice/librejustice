//! Loading skeleton of the decision page: the loaded page's `DecisionLayout`
//! grid, in its real proportions. Actions above the sommaire, an eight-field
//! Synthèse card, a long body, the references above the column of neighbours.
use leptos::prelude::*;

use crate::components::ui::Skeleton;

use super::decision_layout::DecisionLayout;
use super::decision_similar::SimilarCardsSkeleton;

/// Rows of the references block above the neighbours.
const REFERENCE_LINES: &[&str] = &["w-full", "w-2/3", "w-5/6", "w-1/2"];
/// Lignes du TOC.
const TOC_LINES: &[&str] = &["w-3/4", "w-2/3", "w-1/2", "w-3/5", "w-1/3"];
/// Résumé de la carte « Synthèse » — cinq lignes, la densité d'un résumé réel.
const SUMMARY_LINES: &[&str] = &["w-full", "w-11/12", "w-full", "w-10/12", "w-3/5"];
/// Corps : paragraphes `(intitulé, lignes)` calquant la densité d'un jugement.
const BODY_PARAS: &[(&str, &[&str])] = &[
    ("w-1/4", &["w-full", "w-full", "w-11/12", "w-full", "w-3/4"]),
    ("w-full", &["w-full", "w-5/6", "w-full", "w-2/3"]),
    ("w-1/3", &["w-full", "w-11/12", "w-full", "w-full", "w-1/2"]),
    ("w-full", &["w-full", "w-4/5", "w-full", "w-3/5"]),
    ("w-1/5", &["w-full", "w-full", "w-11/12", "w-5/6", "w-2/5"]),
    ("w-full", &["w-3/4"]),
];

/// Barres de texte `h-4` aux largeurs données (le parent pose le `gap`).
fn lines(widths: &'static [&'static str]) -> impl IntoView {
    widths
        .iter()
        .map(|w| view! { <Skeleton class=format!("h-4 {w}") /> })
        .collect_view()
}

#[component]
pub fn DecisionSkeleton() -> impl IntoView {
    let toc = view! {
        <div aria-hidden="true" class="flex flex-col gap-6 lg:sticky lg:top-20">
            <div class="flex items-center gap-2">
                <Skeleton rounded="rounded-md" class="h-8 w-28" />
                <Skeleton rounded="rounded-md" class="h-8 w-8" />
                <Skeleton rounded="rounded-md" class="h-8 w-8" />
            </div>
            <div class="flex flex-col gap-3">
                <Skeleton class="h-3 w-20" />
                <div class="flex flex-col gap-3 border-l-2 border-[var(--color-rule)] pl-3">
                    {lines(TOC_LINES)}
                </div>
            </div>
        </div>
    }
    .into_any();

    let main = view! {
        <div aria-hidden="true" class="flex flex-col gap-10">
            // Carte « Synthèse » : mêmes marges/grille que `DecisionMeta`
            // (h2 → résumé → champs).
            <section class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6">
                <Skeleton class="h-5 w-28" />
                <div class="mt-3 flex flex-col gap-2">{lines(SUMMARY_LINES)}</div>
                // Huit champs (juridiction, dossier, date, solution, domaine, formation,
                // publication, portée) : la carte réelle en aligne quatre rangées de deux.
                <div class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                    {(0..8)
                        .map(|_| {
                            view! {
                                <div class="flex flex-col gap-1">
                                    <Skeleton class="h-3 w-24" />
                                    <Skeleton class="h-4 w-40" />
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
            </section>
            <div class="flex flex-col gap-7">
                {BODY_PARAS
                    .iter()
                    .map(|&(head, rest)| {
                        view! {
                            <div class="flex flex-col gap-2.5">
                                <Skeleton class=format!("h-5 {head}") />
                                {lines(rest)}
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
    .into_any();

    // Intitulé « Décisions de principe similaires » : `w-56`, sa mesure réelle.
    let similar = view! {
        <aside aria-hidden="true" class="flex flex-col gap-4">
            <Skeleton class="h-5 w-48" />
            {lines(REFERENCE_LINES)}
            <Skeleton class="mt-4 h-5 w-56" />
            <SimilarCardsSkeleton />
        </aside>
    }
    .into_any();

    view! { <DecisionLayout toc=toc main=main similar=similar /> }
}
