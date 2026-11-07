//! Conditions d'utilisation. `<Title>` propre, hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

const LIEN: &str = "text-[var(--color-accent)] underline-offset-4 hover:underline";

#[component]
pub fn Terms() -> impl IntoView {
    crate::pages::cache_public_5_min();
    view! {
        <Title text="Conditions d'utilisation - LibreJustice" />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Conditions d'utilisation"</h1>
            <p class="mt-2 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "En vigueur au 13 octobre 2026"
            </p>

            <Section title="Objet">
                <p>
                    "Ces conditions régissent l'utilisation de "
                    <strong>"librejustice.fr"</strong>
                    ", de son connecteur MCP et de son API (ensemble, « le service »), édités par Iris Sainte Fare Garnot (entrepreneur individuel, SIRET 922 108 600 00029). Elles sont acceptées lors de la création d'un compte et s'appliquent à toute utilisation du service. Elles sont régies par le droit français. Contact : "
                    <a class=LIEN href="mailto:contact@librejustice.fr">
                        "contact@librejustice.fr"
                    </a>
                    "."
                </p>
            </Section>

            <Section title="Accès">
                <p>
                    "Le service est gratuit et ouvert à tout usage, y compris commercial. La recherche sur le site ne demande pas de compte. Le connecteur MCP et l'API en demandent un. L'API s'utilise avec une clé personnelle, créée depuis la "
                    <A href="/profile" attr:class=LIEN>
                        "page profil"
                    </A>
                    ". Le titulaire d'un compte répond de l'usage qui en est fait. Il peut le supprimer à tout moment."
                </p>
            </Section>

            <Section title="Usage">
                <p>
                    "Le service permet de rechercher et de consulter des décisions de justice, des textes, des fiches d'entités et des annonces de marchés publics. Il n'a pas vocation à permettre la reproduction de ces contenus dans leur ensemble. Chaque compte dispose d'un volume de requêtes, commun au site, au connecteur MCP et à l'API, afin que la capacité du service reste partagée entre tous les utilisateurs. Ce volume ne peut être contourné, de quelque manière que ce soit. Pour un besoin plus important, écrivez-nous. Il est interdit de perturber le service, d'accéder à des données ou à des comptes qui ne vous sont pas destinés, ou d'en éprouver la sécurité sans autorisation."
                </p>
                <p class="mt-3">
                    "Quelles que soient les données en cause (décisions, textes, annonces, fiches), il est interdit d'utiliser le service pour :"
                </p>
                <ul class="list-disc space-y-2 pl-5">
                    <li>
                        "identifier une personne que sa source a occultée ou soustraite à la diffusion ;"
                    </li>
                    <li>
                        "dresser des listes de personnes ou d'associations selon l'origine, la nationalité, la religion, l'orientation sexuelle, l'état de santé ou l'appartenance syndicale, réels ou supposés, de ces personnes, des membres de ces associations ou de ceux qu'elles défendent ;"
                    </li>
                    <li>
                        "écarter quelqu'un d'un logement ou d'un emploi en raison des procédures qui le concernent ;"
                    </li>
                    <li>
                        "rassembler de quoi retrouver des personnes, des associations, des syndicats ou des lieux de culte, afin de les exposer à des violences ou à des dégradations ;"
                    </li>
                    <li>
                        "évaluer, analyser, comparer ou prédire les pratiques professionnelles des magistrats et des membres du greffe à partir de leur identité, ce que punissent les articles "
                        <A href="/norm/fr/code-de-l-organisation-judiciaire/l111-13" attr:class=LIEN>
                            "L. 111-13 du code de l'organisation judiciaire"
                        </A>
                        " et "
                        <A href="/norm/fr/code-de-justice-administrative/l10" attr:class=LIEN>
                            "L. 10 du code de justice administrative"
                        </A>
                        "."
                    </li>
                </ul>
                <p class="mt-3">
                    "Un compte utilisé à l'une de ces fins est fermé. Son titulaire en reçoit le motif et peut y répondre."
                </p>
            </Section>

            <Section title="Décisions versées">
                <p>
                    "En versant une décision, vous attestez l'avoir anonymisée et avoir le droit de la partager, et vous acceptez qu'elle soit publiée et réutilisée librement. L'éditeur peut la refuser."
                </p>
            </Section>

            <Section title="Réutilisation">
                <p>
                    "Un service qui s'appuie sur LibreJustice pour fournir des résultats à ses propres utilisateurs, par l'API, le connecteur MCP ou tout autre moyen, indique cette provenance de manière visible là où ces résultats apparaissent. S'il fait payer l'accès à ces résultats, il indique aussi qu'ils sont consultables gratuitement sur librejustice.fr. L'utilisateur est seul responsable de l'usage des données obtenues et de ce qu'il en présente à des tiers. Les contenus restent soumis à la licence de leur source, indiquée sur la page "
                    <A href="/sources" attr:class=LIEN>
                        "Données & sources"
                    </A>
                    "."
                </p>
            </Section>

            <Section title="Responsabilité">
                <p>
                    "Le service est fourni en l'état. L'éditeur ne garantit ni l'exactitude, ni l'exhaustivité, ni la disponibilité des données et du service. Les informations servies ne constituent pas un conseil juridique. Les données personnelles sont traitées dans les conditions de la "
                    <A href="/privacy" attr:class=LIEN>
                        "politique de confidentialité"
                    </A>
                    "."
                </p>
            </Section>

            <Section title="Évolution et suspension">
                <p>
                    "L'éditeur peut modifier, limiter, suspendre ou arrêter tout ou partie du service à tout moment et sans préavis. Il peut de même suspendre ou supprimer, sans préavis ni indemnité, un compte ou une clé dont l'usage méconnaît ces conditions ou perturbe le service. Ces conditions peuvent être modifiées. Celles qui s'appliquent sont celles en ligne au moment de l'utilisation."
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
