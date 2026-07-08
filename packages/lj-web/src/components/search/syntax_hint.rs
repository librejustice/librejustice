//! `SyntaxHint` (port de `syntax-hint.tsx`). Bouton « Aide à la recherche » qui
//! ouvre un `<dialog>` modal (modes + opérateurs booléens).
//!
//! `showModal()`/`close()` ne sont pas exposés par `view!` : on passe par un
//! `NodeRef<Dialog>` + `web_sys::HtmlDialogElement` (gated hydrate). En SSR le
//! `<dialog>` est rendu fermé (aucun JS) — parité avec le 1er rendu React.

use leptos::html;
use leptos::prelude::*;

use crate::helpers::cn;

const OPERATORS: &[(&str, &str)] = &[
    ("ET", "intersection"),
    ("OU", "union"),
    ("SAUF", "exclusion"),
    ("PROCHE5", "proximité de 5 mots"),
    ("\"…\"", "expression exacte"),
    ("*", "troncature"),
];

#[component]
pub fn SyntaxHint(#[prop(optional, into)] class: String) -> impl IntoView {
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
        <button type="button" on:click=open class=trigger_class>
            "Aide à la recherche"
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
                        "Aide à la recherche"
                    </h3>
                    <button
                        type="button"
                        on:click=move |_| close_btn()
                        aria-label="Fermer"
                        class="shrink-0 text-lg leading-none text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]"
                    >
                        "✕"
                    </button>
                </div>
                <section class="flex flex-col gap-2">
                    <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                        "Modes de recherche"
                    </h4>
                    <dl class="flex flex-col gap-1.5 text-xs">
                        <div class="flex gap-2">
                            <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">
                                "Sémantique"
                            </dt>
                            <dd class="text-[var(--color-ink-muted)]">
                                "Répond à votre demande et trouve des décisions pertinentes même sans la présence stricte des mots de celle-ci. Mode par défaut."
                            </dd>
                        </div>
                        <div class="flex gap-2">
                            <dt class="w-28 shrink-0 font-medium text-[var(--color-ink)]">
                                "Lexicale"
                            </dt>
                            <dd class="text-[var(--color-ink-muted)]">
                                "Correspondance stricte sur les mots-clés et opérateurs. Activé automatiquement dès qu'un opérateur est détecté dans la requête."
                            </dd>
                        </div>
                    </dl>
                </section>
                <section class="flex flex-col gap-2">
                    <h4 class="text-[0.7rem] font-medium uppercase tracking-[0.15em] text-[var(--color-ink-muted)]">
                        "Opérateurs booléens"
                    </h4>
                    <div class="flex flex-wrap gap-x-4 gap-y-2 text-xs">
                        {OPERATORS
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
                </section>
            </div>
        </dialog>
    }
}
