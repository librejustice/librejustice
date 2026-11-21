//! The API guide, `/{juris}/api-guide`: the public keyed surface, documented for its integrators.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::components::prose;
use crate::seo::CANONICAL_BASE;

const TITRE_SECTION: &str =
    "text-sm font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]";
const BLOC: &str = "overflow-x-auto rounded-md border border-[var(--color-rule)] \
     bg-[var(--color-vellum)] px-4 py-3 font-mono text-xs text-[var(--color-ink)]";
const LIEN: &str = "text-[var(--color-accent)] underline underline-offset-2 hover:opacity-80";

#[component]
pub fn ApiGuide() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let ui = &crate::i18n::use_ui().api_guide;
    let law_adjective = crate::i18n::use_ui().law_adjective;
    let juris = crate::jurisdiction::use_reader_scope();
    let exemple = (ui.example)(CANONICAL_BASE);

    view! {
        <Title text=ui.title />
        <Meta name="description" content=ui.description />
        <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-14 px-4 py-16 sm:px-6 lg:px-8">
            <section class="flex flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.eyebrow}
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl"
                    style="font-variation-settings: 'wght' 300"
                >
                    {ui.heading}
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        {move || {
                            (ui.heading_law)(law_adjective(&juris.code()))
                        }}
                    </em>
                </h1>
                <p class="text-lg leading-relaxed text-[var(--color-ink-muted)]">
                    {ui.intro}
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>{ui.key_heading}</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.key_steps, LIEN)}
                </p>
                <div class=BLOC>
                    <pre>{exemple}</pre>
                </div>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.key_header, LIEN)}
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>{ui.served_heading}</h2>
                <div class="flex flex-col gap-3">
                    {ui
                        .routes
                        .iter()
                        .map(|&(route, quoi)| {
                            view! {
                                <div class="flex flex-col gap-1 border-l-2 border-[var(--color-rule)] pl-4">
                                    <code class="font-mono text-sm text-[var(--color-ink)]">
                                        {route}
                                    </code>
                                    <p class="text-sm text-[var(--color-ink-muted)]">{quoi}</p>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.beta, LIEN)}
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>{ui.rate_heading}</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.rate, LIEN)}
                </p>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.terms, LIEN)}
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>{ui.assistant_heading}</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.assistant, LIEN)}
                </p>
            </section>
        </div>
    }
}
