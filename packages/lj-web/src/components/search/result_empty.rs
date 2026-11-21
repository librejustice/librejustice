//! `ResultEmpty` : aucun résultat. Deux textes, car sans requête ce sont les
//! filtres qui ne trouvent rien, et conseiller de reformuler n'aurait aucun sens.

use leptos::prelude::*;

#[component]
pub fn ResultEmpty(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    move || {
        if query.get().is_empty() {
            return view! {
                <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
                    <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                        {ui.no_results}
                    </p>
                    <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                        {ui.no_decision_for_filters}
                    </h2>
                    <p class="max-w-prose text-[var(--color-ink-muted)]">
                        {ui.remove_a_filter}
                    </p>
                </div>
            }
            .into_any();
        }
        view! {
            <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.no_results}
                </p>
                <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                    {move || (ui.nothing_found_for)(&query.get())}
                </h2>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {ui.rephrase_before}
                    <code class="font-mono">{ui.op_and}</code>", "<code class="font-mono">{ui.op_or}</code>", "
                    <code class="font-mono">{ui.op_not}</code>{ui.rephrase_after}
                </p>
            </div>
        }
        .into_any()
    }
}
