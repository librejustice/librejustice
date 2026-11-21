//! A search that failed, and what to do about it.

use crate::api::client::SearchFailure;
use leptos::prelude::*;

#[component]
pub fn ResultError(failure: SearchFailure) -> impl IntoView {
    let ui = crate::i18n::use_ui();
    let error = ui.shell.error;
    let (title, hint) = failure.words(ui);
    view! {
        <div
            role="alert"
            class="border-t border-[var(--color-accent)] py-12 text-[var(--color-accent)]"
        >
            <p class="text-xs uppercase tracking-[0.2em]">{error}</p>
            <p class="mt-2 font-sans text-xl">{title}</p>
            <p class="mt-1 text-sm text-[var(--color-ink-muted)]">{hint}</p>
        </div>
    }
}
