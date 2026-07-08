//! Layout applicatif. Port de `components/layout/app-shell.tsx` (avec
//! `DecisionBarProvider`). `children()` recoit l'`<Outlet/>` du Router.

use leptos::prelude::*;
use leptos_meta::Meta;
use leptos_router::hooks::use_location;

use crate::components::decision_bar::provide_decision_bar_contexts;
use crate::components::{Footer, TopBar};
use crate::seo::site_default;

/// Drapeau de suppression de la `<meta description>` generique du site, lu par
/// `AppShell` et pose par les pages qui n'en veulent pas (404 `NotFound`, en
/// `robots noindex` sans description — parite avec l'export `meta` RR qui
/// remplace la racine). La page decision passe deja par le filtre de path
/// `/decision/*` (elle emet sa propre description ou aucune), donc n'a pas
/// besoin de ce drapeau.
#[derive(Clone, Copy)]
pub struct SuppressSiteDescription(pub RwSignal<bool>);

#[component]
pub fn AppShell(children: Children) -> impl IntoView {
    // Contextes barre decision : ancetre de la TopBar (consommateur) et des pages
    // routees (DecisionHeader / cartes resultats / voisines, producteurs).
    provide_decision_bar_contexts();

    // `<Meta name="description">` generique du site. Pose ici (dans le Router)
    // plutot qu'au niveau `App` : sur `/decision/*` la page emet sa propre
    // description (et la branche erreur n'en emet aucune, comme le `meta`
    // noindex de RR), donc on supprime la generique pour ne jamais avoir deux
    // balises `description`. Reactif : a la navigation client la balise se
    // monte/demonte selon la route, comme le remplacement de meta de RR.
    let location = use_location();
    let suppress = RwSignal::new(false);
    provide_context(SuppressSiteDescription(suppress));
    let default_description = move || {
        let on_content_route = !location.pathname.get().starts_with("/decision") && !suppress.get();
        on_content_route.then(|| {
            view! { <Meta name="description" content=site_default().description /> }
        })
    };

    view! {
        <div class="flex min-h-svh min-w-0 flex-col print:block print:min-h-0">
            <a
                href="#main"
                class="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:px-4 focus:py-2 focus:text-sm focus:text-[var(--color-ink)]"
            >
                "Aller au contenu principal"
            </a>
            <TopBar />
            <main id="main" class="flex min-w-0 flex-1 flex-col">
                {children()}
            </main>
            <Footer />
        </div>
        // Apres `children()` : une page (ex. NotFound) qui pose `suppress` pendant
        // son rendu synchrone est ainsi vue par cette cloture au moment de la
        // serialisation SSR (ordre d'arbre). La balise sort toujours dans le
        // <head> (leptos_meta), la position ici ne sert qu'a l'ordre de lecture.
        {default_description}
    }
}
