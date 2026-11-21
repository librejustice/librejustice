//! The legal pages: terms of use, privacy policy, legal notice. Each sets its own `<Title>` and
//! inherits the root description.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

use crate::components::{prose, LINK};
use crate::i18n::{Block, LegalPage};

#[component]
pub fn Terms() -> impl IntoView {
    view! { <Legal page=&crate::i18n::use_ui().legal.terms /> }
}

#[component]
pub fn Privacy() -> impl IntoView {
    view! { <Legal page=&crate::i18n::use_ui().legal.privacy /> }
}

#[component]
pub fn LegalNotice() -> impl IntoView {
    view! { <Legal page=&crate::i18n::use_ui().legal.notice /> }
}

#[component]
fn Legal(page: &'static LegalPage) -> impl IntoView {
    crate::pages::cache_public_5_min();
    let back_home = crate::i18n::use_ui().shell.back_home;
    view! {
        <Title text=page.title />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">{page.heading}</h1>
            <p class="mt-2 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {page.in_force}
            </p>
            {page
                .sections
                .iter()
                .map(|section| {
                    view! {
                        <section class="mt-10">
                            <h2 class="font-sans text-xl text-[var(--color-ink)]">{section.title}</h2>
                            <div class="mt-3 space-y-2 text-[var(--color-ink-muted)] [&_strong]:text-[var(--color-ink)]">
                                {section.blocks.iter().enumerate().map(|(i, b)| block(i, b)).collect_view()}
                            </div>
                        </section>
                    }
                })
                .collect_view()}
            <div class="mt-auto pt-12">
                <div class="border-t border-[var(--color-rule)] pt-6">
                    <A
                        href=crate::jurisdiction::use_reader_scope().href("/")
                        attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        <span aria-hidden="true">"←"</span>
                        " "
                        {back_home}
                    </A>
                </div>
            </div>
        </div>
    }
}

/// A block of a section; a paragraph after the first block stands apart.
fn block(index: usize, block: &'static Block) -> AnyView {
    match block {
        Block::Para(runs) if index == 0 => view! { <p>{prose(runs, LINK)}</p> }.into_any(),
        Block::Para(runs) => view! { <p class="mt-3">{prose(runs, LINK)}</p> }.into_any(),
        Block::List(items) => view! {
            <ul class="list-disc space-y-2 pl-5">
                {items.iter().map(|runs| view! { <li>{prose(runs, LINK)}</li> }).collect_view()}
            </ul>
        }
        .into_any(),
        Block::Address(lines) => view! {
            <address class="not-italic">
                {lines
                    .iter()
                    .enumerate()
                    .map(|(i, runs)| view! { {(i > 0).then(|| view! { <br /> })} {prose(runs, LINK)} })
                    .collect_view()}
            </address>
        }
        .into_any(),
    }
}
