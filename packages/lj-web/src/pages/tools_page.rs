//! Page `/tools`: the hub of what the site offers beyond its search bar.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::i18n::ToolCopy;

/// A card is a component, not inline markup: the children of a repeated
/// fragment must stay uniform, or hydration fails in release.
#[component]
fn ToolCard(state: &'static str, copy: &'static ToolCopy, href: &'static str) -> impl IntoView {
    view! {
        <a
            href=href
            class="group flex flex-col gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-5 transition-colors hover:border-[var(--color-accent)]"
        >
            <span class="inline-flex w-fit items-center rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-0.5 text-xs text-[var(--color-ink-subtle)]">
                {state}
            </span>
            <h2 class="font-sans text-xl text-[var(--color-ink)]">{copy.title}</h2>
            <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">{copy.body}</p>
        </a>
    }
}

#[component]
pub fn ToolsPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let ui = &crate::i18n::use_ui().tools;

    view! {
        <Title text=ui.title />
        <Meta name="description" content=ui.description />
        <div class="mx-auto flex w-full min-w-0 max-w-5xl flex-col gap-14 px-4 py-16 sm:px-6 lg:px-8">
            <section class="flex max-w-3xl flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.eyebrow}
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl"
                    style="font-variation-settings: 'wght' 300"
                >
                    {ui.heading_lead}
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        {ui.heading_em}
                    </em>
                </h1>
            </section>

            <section class="grid gap-4 sm:grid-cols-2">
                {[
                    (ui.available, &ui.mcp, "/mcp-guide"),
                    (ui.available, &ui.api, "/api-guide"),
                    (ui.beta, &ui.redact, "/redact"),
                    (ui.beta, &ui.contribute, "/decision/contribute"),
                ]
                    .into_iter()
                    .map(|(state, copy, href)| view! { <ToolCard state copy href /> })
                    .collect_view()}
            </section>
        </div>
    }
}
