//! Mentions légales. Port de `mentions-legales-page.tsx`. `<Title>` propre,
//! hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

#[component]
pub fn MentionsLegales() -> impl IntoView {
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
                    "SIRET : 922 108 600 00011"
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
                    "LibreJustice indexe les décisions de justice françaises rendues publiques par l'État : jurisprudence judiciaire (Cour de cassation, cours d'appel, tribunaux) diffusée via "
                    <strong>"Judilibre"</strong>
                    ", et jurisprudence administrative diffusée par l'"
                    <strong>"Open Data du Conseil d'État"</strong>
                    ". Ces données sont mises à disposition sous "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="https://www.etalab.gouv.fr/licence-ouverte-open-licence"
                        rel="noreferrer"
                        target="_blank"
                    >
                        "Licence Ouverte Etalab 2.0"
                    </a>
                    "."
                </p>
                <p class="mt-3">
                    "LibreJustice ne modifie pas le contenu des décisions et ne garantit ni l'exhaustivité ni l'actualité du corpus indexé. Pour toute référence officielle, consulter directement "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="https://www.legifrance.gouv.fr"
                        rel="noreferrer"
                        target="_blank"
                    >
                        "legifrance.gouv.fr"
                    </a>
                    "."
                </p>
            </Section>

            <Section title="Responsabilité">
                <p>
                    "Les informations présentes sur ce site sont fournies à titre indicatif. Elles ne constituent pas un conseil juridique et ne sauraient engager la responsabilité de l'éditeur. L'éditeur ne peut garantir l'exactitude, l'exhaustivité ou l'actualité des données indexées."
                </p>
            </Section>

            <Section title="Propriété intellectuelle">
                <p>
                    "L'infrastructure logicielle, le design et les composants du site sont la propriété de l'éditeur. Les décisions de justice reproduites sont des documents publics soumis à la Licence Ouverte Etalab 2.0."
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
