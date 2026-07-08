//! `ResultError` (port de `result-error.tsx`). Erreur de recherche (API down).

use leptos::prelude::*;

#[component]
pub fn ResultError(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div
            role="alert"
            class="border-t border-[var(--color-accent)] py-12 text-[var(--color-accent)]"
        >
            <p class="text-xs uppercase tracking-[0.2em]">"Erreur"</p>
            <p class="mt-2 font-sans text-xl">{message}</p>
            <p class="mt-1 text-sm text-[var(--color-ink-muted)]">
                "L'API est peut-être indisponible. Réessayez dans un instant."
            </p>
        </div>
    }
}
