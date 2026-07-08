//! `SearchSubmit` (port de `search-submit.tsx`). Split button « Rechercher » +
//! toggle Mode IA (icône sparkles SVG inline — pas d'emoji, qui sort en tofu sans
//! police emoji). En IA, les deux moitiés passent au bordeaux d'accent
//! (rerank LLM + résumés auto, ADR 0041).

use leptos::prelude::*;

use crate::helpers::cn;

/// Taille du bouton (port de la prop `size`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitSize {
    Md,
    Lg,
}

#[component]
pub fn SearchSubmit(
    #[prop(into)] ai_mode: Signal<bool>,
    #[prop(into)] on_toggle: Callback<bool>,
    size: SubmitSize,
    /// Affiche la moitié toggle IA (rerank + résumés = corpus décisions ; la
    /// source textes n'a pas de mode IA).
    #[prop(into, default = Signal::derive(|| true))]
    show_ai: Signal<bool>,
) -> impl IntoView {
    let height_cls = if size == SubmitSize::Lg {
        "h-14"
    } else {
        "h-11"
    };
    let submit_pad_cls = if size == SubmitSize::Lg {
        "px-6 text-base"
    } else {
        "px-4 text-sm"
    };
    let toggle_pad_cls = if size == SubmitSize::Lg {
        "px-4"
    } else {
        "px-3"
    };

    let base_side = concat!(
        "inline-flex items-center justify-center gap-2 font-medium transition-colors ",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)] ",
        "focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-background)] focus-visible:z-10"
    );

    let on_cls = move || {
        if ai_mode.get() {
            "bg-[var(--color-accent)] text-[var(--color-accent-foreground)] hover:opacity-90"
        } else {
            "bg-[var(--color-ink)] text-[var(--color-parchment)] hover:bg-[var(--color-ink-muted)]"
        }
    };
    let toggle_cls = move || {
        if ai_mode.get() {
            "bg-[var(--color-accent)] text-[var(--color-accent-foreground)] hover:opacity-90"
        } else {
            "bg-[var(--color-ink-muted)] text-[var(--color-parchment)] hover:bg-[var(--color-ink)]"
        }
    };

    let wrapper = cn([
        "relative inline-flex shrink-0 items-stretch self-start overflow-hidden rounded-md border border-[var(--color-rule)]",
        height_cls,
    ]);

    // Masquage IA par `display:none` inline et non `<Show>` : la structure DOM
    // reste identique SSR ↔ client (un `<Show>` ici panique l'hydratation des
    // pages SSR qui montent SearchSubmit, ex. la landing). Style inline plutôt
    // que classe `hidden` : les closures `class` réactives écrasent
    // `class:hidden`, et la classe perd la spécificité contre `inline-flex`.
    let ai_display = move || if show_ai.get() { "" } else { "none" };
    let submit_class = move || cn([base_side, submit_pad_cls, on_cls(), "whitespace-nowrap"]);
    let toggle_class = move || cn([base_side, toggle_pad_cls, toggle_cls()]);
    let title = move || {
        if ai_mode.get() {
            "Mode IA activé : résultats reclassés et résumés affichés automatiquement. Cliquer pour désactiver."
        } else {
            "Activer le mode IA : reclasse les résultats par un LLM et pré-charge le résumé de chaque décision."
        }
    };
    let sparkles_class = move || {
        cn([
            // `text-...` : les étoiles décoratives héritent cette couleur (currentColor)
            // pour rester visibles sur le bordeaux quand l'IA est active.
            "pointer-events-none absolute inset-0 text-[var(--color-accent-foreground)] transition-opacity duration-500 ease-out",
            if ai_mode.get() {
                "opacity-100"
            } else {
                "opacity-0"
            },
        ])
    };

    view! {
        <div class=wrapper>
            <button type="submit" class=submit_class>
                "Rechercher"
            </button>
            <div
                aria-hidden="true"
                class="w-px bg-[var(--color-rule)]"
                style:display=ai_display
            ></div>
            <button
                type="button"
                role="switch"
                aria-checked=move || ai_mode.get().to_string()
                aria-label="Mode IA"
                on:click=move |_| on_toggle.run(!ai_mode.get_untracked())
                title=title
                class=toggle_class
                style:display=ai_display
            >
                <Sparkle class="h-4 w-4" />
            </button>
            <span aria-hidden="true" class=sparkles_class style:display=ai_display>
                <Sparkle class="absolute top-1 left-2 h-[11px] w-[11px]" />
                <Sparkle class="absolute bottom-1 left-[32%] h-2.5 w-2.5" />
                <Sparkle class="absolute top-1.5 left-[55%] h-[11px] w-[11px]" />
            </span>
        </div>
    }
}

/// Étoile à quatre branches (sparkle) en SVG inline — icône du Mode IA. Remplace
/// l'emoji `✨` (rendu non garanti, tofu sans police emoji). `fill=currentColor`
/// pour hériter la couleur de texte du contexte (parchment / accent-foreground).
#[component]
fn Sparkle(#[prop(into)] class: String) -> impl IntoView {
    view! {
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="currentColor" class=class>
            <path d="M8 1 Q8.7 7.3 15 8 Q8.7 8.7 8 15 Q7.3 8.7 1 8 Q7.3 7.3 8 1 Z" />
        </svg>
    }
}
