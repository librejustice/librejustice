//! `ResultEmpty` : aucun résultat. Deux textes, car sans requête ce sont les
//! filtres qui ne trouvent rien, et conseiller de reformuler n'aurait aucun sens.

use leptos::prelude::*;

#[component]
pub fn ResultEmpty(#[prop(into)] query: Signal<String>) -> impl IntoView {
    move || {
        if query.get().is_empty() {
            return view! {
                <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
                    <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                        "Aucun résultat"
                    </p>
                    <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                        "Aucune décision ne répond à ces filtres"
                    </h2>
                    <p class="max-w-prose text-[var(--color-ink-muted)]">
                        "Retirez un filtre, ou élargissez la période."
                    </p>
                </div>
            }
            .into_any();
        }
        view! {
            <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Aucun résultat"
                </p>
                <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                    "Rien trouvé pour «\u{00A0}"{move || query.get()}"\u{00A0}»"
                </h2>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    "Essayez de reformuler avec moins de termes, des synonymes, ou des opérateurs booléens ("
                    <code class="font-mono">"ET"</code>", "<code class="font-mono">"OU"</code>", "
                    <code class="font-mono">"SAUF"</code>"). Le mode Reformulation fonctionne mieux avec des formulations naturelles."
                </p>
            </div>
        }
        .into_any()
    }
}
