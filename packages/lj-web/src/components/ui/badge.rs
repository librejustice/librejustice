//! `Badge` (port de `ui/badge.tsx`). CVA -> `badge_classes`.

use leptos::prelude::*;

use crate::helpers::cn;

/// Tonalite. Defaut : `Neutral` (= `defaultVariants.tone`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Ink,
    Accent,
    Hybrid,
    Lexical,
    Outline,
}

const BASE: &str =
    "inline-flex items-center gap-1.5 rounded-sm px-2 py-0.5 text-xs font-medium uppercase tracking-wide";

fn tone_classes(tone: BadgeTone) -> &'static str {
    match tone {
        BadgeTone::Neutral => "bg-[var(--color-vellum)] text-[var(--color-ink-muted)]",
        BadgeTone::Ink => "bg-[var(--color-ink)] text-[var(--color-parchment)]",
        BadgeTone::Accent => "bg-[var(--color-accent-soft)] text-[var(--color-accent)]",
        BadgeTone::Hybrid => "bg-[var(--color-mode-hybrid-soft)] text-[var(--color-mode-hybrid)]",
        BadgeTone::Lexical => {
            "bg-[var(--color-mode-lexical-soft)] text-[var(--color-mode-lexical)]"
        }
        BadgeTone::Outline => {
            "border border-[var(--color-rule)] bg-transparent text-[var(--color-ink-muted)]"
        }
    }
}

/// Classes completes d'un badge (port de `badgeVariants` + `cn(…, className)`).
pub fn badge_classes(tone: BadgeTone, extra: &str) -> String {
    cn([BASE, tone_classes(tone), extra])
}

/// Badge (span).
#[component]
pub fn Badge(
    #[prop(optional, into)] tone: Signal<BadgeTone>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let classes = badge_classes(tone.get_untracked(), &class);
    view! { <span class=classes>{children()}</span> }
}
