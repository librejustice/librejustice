//! Page `/redact`: the workbench, which takes the whole window.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

#[component]
pub fn RedactPage() -> impl IntoView {
    crate::pages::cache_public_5_min();

    let fill = expect_context::<crate::components::FillViewport>();
    fill.0.set(true);
    on_cleanup(move || fill.0.set(false));

    view! {
        <Title text="Anonymisation - LibreJustice" />
        <Meta
            name="description"
            content="Anonymiser un PDF, un document Word ou une image sans que le fichier quitte votre navigateur."
        />
        <div class="flex min-h-0 w-full min-w-0 flex-1 flex-col p-2 sm:px-6 sm:py-4 lg:px-8">
            <h1 class="sr-only">"Anonymiser un document dans votre navigateur"</h1>
            <crate::pages::redact::SiteWorkbench />
        </div>
    }
}
