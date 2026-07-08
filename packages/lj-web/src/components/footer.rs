//! Pied de page. Port verbatim de `components/layout/footer.tsx`.

use leptos::prelude::*;

#[component]
pub fn Footer() -> impl IntoView {
    view! {
        <footer class="border-t border-[var(--color-rule)] bg-[var(--color-vellum)]">
            <div class="mx-auto flex max-w-7xl flex-col gap-3 px-4 py-8 text-sm text-[var(--color-ink-muted)] sm:flex-row sm:items-center sm:justify-between sm:px-6 lg:px-8">
                <p class="text-base">
                    <span
                        class="font-sans tracking-[0.02em] text-[var(--color-ink-muted)]"
                        style="font-variation-settings: 'wght' 300"
                    >
                        "Libre"
                    </span>
                    <span
                        class="font-sans tracking-[-0.02em] text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 650"
                    >
                        "Justice"
                    </span>
                </p>
                <nav class="flex flex-wrap gap-x-5 gap-y-2">
                    <a class="hover:text-[var(--color-ink)]" href="/sources">
                        "Données & sources"
                    </a>
                    <a class="hover:text-[var(--color-ink)]" href="/mentions-legales">
                        "Mentions légales"
                    </a>
                    <a class="hover:text-[var(--color-ink)]" href="/confidentialite">
                        "Confidentialité"
                    </a>
                </nav>
            </div>
        </footer>
    }
}
