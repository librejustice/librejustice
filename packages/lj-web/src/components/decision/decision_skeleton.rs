//! Skeleton de chargement de la page décision : même grille `DecisionLayout`
//! que la page chargée, avec les proportions du réel — carte « Synthèse »
//! compacte, corps long, rail de voisins.
use leptos::prelude::*;

use crate::components::ui::Skeleton;

use super::decision_layout::DecisionLayout;

/// Lignes du TOC.
const TOC_LINES: &[&str] = &["w-3/4", "w-2/3", "w-1/2", "w-3/5", "w-1/3"];
/// Résumé de la carte « Synthèse ».
const SUMMARY_LINES: &[&str] = &["w-full", "w-11/12", "w-3/5"];
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
        <nav aria-hidden="true" class="flex flex-col gap-3 lg:sticky lg:top-20 lg:self-start">
            {lines(TOC_LINES)}
        </nav>
    }
    .into_any();

    let main = view! {
        <div aria-hidden="true" class="flex flex-col gap-10">
            // Carte « Synthèse » : mêmes marges/grille que `DecisionMeta`
            // (h2 → résumé → champs → références).
            <section class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6">
                <Skeleton class="h-5 w-28" />
                <div class="mt-3 flex flex-col gap-2">{lines(SUMMARY_LINES)}</div>
                <div class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                    {(0..4)
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
                <div class="mt-5 border-t border-[var(--color-rule)] pt-4">
                    <Skeleton class="h-4 w-44" />
                    <div class="mt-3 flex flex-col gap-2">{lines(&["w-2/3", "w-1/2"])}</div>
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

    let similar = view! {
        <aside aria-hidden="true" class="flex flex-col gap-4 lg:sticky lg:top-20 lg:self-start">
            <Skeleton class="h-5 w-40" />
            <ul class="flex flex-col gap-3">
                {(0..4)
                    .map(|_| {
                        view! {
                            <li class="h-20 animate-pulse rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50" />
                        }
                    })
                    .collect_view()}
            </ul>
        </aside>
    }
    .into_any();

    view! { <DecisionLayout toc=toc main=main similar=similar /> }
}
