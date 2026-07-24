//! Accordéon « Commentaires » (ADR 0204/0212) : analyses officielles,
//! conclusions du rapporteur public, notes de doctrine — en fin de page
//! décision comme article de norme, **fermé par défaut**. Style uniforme :
//! une analyse (body porté localement) se déplie sur place ; les conclusions
//! du rapporteur public (droits réservés, jamais stockées) sont une
//! ligne-lien externe vers ArianeWeb, composée côté API.
use leptos::prelude::*;
use lj_dtos::Commentaire;

#[component]
pub fn DecisionCommentaires(commentaires: Vec<Commentaire>) -> impl IntoView {
    if commentaires.is_empty() {
        return ().into_any();
    }
    let title = "Commentaires";
    let items = commentaires
        .into_iter()
        .map(|c| match c.url {
            Some(_) => view! { <CommentaireLien commentaire=c /> }.into_any(),
            None => view! { <CommentaireAnalyse commentaire=c /> }.into_any(),
        })
        .collect_view();

    view! {
        <section
            aria-label=title
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/30"
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

/// Analyse officielle : rubriques du plan de classement, sommaire doctrinal,
/// renvois, signature (auteur + date).
#[component]
fn CommentaireAnalyse(commentaire: Commentaire) -> impl IntoView {
    let rubriques = (!commentaire.rubriques.is_empty()).then(|| {
        let lis = commentaire
            .rubriques
            .into_iter()
            .map(|r| view! { <li>{r}</li> })
            .collect_view();
        view! {
            <ul class="flex flex-col gap-1 text-xs text-[var(--color-ink-subtle)]">{lis}</ul>
        }
    });
    let renvois = (!commentaire.renvois.is_empty()).then(|| {
        let lis = commentaire
            .renvois
            .into_iter()
            .map(|r| view! { <li>{r}</li> })
            .collect_view();
        view! {
            <ul class="flex flex-col gap-1 text-xs italic text-[var(--color-ink-subtle)]">
                {lis}
            </ul>
        }
    });
    let signature = match (commentaire.author, commentaire.date) {
        (Some(a), Some(d)) => Some(format!("{a} — {d}")),
        (Some(a), None) => Some(a),
        (None, Some(d)) => Some(d),
        (None, None) => None,
    };
    view! {
        <article class="flex flex-col gap-3 border-t border-[var(--color-rule)] pt-5 first:border-t-0 first:pt-0">
            {rubriques}
            <p class="whitespace-pre-line text-sm leading-relaxed text-[var(--color-ink)]">
                {commentaire.body}
            </p>
            {renvois}
            {signature
                .map(|s| {
                    view! {
                        <p class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                            {s}
                        </p>
                    }
                })}
        </article>
    }
}

/// Ligne-lien externe. Deux formes : conclusions du rapporteur public (droits
/// réservés, lien nu vers ArianeWeb) ou document lié titré (rapport/avis Cass,
/// note de doctrine) avec éditeur et, le cas échéant, mention d'accès abonnés.
#[component]
fn CommentaireLien(commentaire: Commentaire) -> impl IntoView {
    let Commentaire {
        url,
        title,
        publisher,
        access,
        ..
    } = commentaire;
    let url = url.unwrap_or_default();
    // Conclusions CRP : aucune donnée du document n'est stockée → libellé fixe.
    let label = title.unwrap_or_else(|| "Conclusions du rapporteur public".to_string());
    let source = publisher.unwrap_or_else(|| "ArianeWeb".to_string());
    let abonnes = access.as_deref() == Some("abonnes");
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
            {abonnes
                .then(|| {
                    view! {
                        <span class="rounded-sm bg-[var(--color-rule)]/40 px-1.5 py-0.5 text-[0.65rem] uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                            "Abonnés"
                        </span>
                    }
                })}
        </p>
    }
}
