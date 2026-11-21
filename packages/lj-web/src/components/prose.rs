use leptos::prelude::*;

use crate::i18n::Run;

/// The accent link of running text.
pub const LINK: &str = "text-[var(--color-accent)] underline-offset-4 hover:underline";

/// A sentence of the catalogue, its links drawn with `link`.
pub fn prose(runs: &'static [Run], link: &'static str) -> impl IntoView {
    runs.iter()
        .map(|run| match *run {
            Run::Text(text) => text.into_any(),
            Run::Strong(text) => view! { <strong>{text}</strong> }.into_any(),
            Run::Em(text) => view! { <em>{text}</em> }.into_any(),
            Run::Code(text) => view! { <code class="font-mono text-xs">{text}</code> }.into_any(),
            Run::Link(href, text) => view! {
                <a class=link href=href rel="noreferrer" target="_blank">
                    {text}
                </a>
            }
            .into_any(),
            Run::Local(href, text) => view! {
                <a class=link href=href>
                    {text}
                </a>
            }
            .into_any(),
            Run::File(href, text) => view! {
                <a class=link href=href rel="external">
                    {text}
                </a>
            }
            .into_any(),
            Run::Download(href, text) => view! {
                <a class=link href=href download>
                    {text}
                </a>
            }
            .into_any(),
        })
        .collect_view()
}
