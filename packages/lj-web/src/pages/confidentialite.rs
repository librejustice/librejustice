//! Politique de confidentialité. Port de `privacy-page.tsx`. Herite du
//! title/description racine (pas de `<Title>` propre).

use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Confidentialite() -> impl IntoView {
    view! {
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                "Politique de confidentialité"
            </h1>
            <p class="mt-2 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "En vigueur au 15 mai 2026"
            </p>

            <Section title="Responsable de traitement">
                <p>
                    "Iris Sainte Fare Garnot (entrepreneur individuel, SIRET 922 108 600 00011), éditeur de "
                    <strong>"librejustice.fr"</strong>
                    ". Contact : "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                    "."
                </p>
            </Section>

            <Section title="Données collectées">
                <p>
                    <strong>"Compte utilisateur."</strong>
                    " Adresse email, identifiant interne, nom affiché si renseigné, et identifiant du fournisseur OAuth si vous vous connectez via Google. Effacé à la suppression du compte."
                </p>
                <p class="mt-3">
                    <strong>"Signets et historique de recherche."</strong>
                    " Identifiants des décisions sauvegardées, termes saisis, filtres appliqués. Attachés à votre compte, lus uniquement pour vous afficher vos propres listes. Supprimables à tout moment, entrée par entrée ou en bloc."
                </p>
                <p class="mt-3">
                    <strong>"Traces applicatives."</strong>
                    " Latences, erreurs et chemins d'URL côté API, sans adresse IP. Conservation : 14 jours."
                </p>
            </Section>

            <Section title="Cookies et stockage local">
                <p>
                    "Aucun cookie publicitaire, aucun traceur soumis à consentement. Seulement :"
                </p>
                <ul class="mt-3 list-disc space-y-2 pl-5">
                    <li>
                        "Un jeton de session stocké dans votre navigateur si vous êtes connecté, effacé à la déconnexion."
                    </li>
                    <li>
                        "Une mesure d'audience anonyme côté Cloudflare, sans cookie ni identifiant persistant."
                    </li>
                </ul>
            </Section>

            <Section title="Sous-traitants">
                <ul class="list-disc space-y-2 pl-5">
                    <li>
                        <strong>"OVH"</strong>
                        " (Roubaix) : hébergement, base de données, télémétrie."
                    </li>
                    <li>
                        <strong>"Cloudflare Lda."</strong>
                        " (Lisbonne) : CDN, protection, mesure d'audience."
                    </li>
                    <li>
                        <strong>"Supabase"</strong>
                        " (San Francisco, données à Dublin) : authentification."
                    </li>
                    <li>
                        <strong>"Grafana Cloud"</strong>
                        " (New York, données à Francfort) : logs et traces."
                    </li>
                </ul>
            </Section>

            <Section title="Vos droits">
                <p>
                    "Vous disposez d'un droit d'accès, de rectification, d'effacement, de limitation, d'opposition et de portabilité (RGPD, loi Informatique et Libertés)."
                </p>
                <p class="mt-3">
                    "Le bouton « Supprimer mon compte » sur votre "
                    <A
                        href="/profil"
                        attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                    >
                        "page profil"
                    </A>
                    " efface immédiatement votre compte, vos signets et votre historique."
                </p>
                <p class="mt-3">
                    "Pour toute autre demande, écrivez à "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                    " ou saisissez la "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="https://www.cnil.fr/fr/plaintes"
                        rel="noreferrer"
                        target="_blank"
                    >
                        "CNIL"
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
            <div class="mt-3 space-y-2 text-[var(--color-ink-muted)] [&_strong]:text-[var(--color-ink)] [&_code]:rounded [&_code]:bg-[var(--color-parchment)] [&_code]:px-1 [&_code]:py-0.5 [&_code]:text-xs">
                {children()}
            </div>
        </section>
    }
}
