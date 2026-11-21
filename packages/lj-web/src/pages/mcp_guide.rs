//! Guide MCP. `<Title>` propre, hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::prose;
use crate::seo::CANONICAL_BASE;

const LIEN: &str = "underline underline-offset-2";

/// Fiche du connecteur dans le répertoire officiel claude.ai : installation en
/// un clic, sans saisie de l'URL de l'endpoint.
const CLAUDE_DIRECTORY_URL: &str = "https://claude.ai/directory/connectors/librejustice";

const CHATGPT_DIRECTORY_URL: &str =
    "https://chatgpt.com/plugins/plugin_asdk_app_6a053194fe5c81918e1a3b98ab1f4d63";

#[component]
pub fn McpGuide() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let ui = &crate::i18n::use_ui().mcp_guide;
    let mcp_url = format!("{CANONICAL_BASE}/mcp/");
    let prompts = ui
        .prompts
        .iter()
        .map(|prompt| view! { <CopyPrompt text=*prompt /> });
    let steps = |steps: &'static [&'static [crate::i18n::Run]]| {
        steps
            .iter()
            .enumerate()
            .map(|(i, runs)| view! { <Step n=i as u8 + 1>{prose(runs, LIEN)}</Step> })
            .collect_view()
    };

    view! {
        <Title text=ui.title />
        <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-16 px-4 py-16 sm:px-6 lg:px-8">
            // Hero
            <section class="flex flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.eyebrow}
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl lg:text-[3.5rem]"
                    style="font-variation-settings: 'wght' 300"
                >
                    {ui.heading}
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        {ui.heading_em}
                    </em>
                </h1>
                <p class="text-lg leading-relaxed text-[var(--color-ink-muted)]">
                    {ui.intro}
                </p>
                <div class="flex flex-wrap items-center gap-3">
                    <a
                        href=CLAUDE_DIRECTORY_URL
                        target="_blank"
                        rel="noopener noreferrer"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        {ui.add_to_claude}
                    </a>
                    <a
                        href=CHATGPT_DIRECTORY_URL
                        target="_blank"
                        rel="noopener noreferrer"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        {ui.add_to_chatgpt}
                    </a>
                </div>
            </section>

            // Démo
            <section class="flex flex-col gap-4">
                <SectionLabel>{ui.demo}</SectionLabel>
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
                <SectionLabel>{ui.install_heading}</SectionLabel>

                <div class="flex flex-col gap-3">
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        {ui.install_intro}
                    </p>
                    <CopyEndpoint url=mcp_url />
                </div>

                <ClientHowto title=ui.claude>
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        {prose(ui.claude_note, LIEN)}
                    </p>
                </ClientHowto>

                <ClientHowto title=ui.vibe>{steps(ui.vibe_steps)}</ClientHowto>

                <ClientHowto title=ui.perplexity>{steps(ui.perplexity_steps)}</ClientHowto>
            </section>

            // Compétences : la méthode de recherche, en plus du connecteur
            <section class="flex flex-col gap-8">
                <SectionLabel>{ui.skills_heading}</SectionLabel>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {prose(ui.skills_intro, LIEN)}
                </p>

                <ClientHowto title=ui.claude_web>{steps(ui.claude_web_steps)}</ClientHowto>

                <ClientHowto title=ui.chatgpt>
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        {ui.chatgpt_skills}
                    </p>
                </ClientHowto>
            </section>

            // Prompts de test
            <section class="flex flex-col gap-4">
                <SectionLabel>{ui.prompts_heading}</SectionLabel>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    {ui.prompts_intro}
                </p>
                <ul class="flex flex-col gap-2">{prompts.collect_view()}</ul>
            </section>

            // Installation par prompt : le plugin complet (connecteur MCP + compétences)
            <section class="flex flex-col gap-8">
                <SectionLabel>{ui.instruction_heading}</SectionLabel>

                <ClientHowto title=ui.desktop>
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        {prose(ui.desktop_note, LIEN)}
                    </p>
                    <CopyBlock text=ui.install_prompt label=ui.install_prompt_label />
                </ClientHowto>
            </section>

            <section class="flex flex-col gap-4">
                <SectionLabel>{ui.code_heading}</SectionLabel>
                <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                    {ui.code_lead}
                    <a href=crate::jurisdiction::use_reader_scope().href("/api-guide") class=LIEN>
                        {ui.code_link}
                    </a>
                    {ui.code_tail}
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
    let ui = &crate::i18n::use_ui().mcp_guide;
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
                {ui.endpoint}
            </span>
            <code class="min-w-0 flex-1 truncate font-mono text-sm text-[var(--color-ink)]">
                {move || url_sig.get()}
            </code>
            <button
                type="button"
                on:click=on_copy
                aria-label=ui.copy_address
                class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                {move || if copied.get() { ui.copied } else { ui.copy }}
            </button>
        </div>
    }
}

/// Bloc de texte copiable (commandes, prompt). Statique au SSR ; le bouton
/// copie via l'API clipboard après hydratation (même pattern que `CopyEndpoint`).
#[component]
fn CopyBlock(text: &'static str, label: &'static str) -> impl IntoView {
    let ui = &crate::i18n::use_ui().mcp_guide;
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
                    aria-label=ui.copy
                    class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
                >
                    {move || if copied.get() { ui.copied } else { ui.copy }}
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
    let ui = &crate::i18n::use_ui().mcp_guide;
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
                aria-label=ui.copy_instruction
                class="shrink-0 rounded px-2 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:bg-[var(--color-rule)] hover:text-[var(--color-ink)]"
            >
                {move || if copied.get() { ui.copied } else { ui.copy }}
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
