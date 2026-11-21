//! The « Citée par » block: the decisions whose text cites the page's, or whose
//! Conseil d'Etat analysis refers to it, authority first. Not the chronology of
//! the rail, the steps of the same case.

use leptos::prelude::*;
use lj_dtos::CitingDecisionHit;

use crate::pages::law_page::cited_by::CitedDecisionRow;

#[component]
pub fn DecisionCitantes(citantes: Vec<CitingDecisionHit>, total: Option<i64>) -> impl IntoView {
    if citantes.is_empty() {
        return ().into_any();
    }
    let ui = &crate::i18n::use_ui().decision;
    let title = ui.cited_by;
    let reste = total
        .map(|t| t - citantes.len() as i64)
        .filter(|r| *r > 0)
        .map(ui.cited_by_more);
    let lignes = citantes
        .into_iter()
        .map(|hit| view! { <CitedDecisionRow hit=hit cited=None /> })
        .collect_view();

    view! {
        <section
            aria-label=title
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6 print:hidden"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">{title}</h2>
            <ul class="mt-4 flex flex-col">{lignes}</ul>
            {reste
                .map(|r| {
                    view! { <p class="mt-3 text-xs text-[var(--color-ink-subtle)]">{r}</p> }
                })}
        </section>
    }
    .into_any()
}
