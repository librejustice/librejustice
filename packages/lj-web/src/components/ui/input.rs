//! `Input` (port de `ui/input.tsx`). Wrapper `div` (focus-within + invalid) +
//! `<input>` interne, slots `leading`/`trailing` optionnels.
//!
//! Le wrapper porte l'etat focus/invalid ; l'`<input>` interne est la cible des
//! evenements. Les tranches branchent `on:input`/`prop:value` via `node_ref`
//! (passe a `input_ref`), ou spreadent des attributs avec `{..}` sur l'`<Input>`
//! (qui retombent sur le wrapper) — pour cibler l'input lui-meme, utiliser le ref.

use leptos::html;
use leptos::prelude::*;

use crate::helpers::cn;

/// Champ texte. `class` complete le wrapper. `input_ref` expose l'`<input>`
/// interne (branchement evenements/valeur cote appelant).
#[component]
pub fn Input(
    #[prop(default = "text", into)] r#type: &'static str,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(optional, into)] class: String,
    #[prop(optional)] input_ref: NodeRef<html::Input>,
    #[prop(optional)] leading: Option<ViewFn>,
    #[prop(optional)] trailing: Option<ViewFn>,
) -> impl IntoView {
    let is_invalid = invalid.get_untracked();
    let wrapper = cn([
        "group flex h-11 w-full min-w-0 items-center gap-2 rounded-md border border-[var(--color-rule)]",
        "bg-[var(--color-parchment)] px-3 transition-colors",
        "has-[:focus-visible]:border-[var(--color-ink)]",
        if is_invalid {
            "border-[var(--color-accent)] has-[:focus-visible]:border-[var(--color-accent)]"
        } else {
            ""
        },
        &class,
    ]);
    let input_classes = cn([
        "h-full min-w-0 flex-1 bg-transparent text-[var(--color-ink)] outline-none",
        "placeholder:text-[var(--color-ink-subtle)]",
        "disabled:cursor-not-allowed disabled:opacity-60",
    ]);
    view! {
        <div class=wrapper>
            {leading
                .map(|l| {
                    view! {
                        <span class="text-[var(--color-ink-subtle)]" aria-hidden="true">
                            {l.run()}
                        </span>
                    }
                })}
            <input
                node_ref=input_ref
                type=r#type
                size="1"
                placeholder=placeholder
                aria-invalid=move || invalid.get().then_some("true")
                class=input_classes
            />
            {trailing
                .map(|t| {
                    view! { <span class="text-[var(--color-ink-subtle)]">{t.run()}</span> }
                })}
        </div>
    }
}
