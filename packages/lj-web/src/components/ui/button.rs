//! `Button` (port de `ui/button.tsx`). CVA -> `button_classes`.

use leptos::prelude::*;

use crate::helpers::cn;

/// Variante visuelle. Defaut : `Primary` (= `defaultVariants.variant`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Accent,
    Outline,
    Ghost,
    Link,
}

/// Taille. Defaut : `Md` (= `defaultVariants.size`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonSize {
    Sm,
    #[default]
    Md,
    Lg,
    Icon,
}

const BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-background)] disabled:pointer-events-none disabled:opacity-50";

fn variant_classes(variant: ButtonVariant) -> &'static str {
    match variant {
        ButtonVariant::Primary => {
            "bg-[var(--color-ink)] text-[var(--color-parchment)] hover:bg-[var(--color-ink-muted)]"
        }
        ButtonVariant::Accent => {
            "bg-[var(--color-accent)] text-[var(--color-accent-foreground)] hover:opacity-90"
        }
        ButtonVariant::Outline => {
            "border border-[var(--color-rule)] bg-transparent text-[var(--color-ink)] hover:bg-[var(--color-vellum)]"
        }
        ButtonVariant::Ghost => {
            "bg-transparent text-[var(--color-ink)] hover:bg-[var(--color-vellum)]"
        }
        ButtonVariant::Link => {
            "h-auto bg-transparent px-0 py-0 text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
        }
    }
}

fn size_classes(size: ButtonSize) -> &'static str {
    match size {
        ButtonSize::Sm => "h-8 rounded-sm px-3 text-sm",
        ButtonSize::Md => "h-10 rounded-md px-4 text-sm",
        ButtonSize::Lg => "h-12 rounded-md px-5 text-base",
        ButtonSize::Icon => "h-9 w-9 rounded-sm p-0",
    }
}

/// Classes complietes d'un bouton (port de `buttonVariants` + `cn(…, className)`).
pub fn button_classes(variant: ButtonVariant, size: ButtonSize, extra: &str) -> String {
    cn([BASE, variant_classes(variant), size_classes(size), extra])
}

/// Bouton. `class` = classes additionnelles (fusionnees apres les variantes).
#[component]
pub fn Button(
    #[prop(optional, into)] variant: Signal<ButtonVariant>,
    #[prop(optional, into)] size: Signal<ButtonSize>,
    #[prop(default = "button", into)] r#type: &'static str,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let classes = button_classes(variant.get_untracked(), size.get_untracked(), &class);
    view! {
        <button type=r#type class=classes>
            {children()}
        </button>
    }
}
