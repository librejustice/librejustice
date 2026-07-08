//! Bloc « Source » : provenance/audit en pied de décision (source d'origine,
//! ECLI, permalien LibreJustice). Le contenu provient du builder partagé
//! [`lj_dtos::provenance_rows`] — même source de vérité que l'export PDF et
//! l'export DOCX (`pdf_export` / `docx_export`).
use leptos::prelude::*;
use lj_dtos::DecisionDetail;

#[component]
pub fn DecisionProvenance(detail: DecisionDetail) -> impl IntoView {
    let rows = lj_dtos::provenance_rows(&detail);
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
    // Permalien → lien cliquable ; ECLI → mono (identifiant) ; reste → texte.
    let value_view = match label {
        "Permalien" => {
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
        }
        "ECLI" => view! { <span class="break-all font-mono text-sm text-[var(--color-ink)]">{value}</span> }
            .into_any(),
        _ => view! { <span class="text-sm text-[var(--color-ink)]">{value}</span> }.into_any(),
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
