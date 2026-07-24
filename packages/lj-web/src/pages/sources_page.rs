//! Page `/sources` — Données & sources (ADR 0114). Décrit chaque source du
//! corpus (décisions + référentiel de droit), avec lien officiel et descriptif.
//! Chaque section porte une ancre (`id`) ciblée par les pages d'article
//! (`/sources#dila`…) et par la homepage. Page statique SSR (SEO), `<Title>`
//! propre, hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;

#[component]
pub fn SourcesPage() -> impl IntoView {
    view! {
        <Title text="Données & sources - LibreJustice" />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Données & sources"</h1>
            <p class="mt-3 max-w-prose text-[var(--color-ink-muted)]">
                "LibreJustice agrège des données juridiques publiques : décisions de "
                "justice et droit positif (codes, lois, traités). Chaque source, avec "
                "son origine et ses conditions de diffusion, est détaillée ci-dessous. "
                "LibreJustice n'en modifie pas le contenu et ne garantit ni "
                "l'exhaustivité ni l'actualité du corpus."
            </p>

            <Source
                id="judilibre"
                titre="Judilibre — Cour de cassation"
                href="https://www.courdecassation.fr/acces-rapide-judilibre"
                lien_label="Cour de cassation · Judilibre"
            >
                "Décisions de l'ordre judiciaire (Cour de cassation, cours d'appel, "
                "tribunaux judiciaires et de commerce), diffusées en open data par la "
                "Cour de cassation via l'API Judilibre, pseudonymisées avant "
                "publication. LibreJustice en ingère l'intégralité du flux et le "
                "resynchronise quotidiennement. L'open data judiciaire s'élargit "
                "progressivement : les conseils de prud'hommes et le pénal de première "
                "instance sont attendus en 2027 ("
                <a
                    class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                    href="https://www.justice.gouv.fr/documentation/open-data-decisions-justice"
                    rel="noreferrer"
                    target="_blank"
                >
                    "calendrier d'ouverture"
                </a>
                ")."
            </Source>

            <Source
                id="opendata-administratif"
                titre="Open data de la justice administrative — Conseil d'État"
                href="https://opendata.justice-administrative.fr/"
                lien_label="opendata.justice-administrative.fr"
            >
                "Décisions de l'ordre administratif (Conseil d'État, cours "
                "administratives d'appel, tribunaux administratifs), diffusées par le "
                "Conseil d'État en archives ZIP XML mises à jour quotidiennement et "
                "pseudonymisées avant publication. LibreJustice les resynchronise "
                "chaque jour. Le flux fournit le texte intégral des décisions, mais ni "
                "les analyses ni les conclusions des rapporteurs publics, qui ne sont "
                "pas diffusées en open data."
            </Source>

            <Source
                id="dila"
                titre="DILA — bases juridiques de l'État"
                href="https://echanges.dila.gouv.fr/OPENDATA/"
                lien_label="Open data DILA (echanges.dila.gouv.fr)"
            >
                "La Direction de l'information légale et administrative diffuse en bulk "
                "les bases ouvertes du droit français. LibreJustice en ingère : "
                "LEGI (codes & lois consolidés, versionnés par article — chaque article "
                "servi en vigueur ou à une date passée ; source des pages /texte et de la "
                "recherche dans les textes), JORF (Journal officiel, dont les traités "
                "publiés par décret), et la jurisprudence JADE (Conseil d'État, avec "
                "analyses) ainsi que les décisions du Conseil constitutionnel (CONSTIT). "
                "Ces fonds sont resynchronisés quotidiennement (incréments DILA)."
            </Source>

            <Source
                id="jafbase"
                titre="JaFBase, droit international privé de la famille"
                href="http://jafbase.fr"
                lien_label="jafbase.fr"
            >
                "Base bénévole tenue depuis 2008 par Cyril Roth, magistrat, l'une des "
                "seules à rendre accessible le droit étranger de la famille. Avec "
                "l'autorisation de l'auteur, LibreJustice y reprend les codes étrangers "
                "(famille, état des personnes, code civil) découpés article par article "
                "et reliés aux décisions qui les citent. JaFBase n'est pas qu'un recueil "
                "de textes : c'est aussi une méthode, une "
                <a
                    class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                    href="http://jafbase.fr/instruments"
                    rel="noreferrer"
                    target="_blank"
                >
                    "carte mentale"
                </a>
                " pour déterminer la règle de conflit applicable. Ces textes ne sont ni "
                "exhaustifs ni garantis à jour."
            </Source>
        </div>
    }
}

/// Section ancrée d'une source : titre (ancre `id` ciblable), descriptif, lien
/// officiel externe.
#[component]
fn Source(
    id: &'static str,
    titre: &'static str,
    href: &'static str,
    lien_label: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <section id=id class="mt-8 scroll-mt-24">
            <h3 class="font-sans text-base text-[var(--color-ink)]">{titre}</h3>
            <p class="mt-2 max-w-prose text-[var(--color-ink-muted)]">{children()}</p>
            <a
                class="group mt-2 inline-flex items-center gap-1.5 text-sm text-[var(--color-accent)]"
                href=href
                rel="noreferrer"
                target="_blank"
            >
                <span class="underline-offset-4 group-hover:underline">{lien_label}</span>
                <span aria-hidden="true">"↗"</span>
            </a>
        </section>
    }
}
