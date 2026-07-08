//! Guide MCP. Port de `mcp-page.tsx`. Herite du title/description racine.

use leptos::prelude::*;

use crate::seo::CANONICAL_BASE;

/// Prompts d'exemple a copier dans un client IA (verbatim `TEST_PROMPTS`).
const TEST_PROMPTS: [&str; 4] = [
    "Trouve 5 décisions sur l'annulation d'un permis de construire.",
    "Cherche en mode lexical : OQTF SAUF rétention.",
    "Trouve une décision sur le harcèlement moral dans la fonction publique puis donne-moi son texte intégral.",
    "Quelles sont les conditions posées par le Conseil d'État pour engager la responsabilité de l'État sans faute ?",
];

#[component]
pub fn McpGuide() -> impl IntoView {
    let mcp_url = format!("{CANONICAL_BASE}/mcp/");
    let prompts = TEST_PROMPTS.iter().map(|prompt| {
        view! {
            <li class="border-l-2 border-[var(--color-accent)] py-1 pl-4 text-sm text-[var(--color-ink-muted)]">
                {*prompt}
            </li>
        }
    });

    view! {
        <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-16 px-4 py-16 sm:px-6 lg:px-8">
            // Hero
            <section class="flex flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Remote MCP"
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl lg:text-[3.5rem]"
                    style="font-variation-settings: 'wght' 300"
                >
                    "Votre IA, "
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        "connectée au droit."
                    </em>
                </h1>
                <p class="text-lg leading-relaxed text-[var(--color-ink-muted)]">
                    "Connectez Claude, ChatGPT ou votre propre agent à la jurisprudence française via le protocole MCP. La recherche sémantique et lexicale, directement dans le contexte de votre IA."
                </p>
                <CopyEndpoint url=mcp_url />
            </section>

            // Démo
            <section class="flex flex-col gap-4">
                <SectionLabel>"Démo"</SectionLabel>
                <div class="w-full min-w-0 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]">
                    <video
                        src="/mcp-demo.webm"
                        autoplay
                        loop
                        muted
                        playsinline
                        controls
                        class="block h-auto w-full min-w-0"
                    />
                </div>
            </section>

            // Installation
            <section class="flex flex-col gap-8">
                <SectionLabel>"Installation par client"</SectionLabel>

                <ClientHowto title="Claude (claude.ai)">
                    <Step n=1>
                        "Connectez-vous sur "
                        <a
                            href="https://claude.ai"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="underline underline-offset-2"
                        >
                            "claude.ai"
                        </a>
                        "."
                    </Step>
                    <Step n=2>
                        "En bas à gauche, cliquez sur votre avatar puis "
                        <strong>"Paramètres → Intégrations"</strong>
                        "."
                    </Step>
                    <Step n=3>
                        "Cliquez sur "
                        <strong>"Ajouter un connecteur personnalisé"</strong>
                        ", donnez un nom (ex. "
                        <em>"LibreJustice"</em>
                        ") et collez l'URL de l'endpoint."
                    </Step>
                    <Step n=4>
                        "Claude vous redirige vers LibreJustice pour valider la connexion via OAuth. Une fois autorisé, le connecteur est actif dans tous vos projets."
                    </Step>
                </ClientHowto>

                <ClientHowto title="ChatGPT">
                    <Step n=1>
                        "Connectez-vous sur "
                        <a
                            href="https://chatgpt.com"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="underline underline-offset-2"
                        >
                            "chatgpt.com"
                        </a>
                        " (compte Plus, Pro, Team ou Enterprise requis)."
                    </Step>
                    <Step n=2>
                        "Allez dans "
                        <strong>"Paramètres → Applications"</strong>
                        ", ouvrez "
                        <strong>"Paramètres avancés"</strong>
                        " et activez le "
                        <strong>"Developer mode"</strong>
                        "."
                    </Step>
                    <Step n=3>
                        "Revenez dans "
                        <strong>"Paramètres → Applications"</strong>
                        ", cliquez sur "
                        <strong>"Ajouter une application"</strong>
                        " et collez l'URL de l'endpoint."
                    </Step>
                    <Step n=4>
                        "Autorisez l'accès sur la page LibreJustice qui s'ouvre. Le connecteur est prêt dans vos conversations."
                    </Step>
                </ClientHowto>

                <ClientHowto title="Le Chat (Mistral)">
                    <Step n=1>
                        "Connectez-vous sur "
                        <a
                            href="https://chat.mistral.ai"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="underline underline-offset-2"
                        >
                            "chat.mistral.ai"
                        </a>
                        " (disponible sur tous les plans, y compris gratuit)."
                    </Step>
                    <Step n=2>
                        "Dans la barre latérale, ouvrez "
                        <strong>"Contexte → Connecteurs"</strong>
                        "."
                    </Step>
                    <Step n=3>
                        <strong>"+ Ajouter un connecteur → Connecteur MCP personnalisé"</strong>
                        ". Nommez (ex. "
                        <em>"LibreJustice"</em>
                        "), collez l'URL, méthode "
                        <strong>"OAuth"</strong>
                        "."
                    </Step>
                    <Step n=4>
                        <strong>"Créer"</strong>
                        " — validez la connexion sur la page LibreJustice qui s'ouvre."
                    </Step>
                </ClientHowto>

                <ClientHowto title="Perplexity">
                    <Step n=1>
                        "Connectez-vous sur "
                        <a
                            href="https://perplexity.ai"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="underline underline-offset-2"
                        >
                            "perplexity.ai"
                        </a>
                        "."
                    </Step>
                    <Step n=2>
                        "En bas à gauche, cliquez sur votre avatar puis "
                        <strong>"Connecteur"</strong>
                        "."
                    </Step>
                    <Step n=3>
                        "Sélectionnez "
                        <strong>"Connecteur personnalisé"</strong>
                        ". Renseignez le nom (ex. "
                        <em>"LibreJustice"</em>
                        ") et collez l'URL de l'endpoint."
                    </Step>
                    <Step n=4>
                        "Laissez "
                        <strong>"OAuth"</strong>
                        " comme méthode d'authentification, puis cliquez sur "
                        <strong>"Ajouter"</strong>
                        ". Perplexity vous redirige vers LibreJustice pour valider la connexion."
                    </Step>
                </ClientHowto>
            </section>

            // Prompts de test
            <section class="flex flex-col gap-4">
                <SectionLabel>"Exemples de requêtes"</SectionLabel>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    "Copiez ces prompts dans votre client IA pour tester l'intégration."
                </p>
                <ul class="flex flex-col gap-2">{prompts.collect_view()}</ul>
            </section>
        </div>
    }
}

/// Endpoint MCP + bouton copier. Port de `CopyEndpoint` : le SSR rend le fallback
/// canonique (`CANONICAL_BASE + "/mcp/"`) ; côté client, l'URL est ré-résolue
/// depuis l'origine courante (port de `window.location.origin`) après hydratation
/// — l'init identique au SSR évite tout décalage d'hydratation.
#[component]
fn CopyEndpoint(url: String) -> impl IntoView {
    let url_sig = RwSignal::new(url);
    let copied = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let origin = crate::pages::login_page::browser::location_origin();
        url_sig.set(format!("{origin}/mcp/"));
    });

    let on_copy = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let value = url_sig.get_untracked();
            leptos::task::spawn_local(async move {
                if crate::dom::copy_text(&value).await {
                    copied.set(true);
                    leptos::prelude::set_timeout(
                        move || copied.set(false),
                        std::time::Duration::from_millis(2000),
                    );
                }
            });
        }
    };

    view! {
        <div class="flex items-center gap-2 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)] px-4 py-3">
            <span class="shrink-0 text-xs uppercase tracking-[0.15em] text-[var(--color-ink-subtle)]">
                "Endpoint"
            </span>
            <code class="min-w-0 flex-1 truncate font-mono text-sm text-[var(--color-ink)]">
                {move || url_sig.get()}
            </code>
            <button
                type="button"
                on:click=on_copy
                aria-label="Copier l'URL"
                class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                {move || if copied.get() { "Copié ✓" } else { "Copier" }}
            </button>
        </div>
    }
}

#[component]
fn Step(n: u8, children: Children) -> impl IntoView {
    view! {
        <div class="flex gap-3">
            <span class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--color-rule)] text-xs font-medium text-[var(--color-ink-subtle)]">
                {n}
            </span>
            <div class="text-sm leading-relaxed text-[var(--color-ink-muted)]">{children()}</div>
        </div>
    }
}

#[component]
fn ClientHowto(title: &'static str, children: Children) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-4">
            <h3 class="font-sans text-base font-medium text-[var(--color-ink)]">{title}</h3>
            <div class="flex flex-col gap-4 border-l border-[var(--color-rule)] pl-4">
                {children()}
            </div>
        </div>
    }
}

#[component]
fn SectionLabel(children: Children) -> impl IntoView {
    view! {
        <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
            {children()}
        </p>
    }
}
