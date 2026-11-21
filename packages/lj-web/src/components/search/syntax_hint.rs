//! `SyntaxHint` : bouton « Aide à la recherche » qui
//! ouvre un `<dialog>` modal dont le contenu suit le moteur documenté
//! ([`HelpCorpus`]) : décisions (modes + opérateurs booléens) ou textes
//! (mots-clés, texte nommé, numéro d'article, filtres).
//!
//! `showModal()`/`close()` ne sont pas exposés par `view!` : on passe par un
//! `NodeRef<Dialog>` + `web_sys::HtmlDialogElement` (gated hydrate). En SSR le
//! `<dialog>` est rendu fermé (aucun JS).

use leptos::html;
use leptos::prelude::*;
use lj_dtos::UiAction;

use super::mode_select::decision_modes;
use crate::helpers::cn;
use crate::i18n::use_ui;

/// The engine the help documents: textes has no operators and no semantic mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpCorpus {
    #[default]
    Decisions,
    Textes,
}

#[component]
pub fn SyntaxHint(
    #[prop(optional)] corpus: HelpCorpus,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let dialog_ref = NodeRef::<html::Dialog>::new();

    let open = move |_| {
        #[cfg(feature = "hydrate")]
        if let Some(d) = dialog_ref.get() {
            let _ = d.show_modal();
        }
        let _ = dialog_ref;
    };
    let close = move || {
        #[cfg(feature = "hydrate")]
        if let Some(d) = dialog_ref.get() {
            d.close();
        }
    };

    // `close` est `Copy` (closure sans capture par valeur hors `NodeRef` Copy) :
    // capturé par copie dans chaque handler.
    let on_backdrop = move |ev: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            let is_dialog = dialog_ref
                .get()
                .zip(ev.target())
                .map(|(d, t)| t.dyn_ref::<web_sys::Node>() == Some(d.unchecked_ref()))
                .unwrap_or(false);
            if is_dialog {
                close();
            }
        }
        // SSR : `close` et `ev` ne sont pas consommés (pas de DOM).
        let _ = (&ev, &close);
    };

    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Escape" {
            close();
        }
    };

    let close_btn = close;

    let trigger_class = cn([
        "inline-flex items-center gap-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]",
        &class,
    ]);

    view! {
        <button
            type="button"
            on:click=open
            class=trigger_class
            data-usage=UiAction::SearchSyntaxHelp.as_str()
        >
            {use_ui().search.help}
            <span aria-hidden="true" class="opacity-60">
                "?"
            </span>
        </button>
        <dialog
            node_ref=dialog_ref
            on:click=on_backdrop
            on:keydown=on_keydown
            class="m-auto w-full max-w-lg rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] p-0 shadow-xl"
            style="overscroll-behavior: contain"
        >
            <div class="flex flex-col gap-5 p-6">
                <div class="flex items-center justify-between gap-4">
                    <h3
                        class="font-sans text-sm text-[var(--color-ink)]"
                        style="font-variation-settings: 'wght' 600"
                    >
                        {use_ui().search.help}
                    </h3>
                    <button
                        type="button"
                        on:click=move |_| close_btn()
                        aria-label=use_ui().search.close
                        class="shrink-0 text-lg leading-none text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]"
                    >
                        "✕"
                    </button>
                </div>
                {match corpus {
                    HelpCorpus::Decisions => decisions_help(),
                    HelpCorpus::Textes => textes_help(),
                }}
            </div>
        </dialog>
    }
}

/// The decisions help: the four modes as their pills word them, how to name one
/// decision, the operators.
fn decisions_help() -> AnyView {
    let ui = &use_ui().search;
    view! {
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.help_modes}
            </h4>
            <dl class="flex flex-col gap-1.5 text-xs">
                {decision_modes(ui)
                    .into_iter()
                    .map(|(_, label, tooltip)| {
                        view! {
                            <div class="flex gap-2">
                                <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">
                                    {label}
                                </dt>
                                <dd class="text-[var(--color-ink-muted)]">{tooltip}</dd>
                            </div>
                        }
                    })
                    .collect::<Vec<_>>()}
            </dl>
        </section>
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.help_find_one}
            </h4>
            {code_chips(ui.help_designations)}
        </section>
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.ai_sort}
            </h4>
            <p class="text-xs text-[var(--color-ink-muted)]">
                {ui.help_ai_sort_text}
            </p>
        </section>
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.help_operators_title}
            </h4>
            {code_chips(ui.help_operators)}
        </section>
    }
    .into_any()
}

fn code_chips(items: &'static [(&'static str, &'static str)]) -> impl IntoView {
    view! {
        <div class="flex flex-wrap gap-x-4 gap-y-2 text-xs">
            {items
                .iter()
                .map(|(token, meaning)| {
                    view! {
                        <span class="inline-flex items-center gap-1.5">
                            <code class="rounded-xs bg-[var(--color-vellum)] px-1.5 py-0.5 font-mono text-[0.72rem] text-[var(--color-ink)]">
                                {*token}
                            </code>
                            <span class="text-[var(--color-ink-muted)]">{*meaning}</span>
                        </span>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// Corps de l'aide textes : classement par mots-clés, sans opérateurs — les
/// trois gestes qui marchent, puis les filtres.
fn textes_help() -> AnyView {
    let ui = &use_ui().search;
    view! {
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.help_how}
            </h4>
            <dl class="flex flex-col gap-1.5 text-xs">
                <div class="flex gap-2">
                    <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">{ui.help_notion}</dt>
                    <dd class="text-[var(--color-ink-muted)]">
                        {ui.help_notion_text}
                    </dd>
                </div>
                <div class="flex gap-2">
                    <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">
                        {ui.help_named_text}
                    </dt>
                    <dd class="text-[var(--color-ink-muted)]">
                        {ui.help_named_text_text}
                    </dd>
                </div>
                <div class="flex gap-2">
                    <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">
                        {ui.help_article_number}
                    </dt>
                    <dd class="text-[var(--color-ink-muted)]">
                        {ui.help_article_number_text}
                    </dd>
                </div>
            </dl>
        </section>
        <section class="flex flex-col gap-2">
            <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                {ui.help_refine}
            </h4>
            <p class="text-xs text-[var(--color-ink-muted)]">
                {ui.help_refine_text}
            </p>
        </section>
    }
    .into_any()
}
