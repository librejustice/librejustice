//! Bloc « Source » : provenance/audit en pied de fiche, décision comme article.
//! Les lignes viennent d'un builder pur propre à chaque tranche ; le rendu est
//! partagé, et il épèle les URL : une page imprimée se vérifie sans le site.
use crate::seo::CANONICAL_BASE;
use leptos::prelude::*;

/// How a value of the block reads, a URL being a link whatever its look.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Text,
    /// An identifier, set in monospace.
    Code,
    /// The source, linked to `source_href` when given.
    Source,
}

#[component]
pub fn ProvenanceBlock(
    rows: Vec<(&'static str, String, Look)>,
    #[prop(default = None)] source_href: Option<String>,
) -> impl IntoView {
    let title = crate::i18n::use_ui().provenance.title;
    let items = rows
        .into_iter()
        .map(|(label, value, look)| {
            let href = (look == Look::Source)
                .then(|| source_href.clone())
                .flatten();
            view! { <ProvenanceField label=label value=value look=look href=href /> }
        })
        .collect_view();

    view! {
        <section
            aria-label=title
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/30 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">{title}</h2>
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">{items}</dl>
        </section>
    }
}

#[component]
fn ProvenanceField(
    label: &'static str,
    value: String,
    look: Look,
    href: Option<String>,
) -> impl IntoView {
    let value_view = if let Some(href) = href {
        view! {
            <a
                href=href
                class="text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {value}
            </a>
        }
        .into_any()
    } else if value.starts_with("http") {
        let href = value.clone();
        let external = !value.starts_with(CANONICAL_BASE);
        view! {
            <a
                href=href
                target=external.then_some("_blank")
                rel=external.then_some("noopener external")
                class="break-all font-mono text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {value}
            </a>
        }
        .into_any()
    } else if look == Look::Code {
        view! { <span class="break-all font-mono text-sm text-[var(--color-ink)]">{value}</span> }
            .into_any()
    } else {
        view! { <span class="text-sm text-[var(--color-ink)]">{value}</span> }.into_any()
    };
    view! {
        <div class="flex flex-col gap-1">
            <dt class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {label}
            </dt>
            <dd>{value_view}</dd>
        </div>
    }
}
