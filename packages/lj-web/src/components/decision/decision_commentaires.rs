//! « Commentaires » accordion: the rapporteur public's conclusions, the court's
//! related documents, doctrinal notes, at the foot of a decision page as of a
//! statute article, **collapsed by default**. A single form, the outgoing link
//! line: nothing is redistributed here, the court's own sommaire having become
//! the summary of the decision.
use leptos::prelude::*;
use lj_dtos::Commentaire;

#[component]
pub fn DecisionCommentaires(commentaires: Vec<Commentaire>) -> impl IntoView {
    let liens: Vec<Commentaire> = commentaires
        .into_iter()
        .filter(|c| c.url.is_some())
        .collect();
    if liens.is_empty() {
        return ().into_any();
    }
    let title = "Commentaires";
    let items = liens
        .into_iter()
        .map(|c| view! { <CommentaireLien commentaire=c /> })
        .collect_view();

    view! {
        <section
            aria-label=title
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/30 print:hidden"
        >
            <details class="group p-6">
                <summary class="flex cursor-pointer list-none items-center justify-between gap-4 font-sans text-base text-[var(--color-ink)] [&::-webkit-details-marker]:hidden">
                    {title}
                    <span
                        aria-hidden="true"
                        class="text-[var(--color-ink-subtle)] transition-transform group-open:rotate-90"
                    >
                        "›"
                    </span>
                </summary>
                <div class="mt-5 flex flex-col gap-6">{items}</div>
            </details>
        </section>
    }
    .into_any()
}

#[component]
fn CommentaireLien(commentaire: Commentaire) -> impl IntoView {
    let reference = commentaire.reference();
    let Commentaire {
        url,
        title,
        publisher,
        ..
    } = commentaire;
    let url = url.unwrap_or_default();
    // Conclusions CRP : aucune donnée du document n'est stockée → libellé fixe.
    let label = title.unwrap_or_else(|| "Conclusions du rapporteur public".to_string());
    let source = publisher.unwrap_or_else(|| "ArianeWeb".to_string());
    view! {
        <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1 border-t border-[var(--color-rule)] pt-5 first:border-t-0 first:pt-0">
            <a
                href=url
                rel="external noopener"
                target="_blank"
                class="text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {format!("{label} — {source}")}
            </a>
            {reference
                .map(|r| {
                    view! {
                        <span class="text-sm text-[var(--color-ink-subtle)]">{r}</span>
                    }
                })}
        </p>
    }
}
