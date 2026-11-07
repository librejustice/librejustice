//! Page `/api-guide` : la surface publique à clé, documentée pour qui l'intègre.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::seo::CANONICAL_BASE;

const TITRE_SECTION: &str =
    "text-sm font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]";
const BLOC: &str = "overflow-x-auto rounded-md border border-[var(--color-rule)] \
     bg-[var(--color-vellum)] px-4 py-3 font-mono text-xs text-[var(--color-ink)]";
const LIEN: &str = "text-[var(--color-accent)] underline underline-offset-2 hover:opacity-80";

/// Served in the order a caller chains them: search, then open.
const ROUTES: [(&str, &str); 7] = [
    (
        "GET /api/search",
        "Recherche de décisions, sous les noms de paramètres de l'outil MCP `search_decisions` : `query`, `limit`, `offset`, filtres de juridiction et de date.",
    ),
    (
        "GET /api/search-textes",
        "Recherche d'articles dans le référentiel de normes. `query`, `code` pour borner à un texte, `as_of` pour chercher dans les versions en vigueur ce jour-là.",
    ),
    (
        "GET /api/decision/{id}",
        "Une décision entière, avec son texte et ses métadonnées.",
    ),
    (
        "GET /api/norm/{jurisdiction}/{code}",
        "Un texte sans articles rend son corps ; un texte qui en porte rend son sommaire. La juridiction est son code ISO en minuscules (`fr`, `be`, `eu`) ; le code se nomme par son slug, son nom ou son sigle.",
    ),
    (
        "GET /api/norm/{jurisdiction}/{code}/{article}",
        "Un article dans sa version en vigueur, ou telle qu'elle était au jour passé en `as_of`.",
    ),
    (
        "GET /api/norm/{jurisdiction}/{code}/{path}",
        "Une division du texte et ses articles, nommée par le chemin que porte le sommaire (`livre-i/titre-v`).",
    ),
    (
        "GET /api/article/{LEGIARTI…}",
        "Le même article, adressé par son identifiant Légifrance, qui peut aussi désigner un texte ou une section.",
    ),
];

#[component]
pub fn ApiGuide() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let exemple = format!(
        "curl -H 'Authorization: Bearer ljk_votre_cle' \\\n  \
         '{CANONICAL_BASE}/api/search?query=trouble+anormal+de+voisinage&limit=3'"
    );

    view! {
        <Title text="API publique - LibreJustice" />
        <Meta
            name="description"
            content="L'API REST de LibreJustice : recherche de jurisprudence et de normes, clé gratuite."
        />
        <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-14 px-4 py-16 sm:px-6 lg:px-8">
            <section class="flex flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "API publique · bêta"
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl"
                    style="font-variation-settings: 'wght' 300"
                >
                    "Le droit français, "
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        "dans votre système."
                    </em>
                </h1>
                <p class="text-lg leading-relaxed text-[var(--color-ink-muted)]">
                    "L'API donne accès en REST à la même base que le site et le connecteur MCP. La jurisprudence et les normes sont mises à jour chaque jour depuis les sources officielles et la clé d'accès est gratuite."
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Générer une clé"</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "Créez un compte, puis une clé depuis "
                    <a href="/profile" class=LIEN>
                        "votre profil"
                    </a>
                    ". Le secret n'est affiché qu'une fois. Une clé se révoque à tout moment."
                </p>
                <div class=BLOC>
                    <pre>{exemple}</pre>
                </div>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "L'en-tête "
                    <code class="font-mono text-xs">"x-lj-api-key"</code>
                    " est accepté à la place du "
                    <code class="font-mono text-xs">"Bearer"</code>
                    "."
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Ce qui est servi"</h2>
                <div class="flex flex-col gap-3">
                    {ROUTES
                        .iter()
                        .map(|&(route, quoi)| {
                            view! {
                                <div class="flex flex-col gap-1 border-l-2 border-[var(--color-rule)] pl-4">
                                    <code class="font-mono text-sm text-[var(--color-ink)]">
                                        {route}
                                    </code>
                                    <p class="text-sm text-[var(--color-ink-muted)]">{quoi}</p>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "L'API est en bêta : ses noms de paramètres et de champs peuvent encore changer. Le contrat complet, paramètres et réponses, se lit en OpenAPI : "
                    <a href="/openapi.json" rel="external" class=LIEN>
                        "openapi.json"
                    </a>
                    ". Si votre usage demande autre chose, "
                    <a href="/feedback" class=LIEN>
                        "dites-le nous"
                    </a>
                    "."
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Débit et bonne conduite"</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "Vos appels API, vos appels MCP et vos recherches sur le site s'imputent au même compteur. Un dépassement rend un "
                    <code class="font-mono text-xs">"429"</code>
                    " avec un "
                    <code class="font-mono text-xs">"Retry-After"</code>
                    ". Si votre usage légitime dépasse ce plafond, écrivez-nous plutôt que de contourner : le fonds est public et gratuit, on souhaite que ça le reste, en faisant attention à partager les ressources entre tous les utilisateurs."
                </p>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "L'API est régie par les "
                    <a href="/terms" class=LIEN>
                        "conditions d'utilisation"
                    </a>
                    " du service."
                </p>
            </section>

            <section class="flex flex-col gap-4">
                <h2 class=TITRE_SECTION>"Plutôt un assistant ?"</h2>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "Si le consommateur est une IA et non votre code, le "
                    <a href="/mcp-guide" class=LIEN>
                        "connecteur MCP"
                    </a>
                    " fait le même travail sans que vous écriviez de requête : l'assistant enchaîne les recherches lui-même."
                </p>
            </section>
        </div>
    }
}
