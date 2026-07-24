//! Layout 3 colonnes de la page décision. Port de `decision-layout.tsx`.
use leptos::prelude::*;

/// Grille `toc | main | similar`.
///
/// Sur mobile (colonnes empilées), le contenu principal passe AVANT le
/// sommaire/contexte : le lecteur atterrit sur ce qu'il a cliqué, les rails
/// (sommaire, chronologie, versions) suivent. Sur desktop l'ordre visuel
/// redevient `toc | main | similar`.
///
/// `w-full` est nécessaire : la grille est enfant flex du `<main>` (colonne), et
/// `mx-auto` y désactive l'étirement cross-axis (spec flexbox) → largeur
/// fit-content. Le contenu réel (texte long) atteint quand même `max-w`, mais le
/// skeleton (barres en largeurs %) s'effondrait sur ses seules largeurs fixes
/// (~1018px, colonne centrale ~400px) au lieu de remplir les 92rem.
#[component]
pub fn DecisionLayout(toc: AnyView, main: AnyView, similar: AnyView) -> impl IntoView {
    view! {
        <div class="mx-auto grid w-full max-w-[92rem] gap-8 px-4 py-8 sm:px-6 lg:grid-cols-[240px_minmax(0,1fr)_280px] lg:gap-12 lg:px-8 print:block print:max-w-none print:p-0">
            <div class="order-2 lg:order-1 print:hidden">{toc}</div>
            <div class="order-1 min-w-0 lg:order-2">{main}</div>
            <div class="order-3 print:hidden">{similar}</div>
        </div>
    }
}
