//! Bloc « Source » : provenance/audit en pied de fiche, décision comme article.
//! Les lignes viennent d'un builder pur propre à chaque tranche ; le rendu est
//! partagé, et il épèle les URL : une page imprimée se vérifie sans le site.
use leptos::prelude::*;

#[component]
pub fn ProvenanceBlock(rows: Vec<(&'static str, String)>) -> impl IntoView {
    let items = rows
        .into_iter()
        .map(|(label, value)| view! { <ProvenanceField label=label value=value /> })
        .collect_view();

    view! {
        <section
            aria-label="Source"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/30 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Source"</h2>
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">{items}</dl>
        </section>
    }
}

#[component]
fn ProvenanceField(label: &'static str, value: String) -> impl IntoView {
    let value_view = if value.starts_with("http") {
        let href = value.clone();
        view! {
            <a
                href=href
                class="break-all font-mono text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {value}
            </a>
        }
        .into_any()
    } else if label == "ECLI" || label == "Identifiant" {
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
