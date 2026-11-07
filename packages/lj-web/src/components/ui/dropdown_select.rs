//! `DropdownSelect` : bouton + panneau
//! flottant custom, fermeture au clic exterieur.

use leptos::prelude::*;

use super::panel::PanelAnchor;
#[cfg(feature = "hydrate")]
use super::panel::{dismiss, Dismiss, PanelEdge};
use super::SelectOption;

/// `field` draws it as a form field: full width, at an input's height.
#[component]
pub fn DropdownSelect(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(into)] options: Signal<Vec<SelectOption>>,
    #[prop(optional, into)] aria_label: String,
    #[prop(optional, into)] class: String,
    #[prop(optional)] field: bool,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let button_ref = NodeRef::<leptos::html::Button>::new();
    let anchor = RwSignal::new(None::<PanelAnchor>);

    #[cfg(feature = "hydrate")]
    dismiss(
        container_ref,
        move || open.get_untracked(),
        move || open.set(false),
        Dismiss {
            escape: false,
            scroll: true,
        },
    );

    let current_label = move || {
        options
            .get()
            .into_iter()
            .find(|o| o.value == value.get())
            .map(|o| o.label)
            .unwrap_or_default()
    };

    let on_toggle = move |_| {
        let next = !open.get_untracked();
        #[cfg(feature = "hydrate")]
        if next {
            if let Some(button) = button_ref.get_untracked() {
                anchor.set(Some(PanelAnchor::measure(&button, PanelEdge::Right, 240.0)));
            }
        }
        open.set(next);
    };

    let wrapper = format!("relative inline-flex {class}");
    let (trigger, text_size) = if field {
        (
            "flex h-10 w-full items-center justify-between gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 text-left text-sm text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]",
            "text-sm",
        )
    } else {
        (
            "flex items-center gap-1.5 rounded border border-[var(--color-rule)] px-2.5 py-1 text-xs text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]",
            "text-xs",
        )
    };

    view! {
        <div node_ref=container_ref class=wrapper>
            <button
                node_ref=button_ref
                type="button"
                aria-label=aria_label
                aria-expanded=move || open.get().then_some("true")
                on:click=on_toggle
                class=trigger
            >
                <span class="min-w-0 truncate">{current_label}</span>
                <svg
                    viewBox="0 0 12 8"
                    class=move || {
                        format!(
                            "h-2 w-2.5 shrink-0 text-[var(--color-ink-subtle)] transition-transform {}",
                            if open.get() { "rotate-180" } else { "" },
                        )
                    }
                    fill="none"
                    aria-hidden="true"
                >
                    <path
                        d="M1 1.5l5 5 5-5"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                </svg>
            </button>
            <div
                style=move || PanelAnchor::style(anchor.get())
                data-open=move || open.get().then_some("true")
                class="lj-panel z-50 max-h-60 overflow-y-auto overscroll-contain rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] py-1 shadow-lg"
            >
                <For
                    each=move || options.get()
                    key=|opt| opt.value.clone()
                    let:opt
                >
                    {
                            let opt_value = opt.value.clone();
                            let selected = move || value.get() == opt_value;
                            let opt_value_click = opt.value.clone();
                            view! {
                                <button
                                    type="button"
                                    on:click=move |_| {
                                        on_change.run(opt_value_click.clone());
                                        open.set(false);
                                    }
                                    class=move || {
                                        // `block` (pas l'`inline-block` par défaut du
                                        // `<button>`) : empile verticalement, sinon sous
                                        // `width:max-content` les options se mettent côte
                                        // à côte.
                                        format!(
                                            "block w-full whitespace-nowrap px-3 py-1.5 text-left {text_size} transition-colors {}",
                                            if selected() {
                                                "bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
                                            } else {
                                                "text-[var(--color-ink-muted)] hover:bg-[var(--color-bordeaux-soft)]/40 hover:text-[var(--color-ink)]"
                                            },
                                        )
                                    }
                                >
                                    {opt.label.clone()}
                                </button>
                            }
                        }
                </For>
            </div>
        </div>
    }
}
