//! Encart « Parties » de la page décision (ADR 0224). Acteurs extraits de la
//! décision (`decision_party`), **résolus au registre uniquement** — même
//! règle que les références de lois : pas de fiche, pas d'affichage. Gabarit
//! à la  : une carte par côté (demandeurs / défendeurs), les conseils
//! du côté rattachés sous ses parties. Rien rendu si aucun acteur résolu.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::DecisionPartyDto;

use crate::helpers::split_entity_uid;

#[component]
pub fn DecisionParties(parties: Vec<DecisionPartyDto>) -> impl IntoView {
    let linked: Vec<DecisionPartyDto> = parties
        .into_iter()
        .filter(|p| p.entity_uid.is_some())
        .collect();
    if linked.is_empty() {
        return ().into_any();
    }

    let is_counsel =
        |p: &DecisionPartyDto| matches!(p.quality.as_str(), "law_firm" | "counsel_name");

    let of_side = |side: &str, counsel: bool| -> Vec<DecisionPartyDto> {
        linked
            .iter()
            .filter(|p| is_counsel(p) == counsel && p.side.as_deref() == Some(side))
            .cloned()
            .collect()
    };
    let sideless = |counsel: bool| -> Vec<DecisionPartyDto> {
        linked
            .iter()
            .filter(|p| is_counsel(p) == counsel && p.side.is_none())
            .cloned()
            .collect()
    };

    view! {
        <section
            aria-label="Parties"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Parties"</h2>
            <div class="mt-4 grid gap-6 sm:grid-cols-2">
                {side_card("Demandeurs", of_side("applicant", false), of_side("applicant", true))}
                {side_card("Défendeurs", of_side("defendant", false), of_side("defendant", true))}
                {side_card("Parties", sideless(false), Vec::new())}
                {side_card("Conseils", sideless(true), Vec::new())}
            </div>
        </section>
    }
    .into_any()
}

/// Carte d'un côté : kicker, parties en liste, conseils du côté en ligne
/// « Conseil(s) : … ». Rien si le côté est vide.
fn side_card(
    title: &'static str,
    parties: Vec<DecisionPartyDto>,
    conseils: Vec<DecisionPartyDto>,
) -> Option<AnyView> {
    if parties.is_empty() && conseils.is_empty() {
        return None;
    }
    let rows = (!parties.is_empty()).then(|| {
        let items = parties
            .into_iter()
            .map(|p| view! { <li>{entity_link(p)}</li> })
            .collect_view();
        view! { <ul class="flex flex-col gap-1.5">{items}</ul> }
    });
    let counsel_line = (!conseils.is_empty()).then(|| {
        let label = if conseils.len() > 1 {
            "Conseils : "
        } else {
            "Conseil : "
        };
        let links = conseils
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let sep = (i > 0)
                    .then(|| view! { <span class="text-[var(--color-ink-subtle)]">" · "</span> });
                view! {
                    {sep}
                    {entity_link(p)}
                }
            })
            .collect_view();
        view! {
            <p class="text-sm">
                <span class="text-[var(--color-ink-subtle)]">{label}</span>
                {links}
            </p>
        }
    });
    Some(
        view! {
            <div class="flex flex-col gap-2">
                <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    {title}
                </h3>
                {rows}
                {counsel_line}
            </div>
        }
        .into_any(),
    )
}

/// Lien vers la fiche `/entite/{ns}/{id}` de l'acteur (toujours résolu ici).
fn entity_link(party: DecisionPartyDto) -> AnyView {
    let uid = party.entity_uid.expect("acteur résolu");
    let (ns, local) = split_entity_uid(&uid);
    let href = format!("/entite/{ns}/{local}");
    view! {
        <A
            href=href
            attr:class="text-sm text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
        >
            {party.value}
        </A>
    }
    .into_any()
}
