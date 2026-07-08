//! `Skeleton` (port de `ui/skeleton.tsx`). Placeholder anime.

use leptos::prelude::*;

use crate::helpers::cn;

/// Bloc skeleton. `class` regle la taille (h-…/w-…).
#[component]
pub fn Skeleton(#[prop(optional, into)] class: String) -> impl IntoView {
    let classes = cn(["animate-pulse rounded-sm bg-[var(--color-vellum)]", &class]);
    view! { <div class=classes></div> }
}
