//! `Card` + sous-composants (port de `ui/card.tsx`). Memes classes, meme DOM.

use leptos::prelude::*;

use crate::helpers::cn;

/// Conteneur carte (div).
#[component]
pub fn Card(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn([
        "rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)]",
        &class,
    ]);
    view! { <div class=classes>{children()}</div> }
}

/// En-tete de carte.
#[component]
pub fn CardHeader(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn([
        "flex flex-col gap-1.5 border-b border-[var(--color-rule)] px-6 py-4",
        &class,
    ]);
    view! { <div class=classes>{children()}</div> }
}

/// Titre de carte (`<h3>`).
#[component]
pub fn CardTitle(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn([
        "text-lg leading-tight tracking-tight text-[var(--color-ink)]",
        &class,
    ]);
    view! { <h3 class=classes>{children()}</h3> }
}

/// Description de carte (`<p>`).
#[component]
pub fn CardDescription(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn(["text-sm text-[var(--color-ink-muted)]", &class]);
    view! { <p class=classes>{children()}</p> }
}

/// Corps de carte.
#[component]
pub fn CardContent(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn(["px-6 py-5", &class]);
    view! { <div class=classes>{children()}</div> }
}

/// Pied de carte.
#[component]
pub fn CardFooter(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    let classes = cn([
        "flex items-center justify-end gap-2 border-t border-[var(--color-rule)] px-6 py-3",
        &class,
    ]);
    view! { <div class=classes>{children()}</div> }
}
