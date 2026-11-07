//! The « Parties » panel of the decision page. The decision's extracted actors,
//! **resolved in the registry only**, the same rule as law references: no
//! entity page, no display. One group per side (applicants / defendants), each
//! side's counsel under the parties they represent. Nothing rendered when no
//! actor resolves.
//!
//! Une exception, et une seule : la partie qu'un conseil résolu représente est
//! affichée même non résolue, en texte simple. Sans elle l'arête pointe dans le
//! vide, et le conseil retomberait sur son côté, ce que la décision ne dit pas.
//! C'est le cas de toute personne physique, jamais résolue par construction.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::DecisionPartyDto;

use crate::helpers::split_entity_uid;

#[component]
pub fn DecisionParties(
    parties: Vec<DecisionPartyDto>,
    /// Appended to the root: the page renders the block twice, once per layout.
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    let is_counsel =
        |p: &DecisionPartyDto| matches!(p.quality.as_str(), "law_firm" | "counsel_name");
    let counsels: Vec<DecisionPartyDto> = parties
        .iter()
        .filter(|p| is_counsel(p) && p.entity_uid.is_some())
        .cloned()
        .collect();
    if counsels.is_empty() && !parties.iter().any(|p| p.entity_uid.is_some()) {
        return ().into_any();
    }
    // les parties : résolues, plus celles qu'un conseil résolu représente
    let ancres: Vec<i32> = counsels.iter().filter_map(|c| c.party_ord).collect();
    let shown: Vec<DecisionPartyDto> = parties
        .into_iter()
        .filter(|p| {
            if is_counsel(p) {
                return p.entity_uid.is_some();
            }
            p.entity_uid.is_some() || ancres.contains(&p.ord)
        })
        .collect();
    if shown.is_empty() {
        return ().into_any();
    }

    let of_side = |side: Option<&str>, counsel: bool| -> Vec<DecisionPartyDto> {
        shown
            .iter()
            .filter(|p| is_counsel(p) == counsel && p.side.as_deref() == side)
            .cloned()
            .collect()
    };

    view! {
        <section aria-label="Parties" class=format!("flex flex-col gap-3 print:hidden {class}")>
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Parties"
            </h2>
            <div class="flex flex-col gap-4">
                {side_card(
                    "Demandeurs",
                    of_side(Some("applicant"), false),
                    of_side(Some("applicant"), true),
                )}
                {side_card(
                    "Défendeurs",
                    of_side(Some("defendant"), false),
                    of_side(Some("defendant"), true),
                )}
                {side_card("Parties", of_side(None, false), Vec::new())}
                {side_card("Conseils", Vec::new(), of_side(None, true))}
            </div>
        </section>
    }
    .into_any()
}

/// Carte d'un côté : kicker, parties en liste avec sous elles les conseils que
/// leur arête nomme, puis les conseils du côté qu'aucune arête ne rattache.
/// Rien si le côté est vide.
fn side_card(
    title: &'static str,
    parties: Vec<DecisionPartyDto>,
    conseils: Vec<DecisionPartyDto>,
) -> Option<AnyView> {
    if parties.is_empty() && conseils.is_empty() {
        return None;
    }
    let rattaches: Vec<i32> = parties.iter().map(|p| p.ord).collect();
    let orphelins: Vec<DecisionPartyDto> = conseils
        .iter()
        .filter(|c| !c.party_ord.is_some_and(|o| rattaches.contains(&o)))
        .cloned()
        .collect();
    let rows = (!parties.is_empty()).then(|| {
        let items = parties
            .into_iter()
            .map(|p| {
                let sous: Vec<DecisionPartyDto> = conseils
                    .iter()
                    .filter(|c| c.party_ord == Some(p.ord))
                    .cloned()
                    .collect();
                view! {
                    <li>
                        {party_label(p)}
                        {counsel_line(sous)}
                    </li>
                }
            })
            .collect_view();
        view! { <ul class="flex flex-col gap-1.5 text-sm">{items}</ul> }
    });
    let counsel_line = counsel_line(orphelins);
    Some(
        view! {
            <div class="flex flex-col gap-1.5">
                <h3 class="text-xs font-medium text-[var(--color-ink-muted)]">{title}</h3>
                {rows}
                {counsel_line}
            </div>
        }
        .into_any(),
    )
}

/// Ligne « Conseil(s) : … » d'un ensemble de conseils, ou rien.
fn counsel_line(conseils: Vec<DecisionPartyDto>) -> Option<AnyView> {
    if conseils.is_empty() {
        return None;
    }
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
                {party_label(p)}
            }
        })
        .collect_view();
    Some(
        view! {
            <p class="text-xs">
                <span class="text-[var(--color-ink-subtle)]">{label}</span>
                {links}
            </p>
        }
        .into_any(),
    )
}

/// Le verbatim de l'acteur : lien vers sa fiche s'il est résolu, texte simple
/// sinon (une personne physique ne l'est jamais).
fn party_label(party: DecisionPartyDto) -> AnyView {
    let Some(uid) = party.entity_uid else {
        return view! { <span class="text-[var(--color-ink)]">{party.value}</span> }.into_any();
    };
    let (ns, local) = split_entity_uid(&uid);
    let href = lj_vocab::entity_path(ns, local);
    view! {
        <A
            href=href
            attr:class="text-[var(--color-ink)] underline decoration-[var(--color-rule)] underline-offset-4 transition-colors hover:text-[var(--color-accent)] hover:decoration-current"
        >
            {party.value}
        </A>
    }
    .into_any()
}
