//! Layout applicatif. `children()` recoit
//! l'`<Outlet/>` du Router.

use leptos::prelude::*;
use leptos_meta::Meta;

use crate::components::decision_bar::provide_decision_bar_contexts;
use crate::components::language::{LangAlternates, LangOffer};
use crate::components::{Footer, TopBar};

/// Drapeau de suppression de la `<meta description>` generique du site, lu par
/// `AppShell` et pose par les pages qui n'en veulent pas ou qui posent la leur
/// (`own_page_description`).
#[derive(Clone, Copy)]
pub struct SuppressSiteDescription(pub RwSignal<bool>);

/// Declare que la page emet sa propre `<meta name="description">` : la generique
/// du site est retiree le temps ou la page est montee.
///
/// A appeler depuis le composant de route, synchroniquement : le `<head>` est
/// serialise avant la resolution des `<Suspense>`, et une declaration venue d'un
/// corps asynchrone arrive apres coup, laissant DEUX balises `description`.
pub fn own_page_description() {
    let suppress = expect_context::<SuppressSiteDescription>();
    suppress.0.set(true);
    on_cleanup(move || suppress.0.set(false));
}

/// Raised by a page that fills the window itself: the shell is pinned to the
/// viewport and the footer is left out.
#[derive(Clone, Copy)]
pub struct FillViewport(pub RwSignal<bool>);

#[component]
pub fn AppShell(children: Children) -> impl IntoView {
    provide_decision_bar_contexts();
    crate::i18n::provide_reader_lang();
    crate::jurisdiction::provide_reader_scope();

    let fill = RwSignal::new(false);
    provide_context(FillViewport(fill));
    let root_class = move || match fill.get() {
        true => "flex h-svh min-w-0 flex-col overflow-hidden print:block print:min-h-0",
        false => "flex min-h-svh min-w-0 flex-col print:block print:min-h-0",
    };

    // `<Meta name="description">` generique du site. Une page qui pose la sienne
    // appelle `own_page_description()`, qui leve `suppress`.
    let suppress = RwSignal::new(false);
    provide_context(SuppressSiteDescription(suppress));
    let site_description = crate::i18n::use_ui().shell.site_description;
    let scope = crate::jurisdiction::use_reader_scope();
    let default_description = move || {
        (!suppress.get()).then(|| {
            view! { <Meta name="description" content=site_description(&scope.code()) /> }
        })
    };

    view! {
        <div class=root_class>
            <a
                href="#main"
                class="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:px-4 focus:py-2 focus:text-sm focus:text-[var(--color-ink)]"
            >
                {crate::i18n::use_ui().shell.skip_to_content}
            </a>
            <LangOffer />
            <TopBar />
            <main id="main" class="flex min-h-0 min-w-0 flex-1 flex-col">
                {children()}
            </main>
            <Show when=move || !fill.get()>
                <Footer />
            </Show>
        </div>
        // Apres `children()` : une page qui pose `suppress` pendant son rendu synchrone
        // est ainsi vue par cette cloture au moment de la serialisation SSR.
        {default_description}
        <LangAlternates />
    }
}
