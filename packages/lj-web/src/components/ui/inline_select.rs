//! `InlineSelect` (port de `ui/inline-select.tsx`). `<select>` natif compact
//! controle (value + callback `on_change`).

use leptos::prelude::*;

use super::SelectOption;
use crate::helpers::cn;

/// Select inline controle. `value` = valeur courante ; `on_change` recoit la
/// nouvelle valeur ; `options` la liste `{value,label}`.
#[component]
pub fn InlineSelect(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(into)] options: Signal<Vec<SelectOption>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let select_classes = cn([
        "cursor-pointer appearance-none bg-transparent py-1 pl-2.5 pr-7 text-xs text-[var(--color-ink)] outline-none",
        &class,
    ]);
    view! {
        <div class="relative inline-flex items-center rounded border border-[var(--color-rule)] transition-colors hover:border-[var(--color-ink)]">
            <select
                prop:value=move || value.get()
                on:change=move |ev| on_change.run(event_target_value(&ev))
                class=select_classes
            >
                <For
                    each=move || options.get()
                    key=|opt| opt.value.clone()
                    let:opt
                >
                    <option value=opt.value.clone()>{opt.label.clone()}</option>
                </For>
            </select>
            <svg
                viewBox="0 0 12 8"
                aria-hidden="true"
                class="pointer-events-none absolute right-2 h-2 w-2.5 text-[var(--color-ink-subtle)]"
                fill="none"
            >
                <path
                    d="M1 1.5l5 5 5-5"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
        </div>
    }
}
