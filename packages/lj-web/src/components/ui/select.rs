//! `Select` (port de `ui/select.tsx`). `<select>` natif stylise + chevron SVG.

use leptos::html;
use leptos::prelude::*;

use crate::helpers::cn;

/// Select natif. `children` = les `<option>`. `select_ref` expose le `<select>`
/// (branchement `on:change`/`prop:value` cote appelant).
#[component]
pub fn Select(
    #[prop(optional, into)] class: String,
    #[prop(optional)] select_ref: NodeRef<html::Select>,
    children: Children,
) -> impl IntoView {
    let select_classes = cn([
        "h-full w-full appearance-none bg-[var(--color-parchment)] pl-3 pr-9 text-sm text-[var(--color-ink)] outline-none",
        "disabled:cursor-not-allowed disabled:opacity-60",
        &class,
    ]);
    view! {
        <div class="relative inline-flex h-10 w-full items-center rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] has-[:focus-visible]:border-[var(--color-ink)]">
            <select node_ref=select_ref class=select_classes>
                {children()}
            </select>
            <svg
                aria-hidden="true"
                viewBox="0 0 12 8"
                class="pointer-events-none absolute right-3 h-2 w-3 text-[var(--color-ink-subtle)]"
            >
                <path
                    d="M1 1.5l5 5 5-5"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
        </div>
    }
}
