//! After a ruling's text, a line linking to each conclusions page presented in it; after
//! the text of conclusions, the link to their ruling.
use leptos::prelude::*;
use lj_dtos::Commentaire;

use crate::i18n::use_ui;

/// Conclusions that have a page of their own show here.
pub fn annexed(c: &Commentaire) -> bool {
    c.kind == "conclusions" && c.url.as_deref().is_some_and(|url| url.starts_with('/'))
}

#[component]
pub fn DecisionConclusions(
    commentaires: Vec<Commentaire>,
    ruling_url: Option<String>,
) -> impl IntoView {
    let ui = &use_ui().decision;
    let mut links: Vec<(String, String)> = commentaires
        .into_iter()
        .filter(annexed)
        .map(|c| {
            let label = c
                .title
                .unwrap_or_else(|| ui.public_rapporteur_conclusions.to_string());
            (c.url.unwrap_or_default(), label)
        })
        .collect();
    links.extend(ruling_url.map(|url| (url, ui.conclusions_ruling.to_string())));
    if links.is_empty() {
        return ().into_any();
    }
    view! {
        <section
            aria-label=ui.conclusions
            class="flex flex-col gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/30 p-6 print:hidden"
        >
            {links
                .into_iter()
                .map(|(url, label)| {
                    view! {
                        <a
                            href=url
                            class="text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
                        >
                            {label}
                        </a>
                    }
                })
                .collect_view()}
        </section>
    }
    .into_any()
}
