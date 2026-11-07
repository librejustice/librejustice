//! Layout 3 colonnes de la page décision (et de la page texte, `law_page`).
use leptos::prelude::*;

use crate::components::layout::{GRID_3COL, SHELL};

/// Grille `toc | main | similar`. La stickiness est l'affaire de chaque
/// colonne : le sommaire est sticky, la colonne similaires défile avec la
/// page.
///
/// On mobile (stacked columns) the desktop rails go: the page reinjects what the
/// left one carries into the main flow, and the similar decisions follow the
/// content, so the mobile reading order is linear.
///
/// La colonne centrale est bornée à `max-w-3xl` en grille 3 colonnes — la même
/// mesure de lecture que `CONTENT_COL` des pages de recherche. Sous `lg`
/// (colonne unique) et à l'impression, pleine largeur.
///
/// `w-full` est nécessaire : la grille est enfant flex du `<main>` (colonne), et
/// `mx-auto` y désactive l'étirement cross-axis (spec flexbox) → largeur
/// fit-content.
#[component]
pub fn DecisionLayout(toc: AnyView, main: AnyView, similar: AnyView) -> impl IntoView {
    view! {
        <div class=format!("{SHELL} {GRID_3COL} print:block print:max-w-none print:p-0")>
            <div class="hidden lg:block print:hidden">{toc}</div>
            <div class="w-full min-w-0 lg:max-w-3xl print:max-w-none">{main}</div>
            <div class="print:hidden">{similar}</div>
        </div>
    }
}
