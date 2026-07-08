//! `Pill` (port de `ui/pill.tsx`). Bouton bascule `aria-pressed`.

use leptos::prelude::*;

use crate::helpers::cn;

const BASE: &str = "inline-flex h-8 items-center gap-2 rounded-full border px-3 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)]";

const ACTIVE: &str =
    "border-[var(--color-ink)] bg-[var(--color-ink)] text-[var(--color-parchment)]";
const INACTIVE: &str = "border-[var(--color-rule)] bg-[var(--color-parchment)] text-[var(--color-ink)] hover:border-[var(--color-ink)]";

/// Classes completes d'une pill selon l'etat actif (port de `cn(BASE, active ? … : …, className)`).
pub fn pill_classes(active: bool, extra: &str) -> String {
    cn([BASE, if active { ACTIVE } else { INACTIVE }, extra])
}

/// Pill (bouton bascule). `active` pilote `aria-pressed` + le style.
#[component]
pub fn Pill(
    #[prop(optional, into)] active: Signal<bool>,
    #[prop(default = "button", into)] r#type: &'static str,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let active_val = active.get_untracked();
    let classes = pill_classes(active_val, &class);
    view! {
        <button type=r#type aria-pressed=active_val class=classes>
            {children()}
        </button>
    }
}
