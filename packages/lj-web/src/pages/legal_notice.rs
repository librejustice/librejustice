//! Mentions légales. `<Title>` propre, hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

#[component]
pub fn LegalNotice() -> impl IntoView {
    crate::pages::cache_public_5_min();
    view! {
        <Title text="Mentions légales - LibreJustice" />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Mentions légales"</h1>
            <p class="mt-2 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "En vigueur au 15 mai 2026"
            </p>

            <Section title="Éditeur du site">
                <address class="not-italic">
                    <strong>"Iris Sainte Fare Garnot"</strong>
                    ", entrepreneur individuel"
                    <br />
                    "également directeur de la publication"
                    <br />
                    "SIRET : 922 108 600 00029"
                    <br />
                    "Courriel : "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                </address>
            </Section>

            <Section title="Hébergement">
                <p>
                    "Site, moteur de recherche, API et base de données : "
                    <strong>"OVH SAS"</strong>
                    " (Roubaix)."
                </p>
                <p class="mt-3">
                    "Réseau de diffusion (CDN) et protection : "
                    <strong>"Cloudflare Lda."</strong>
                    " (Lisbonne)."
                </p>
            </Section>

            <Section title="Données publiées">
                <p>
                    "Les données diffusées par LibreJustice proviennent de sources publiques et de sources dont la reprise est autorisée. Chacune est présentée, avec ses conditions de réutilisation, sur la page "
                    <A href="/sources" attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline">
                        "Données & sources"
                    </A>
                    "."
                </p>
            </Section>

            <Section title="Propriété intellectuelle">
                <p>
                    "Le "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="https://github.com/librejustice/librejustice"
                        rel="noreferrer"
                        target="_blank"
                    >
                        "code du site"
                    </a>
                    " est publié sous licence Apache 2.0. Les composants de tiers sur lesquels il s'appuie, bibliothèques, polices et outils de lecture des documents, restent sous "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="/open-source"
                    >
                        "leurs propres licences"
                    </a>
                    "."
                </p>
            </Section>

            <div class="mt-auto pt-12">
                <div class="border-t border-[var(--color-rule)] pt-6">
                    <A
                        href="/"
                        attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        <span aria-hidden="true">"←"</span>
                        " Retour à l'accueil"
                    </A>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Section(title: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="mt-10">
            <h2 class="font-sans text-xl text-[var(--color-ink)]">{title}</h2>
            <div class="mt-3 space-y-2 text-[var(--color-ink-muted)] [&_strong]:text-[var(--color-ink)]">
                {children()}
            </div>
        </section>
    }
}
