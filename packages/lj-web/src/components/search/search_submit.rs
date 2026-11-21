//! `SearchSubmit` : bouton « Rechercher ». Le reclassement se règle sur la page
//! de résultats (`view_prefs::RerankToggle`), pas ici : il n'a de sens qu'en
//! regard de résultats existants.

use leptos::prelude::*;

use crate::helpers::cn;

/// Taille du bouton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitSize {
    Md,
    Lg,
}

#[component]
pub fn SearchSubmit(size: SubmitSize) -> impl IntoView {
    let (height_cls, pad_cls) = if size == SubmitSize::Lg {
        ("h-14", "px-6 text-base")
    } else {
        ("h-11", "px-4 text-sm")
    };

    let class = cn([
        concat!(
            "inline-flex shrink-0 items-center justify-center gap-2 self-start rounded-md border ",
            "border-[var(--color-rule)] font-medium whitespace-nowrap transition-colors ",
            "bg-[var(--color-ink)] text-[var(--color-parchment)] hover:bg-[var(--color-ink-muted)] ",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)] ",
            "focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-background)]"
        ),
        height_cls,
        pad_cls,
    ]);

    view! {
        <button type="submit" class=class>
            {crate::i18n::use_ui().search.search}
        </button>
    }
}
