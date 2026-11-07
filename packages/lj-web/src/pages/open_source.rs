use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use lj_dtos::{OpenSourceLink, OpenSourceResponse};

use crate::api::ApiClient;
use crate::pages::law_page::data::{sendable, PageError};

const LIEN: &str = "text-[var(--color-accent)] underline-offset-4 hover:underline";

const FONT_SOURCES: &[(&str, &[(&str, &str)])] = &[
    (
        "Pour Liberation Sans Narrow, ",
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
        "Pour la Tahoma de Wine, ",
        &[
            ("tahoma.sfd", "tahoma.sfd"),
            ("tahomabd.sfd", "tahomabd.sfd"),
            ("genttf.ff", "WineTahoma-genttf.ff"),
        ],
    ),
    ("Pour OpenSymbol, ", &[("OpenSymbol.sfd", "OpenSymbol.sfd")]),
];

#[component]
pub fn OpenSource() -> impl IntoView {
    crate::pages::cache_public_5_min();
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
        <Title text="Composants open source - LibreJustice" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Composants open source"</h1>
            <p class="mt-4 text-[var(--color-ink-muted)]">
                "Le "
                <a
                    class=LIEN
                    href="https://github.com/librejustice/librejustice"
                    rel="noreferrer"
                    target="_blank"
                >
                    "code de LibreJustice"
                </a>
                " est publié sous licence Apache 2.0. Il s'appuie sur les composants de tiers ci-dessous, bibliothèques, polices et modèles de lecture des documents, qui gardent chacun leur licence."
            </p>

            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    match list.await {
                        Ok(response) => Either::Left(view! { <Components response=response /> }),
                        Err(err) => {
                            Either::Right(
                                view! {
                                    <p class="mt-10 text-sm text-[var(--color-ink-muted)]">
                                        {err.message}
                                    </p>
                                },
                            )
                        }
                    }
                })}
            </Suspense>

            <section class="mt-10">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">"Mentions"</h2>
                <div class="mt-3 space-y-2 text-sm text-[var(--color-ink-muted)]">
                    <p>
                        "Les deux réseaux PP-OCRv5 sont servis avec leurs poids convertis en float16 et leurs dimensions symboliques renommées. PyLaia RIMES est exporté de PyTorch vers ONNX, avec des poids en int8."
                    </p>
                    <p>
                        "Les listes FrequencyWords, de Hermit Dave, sont construites depuis le corpus OpenSubtitles 2018 d'OPUS. Elles sont servies sans modification."
                    </p>
                    <p>
                        "Les sources des polices sous copyleft sont servies avec elles. "
                        {FONT_SOURCES
                            .iter()
                            .map(|&(font, files)| {
                                let last = files.len() - 1;
                                view! {
                                    {font}
                                    {files
                                        .iter()
                                        .enumerate()
                                        .map(|(i, &(label, file))| {
                                            let sep = match i {
                                                0 => "",
                                                i if i == last => " et ",
                                                _ => ", ",
                                            };
                                            view! {
                                                {sep}
                                                <a
                                                    class=LIEN
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
                        href="/"
                        attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        <span aria-hidden="true">"←"</span>
                        " Retour à l'accueil"
                    </A>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Components(response: OpenSourceResponse) -> impl IntoView {
    view! {
        <table class="mt-10 w-full text-left text-sm">
            <thead class="text-[var(--color-ink)]">
                <tr>
                    <th class="py-2 pr-4 font-medium">"Composant"</th>
                    <th class="py-2 pr-4 font-medium">"Version"</th>
                    <th class="py-2 font-medium">"Licence"</th>
                </tr>
            </thead>
            <tbody class="text-[var(--color-ink-muted)]">
                {response
                    .components
                    .into_iter()
                    .map(|c| {
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
            <a class=LIEN href=href rel="external">
                {link.label}
            </a>
        }),
        None => Either::Right(link.label),
    }
}
