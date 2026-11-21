use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use lj_dtos::{OpenSourceLink, OpenSourceResponse};

use crate::api::ApiClient;
use crate::components::{prose, LINK};
use crate::i18n::OpenSourceUi;
use crate::pages::law_page::data::{sendable, PageError};

type FontSources = (
    fn(&OpenSourceUi) -> &'static str,
    &'static [(&'static str, &'static str)],
);

const FONT_SOURCES: &[FontSources] = &[
    (
        |ui| ui.liberation_sources,
        &[
            ("Regular.sfd", "LiberationSansNarrow-Regular.sfd"),
            ("Bold.sfd", "LiberationSansNarrow-Bold.sfd"),
            ("Italic.sfd", "LiberationSansNarrow-Italic.sfd"),
            ("BoldItalic.sfd", "LiberationSansNarrow-BoldItalic.sfd"),
            ("Makefile", "LiberationSansNarrow-Makefile"),
            ("fontexport.pe", "LiberationSansNarrow-fontexport.pe"),
        ],
    ),
    (
        |ui| ui.tahoma_sources,
        &[
            ("tahoma.sfd", "tahoma.sfd"),
            ("tahomabd.sfd", "tahomabd.sfd"),
            ("genttf.ff", "WineTahoma-genttf.ff"),
        ],
    ),
    (
        |ui| ui.opensymbol_sources,
        &[("OpenSymbol.sfd", "OpenSymbol.sfd")],
    ),
];

#[component]
pub fn OpenSource() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let all = crate::i18n::use_ui();
    let ui = &all.open_source;
    let list = Resource::new_blocking(
        || (),
        |_| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_open_source()
                    .await
                    .map_err(PageError::from)
            })
        },
    );
    view! {
        <Title text=ui.title />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.heading}</h1>
            <p class="mt-4 text-[var(--color-ink-muted)]">{prose(ui.intro, LINK)}</p>

            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    match list.await {
                        Ok(response) => Either::Left(view! { <Components response=response /> }),
                        Err(err) => {
                            Either::Right(
                                view! {
                                    <p class="mt-10 text-sm text-[var(--color-ink-muted)]">
                                        {err.failure.text(&crate::i18n::use_ui().shell)}
                                    </p>
                                },
                            )
                        }
                    }
                })}
            </Suspense>

            <section class="mt-10">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">{ui.notes}</h2>
                <div class="mt-3 space-y-2 text-sm text-[var(--color-ink-muted)]">
                    <p>{ui.ocr_weights}</p>
                    <p>{ui.word_lists}</p>
                    <p>
                        {ui.font_sources}
                        {FONT_SOURCES
                            .iter()
                            .map(|&(font, files)| {
                                let font = font(ui);
                                let last = files.len() - 1;
                                view! {
                                    {font}
                                    {files
                                        .iter()
                                        .enumerate()
                                        .map(|(i, &(label, file))| {
                                            let sep = match i {
                                                0 => "",
                                                i if i == last => ui.and,
                                                _ => ", ",
                                            };
                                            view! {
                                                {sep}
                                                <a
                                                    class=LINK
                                                    href=format!("/redactor/fonts/sources/{file}")
                                                    rel="external"
                                                >
                                                    {label}
                                                </a>
                                            }
                                        })
                                        .collect_view()}
                                    ". "
                                }
                            })
                            .collect_view()}
                    </p>
                    <p>"This software is based in part on the work of the Independent JPEG Group."</p>
                </div>
            </section>

            <div class="mt-auto pt-12">
                <div class="border-t border-[var(--color-rule)] pt-6">
                    <A
                        href=crate::jurisdiction::use_reader_scope().href("/")
                        attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        <span aria-hidden="true">"←"</span>
                        " "
                        {all.shell.back_home}
                    </A>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Components(response: OpenSourceResponse) -> impl IntoView {
    let ui = &crate::i18n::use_ui().open_source;
    view! {
        <table class="mt-10 w-full text-left text-sm">
            <thead class="text-[var(--color-ink)]">
                <tr>
                    <th class="py-2 pr-4 font-medium">{ui.component}</th>
                    <th class="py-2 pr-4 font-medium">{ui.version}</th>
                    <th class="py-2 font-medium">{ui.licence}</th>
                </tr>
            </thead>
            <tbody class="text-[var(--color-ink-muted)]">
                {response
                    .components
                    .into_iter()
                    .map(|mut c| {
                        let named = |label: &mut String| {
                            if let Some(named) = (ui.named)(label) {
                                *label = named.to_string();
                            }
                        };
                        named(&mut c.name);
                        c.licence.iter_mut().for_each(|link| named(&mut link.label));
                        view! {
                            <tr class="border-t border-[var(--color-rule)] align-top">
                                <td class="py-1.5 pr-4 text-[var(--color-ink)]">
                                    {match c.home {
                                        Some(home) => {
                                            Either::Left(
                                                view! {
                                                    <a
                                                        class="underline-offset-4 hover:underline"
                                                        href=home
                                                        rel="noreferrer"
                                                        target="_blank"
                                                    >
                                                        {c.name}
                                                    </a>
                                                },
                                            )
                                        }
                                        None => Either::Right(c.name),
                                    }}
                                </td>
                                <td class="whitespace-nowrap py-1.5 pr-4 tabular-nums">
                                    {c.version}
                                </td>
                                <td class="py-1.5">
                                    {c.licence.into_iter().map(text_link).collect_view()}
                                    {c
                                        .files
                                        .into_iter()
                                        .map(|file| view! { " · " {text_link(file)} })
                                        .collect_view()}
                                </td>
                            </tr>
                        }
                    })
                    .collect_view()}
            </tbody>
        </table>
    }
}

fn text_link(link: OpenSourceLink) -> impl IntoView {
    match link.href {
        Some(href) => Either::Left(view! {
            <a class=LINK href=href rel="external">
                {link.label}
            </a>
        }),
        None => Either::Right(link.label),
    }
}
