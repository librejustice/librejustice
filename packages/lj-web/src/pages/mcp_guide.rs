//! Guide MCP. `<Title>` propre, hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::seo::CANONICAL_BASE;

/// Prompt d'installation pour les applications agentiques (ChatGPT
/// Desktop, Claude Desktop…) : une ligne pointant vers le guide agent
/// `llms-install.md` du dépôt public (convention popularisée par Cline,
/// suivie par le catalogue MCP de Microsoft, Magic MCP, Firebase MCP…).
const UNIVERSAL_PROMPT: &str = "Installe LibreJustice (MCP + compétences) en suivant ce guide : https://github.com/librejustice/librejustice/blob/main/llms-install.md";

/// Fiche du connecteur dans le répertoire officiel claude.ai : installation en
/// un clic, sans saisie de l'URL de l'endpoint.
const CLAUDE_DIRECTORY_URL: &str = "https://claude.ai/directory/connectors/librejustice";

const CHATGPT_DIRECTORY_URL: &str =
    "https://chatgpt.com/plugins/plugin_asdk_app_6a053194fe5c81918e1a3b98ab1f4d63";

/// Prompts d'exemple a copier dans un client IA. Nommer LibreJustice dans la
/// question est ce qui declenche l'appel du connecteur.
const TEST_PROMPTS: [&str; 6] = [
    "En vous appuyant sur LibreJustice, est-ce que vous pouvez me donner sous quelles conditions le Conseil d'État admet la responsabilité sans faute de l'État ?",
    "En vous appuyant sur LibreJustice, pouvez-vous me donner les conditions dans lesquelles le juge statuant en référé-liberté fait droit à la requête lorsque le préfet n'a pas exécuté l'injonction prononcée par une ordonnance de référé suspension rendue au préalable",
    "Pouvez-vous chercher sur LibreJustice quelle valeur du point de déficit fonctionnel permanent les cours d'appel retiennent pour une victime de trente ans avec un taux de 10 %, et dans quelle fourchette ?",
    "En vous appuyant sur LibreJustice, à quelles conditions la responsabilité d'un établissement de santé est-elle engagée pour une infection nosocomiale ? Citez les décisions sur lesquelles vous vous appuyez.",
    "En vous appuyant sur LibreJustice, ouvrez les arrêts de cour d'appel sur l'aliénation parentale et dites-moi comment ils l'articulent avec l'intérêt de l'enfant.",
    "En vous appuyant sur LibreJustice, dites-moi si un mécanicien licencié en 2015 avec dix mois d'ancienneté avait droit à l'indemnité de licenciement de la convention collective de l'automobile, et si la réponse serait la même aujourd'hui.",
];

#[component]
pub fn McpGuide() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let mcp_url = format!("{CANONICAL_BASE}/mcp/");
    let prompts = TEST_PROMPTS
        .iter()
        .map(|prompt| view! { <CopyPrompt text=*prompt /> });

    view! {
        <Title text="MCP - LibreJustice" />
        <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-16 px-4 py-16 sm:px-6 lg:px-8">
            // Hero
            <section class="flex flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Connecteur MCP"
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
                    "Connectez Claude, ChatGPT ou votre propre agent au droit français (jurisprudence et textes) via le protocole MCP. La recherche par mots-clés et par reformulation, directement dans le contexte de votre IA."
                </p>
                <div class="flex flex-wrap items-center gap-3">
                    <a
                        href=CLAUDE_DIRECTORY_URL
                        target="_blank"
                        rel="noopener noreferrer"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        "Ajouter à Claude"
                    </a>
                    <a
                        href=CHATGPT_DIRECTORY_URL
                        target="_blank"
                        rel="noopener noreferrer"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        "Ajouter à ChatGPT"
                    </a>
                </div>
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

                <div class="flex flex-col gap-3">
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        "Claude et ChatGPT s'installent par les boutons en haut de page. Les autres clients n'ont pas encore de fiche LibreJustice dans leur répertoire : ils l'ajoutent comme connecteur personnalisé, à partir de l'adresse du connecteur."
                    </p>
                    <CopyEndpoint url=mcp_url />
                </div>

                <ClientHowto title="Claude">
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        "Claude demande votre accord à chaque appel d'un outil LibreJustice tant que vous ne l'avez pas autorisé. Pour ne plus avoir à valider : "
                        <strong>"Personnalisation → Connecteurs → LibreJustice"</strong>
                        ", puis réglez "
                        <strong>"Outils en lecture seule"</strong>
                        " sur "
                        <strong>"Toujours autoriser"</strong>
                        "."
                    </p>
                </ClientHowto>

                <ClientHowto title="Vibe (Mistral)">
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
                        <strong>"Ajouter un connecteur → Ajouter un connecteur personnalisé"</strong>
                        ". Nommez (ex. "
                        <em>"LibreJustice"</em>
                        ") et collez l'adresse dans "
                        <strong>"Serveur"</strong>
                        " : l'authentification OAuth est détectée d'elle-même."
                    </Step>
                    <Step n=4>
                        <strong>"Ajouter un connecteur"</strong>
                        ", puis validez la connexion sur la page LibreJustice qui s'ouvre."
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
                        " (offre Pro, Max ou Enterprise requise)."
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
                        ") et collez l'adresse du connecteur."
                    </Step>
                    <Step n=4>
                        "Choisissez "
                        <strong>"OAuth"</strong>
                        " comme méthode d'authentification, puis cliquez sur "
                        <strong>"Ajouter"</strong>
                        ". Perplexity vous redirige vers LibreJustice pour valider la connexion."
                    </Step>
                </ClientHowto>
            </section>

            // Compétences : la méthode de recherche, en plus du connecteur
            <section class="flex flex-col gap-8">
                <SectionLabel>"Compétences"</SectionLabel>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "Deux compétences accompagnent le connecteur : "
                    <strong>"recherche-jurisprudence"</strong>
                    " et "
                    <strong>"recherche-normes"</strong>
                    ". Une compétence est un mode d'emploi que votre agent lit avant de chercher : lire une décision en intégralité avant de la citer, viser la version d'un texte à la date du litige, éviter les pièges du corpus. Le connecteur fonctionne sans, mais les réponses sont nettement plus fiables avec."
                </p>

                <ClientHowto title="Claude Web">
                    <Step n=1>
                        "Téléchargez la compétence : "
                        <a
                            href="/skills/recherche-jurisprudence.zip"
                            download
                            class="underline underline-offset-2"
                        >
                            "recherche-jurisprudence.zip"
                        </a>
                        " ou "
                        <a
                            href="/skills/recherche-normes.zip"
                            download
                            class="underline underline-offset-2"
                        >
                            "recherche-normes.zip"
                        </a>
                        "."
                    </Step>
                    <Step n=2>
                        "Sur Claude Web : avatar en bas à gauche → "
                        <strong>"Paramètres → Personnaliser → Compétences"</strong>
                        " (tous les plans)."
                    </Step>
                    <Step n=3>
                        <strong>"Ajouter → Téléverser une compétence"</strong>
                        ", puis sélectionnez le zip. Elle s'active dans vos conversations (le connecteur ajouté ci-dessus reste nécessaire)."
                    </Step>
                </ClientHowto>

                <ClientHowto title="ChatGPT">
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        "L'application du répertoire installe les deux compétences avec le connecteur."
                    </p>
                </ClientHowto>
            </section>

            // Prompts de test
            <section class="flex flex-col gap-4">
                <SectionLabel>"Exemples de requêtes"</SectionLabel>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    "Copiez ces requêtes dans votre client IA pour tester l'intégration."
                </p>
                <ul class="flex flex-col gap-2">{prompts.collect_view()}</ul>
            </section>

            // Installation par prompt : le plugin complet (connecteur MCP + compétences)
            <section class="flex flex-col gap-8">
                <SectionLabel>"Installation par instruction"</SectionLabel>

                <ClientHowto title="ChatGPT Desktop, Claude Desktop…">
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        "Ces applications peuvent installer LibreJustice elles-mêmes, connecteur et compétences d'un coup : collez cette instruction, l'assistant suit le guide "
                        <a
                            href="https://github.com/librejustice/librejustice/blob/main/llms-install.md"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="underline underline-offset-2"
                        >
                            "llms-install.md"
                        </a>
                        " du dépôt."
                    </p>
                    <CopyBlock text=UNIVERSAL_PROMPT label="Prompt d'installation" />
                </ClientHowto>
            </section>

            <section class="flex flex-col gap-4">
                <SectionLabel>"Plutôt du code ?"</SectionLabel>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    "Si le consommateur est votre programme et non une IA, le même fonds se lit en REST : "
                    <a href="/api-guide" class="underline underline-offset-2">
                        "l'API publique"
                    </a>
                    ", clé gratuite et contrat OpenAPI."
                </p>
            </section>
        </div>
    }
}

/// Endpoint MCP + bouton copier : le SSR rend le fallback
/// canonique (`CANONICAL_BASE + "/mcp/"`) ; côté client, l'URL est ré-résolue
/// depuis l'origine courante après hydratation
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
                aria-label="Copier l'adresse"
                class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                {move || if copied.get() { "Copié ✓" } else { "Copier" }}
            </button>
        </div>
    }
}

/// Bloc de texte copiable (commandes, prompt). Statique au SSR ; le bouton
/// copie via l'API clipboard après hydratation (même pattern que `CopyEndpoint`).
#[component]
fn CopyBlock(text: &'static str, label: &'static str) -> impl IntoView {
    let copied = RwSignal::new(false);

    let on_copy = move |_| {
        #[cfg(feature = "hydrate")]
        {
            leptos::task::spawn_local(async move {
                if crate::dom::copy_text(text).await {
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
        <div class="overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]">
            <div class="flex items-center justify-between gap-2 border-b border-[var(--color-rule)] px-4 py-2">
                <span class="text-xs uppercase tracking-[0.15em] text-[var(--color-ink-subtle)]">
                    {label}
                </span>
                <button
                    type="button"
                    on:click=on_copy
                    aria-label="Copier"
                    class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
                >
                    {move || if copied.get() { "Copié ✓" } else { "Copier" }}
                </button>
            </div>
            <pre class="overflow-x-auto whitespace-pre-wrap px-4 py-3 font-mono text-sm leading-relaxed text-[var(--color-ink)]">{text}</pre>
        </div>
    }
}

/// Prompt d'exemple + bouton copier (même pattern clipboard que `CopyBlock`).
/// Le texte se replie : tronqué, un prompt long ne se relit pas et ne dit plus
/// ce qu'il copie.
#[component]
fn CopyPrompt(text: &'static str) -> impl IntoView {
    let copied = RwSignal::new(false);

    let on_copy = move |_| {
        #[cfg(feature = "hydrate")]
        {
            leptos::task::spawn_local(async move {
                if crate::dom::copy_text(text).await {
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
        <li class="flex items-start gap-2 border-l-2 border-[var(--color-accent)] py-1 pl-4">
            <span class="min-w-0 flex-1 text-sm leading-relaxed text-[var(--color-ink-muted)]">
                {text}
            </span>
            <button
                type="button"
                on:click=on_copy
                aria-label="Copier l'instruction"
                class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                {move || if copied.get() { "Copié ✓" } else { "Copier" }}
            </button>
        </li>
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
