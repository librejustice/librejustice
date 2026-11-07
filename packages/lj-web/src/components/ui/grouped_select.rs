use leptos::html;
use leptos::prelude::*;

use super::panel::PanelAnchor;
#[cfg(feature = "hydrate")]
use super::panel::{dismiss, Dismiss, PanelEdge};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectGroup {
    pub label: String,
    pub options: Vec<GroupedOption>,
}

/// An option, and a word shown after its label in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupedOption {
    pub value: String,
    pub label: String,
    pub note: Option<&'static str>,
}

#[cfg(feature = "hydrate")]
const PANEL_CAP: f64 = 360.0;

#[component]
pub fn GroupedSelect(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    groups: Vec<SelectGroup>,
    #[prop(into)] aria_label: String,
    filter_placeholder: &'static str,
    empty_text: &'static str,
) -> impl IntoView {
    let groups = StoredValue::new(groups);
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(None::<String>);
    let anchor = RwSignal::new(None::<PanelAnchor>);
    let container_ref = NodeRef::<html::Div>::new();
    let button_ref = NodeRef::<html::Button>::new();
    let input_ref = NodeRef::<html::Input>::new();
    let list_ref = NodeRef::<html::Div>::new();

    let visible = Memo::new(move |_| {
        let needle = lj_fold::fold(query.get().trim());
        groups.with_value(|groups| {
            groups
                .iter()
                .filter_map(|g| {
                    let options: Vec<GroupedOption> = g
                        .options
                        .iter()
                        .filter(|o| needle.is_empty() || lj_fold::fold(&o.label).contains(&needle))
                        .cloned()
                        .collect();
                    (!options.is_empty()).then(|| SelectGroup {
                        label: g.label.clone(),
                        options,
                    })
                })
                .collect::<Vec<_>>()
        })
    });
    let flat = move || {
        visible.with(|groups| {
            groups
                .iter()
                .flat_map(|g| g.options.iter().map(|o| o.value.clone()))
                .collect::<Vec<_>>()
        })
    };

    let close = move || {
        open.set(false);
        query.set(String::new());
    };
    let pick = move |picked: String| {
        close();
        if let Some(button) = button_ref.get_untracked() {
            let _ = button.focus();
        }
        if picked != value.get_untracked() {
            on_change.run(picked);
        }
    };

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        dismiss(
            container_ref,
            move || open.get_untracked(),
            close,
            Dismiss::default(),
        );
        let _ = leptos_use::use_event_listener_with_options(
            leptos_use::use_document(),
            leptos::ev::scroll,
            move |ev| {
                if !open.get_untracked() {
                    return;
                }
                let inside = container_ref
                    .get_untracked()
                    .zip(ev.target())
                    .and_then(|(el, target)| {
                        target
                            .dyn_ref::<web_sys::Node>()
                            .map(|node| el.contains(Some(node)))
                    })
                    .unwrap_or(false);
                if !inside {
                    close();
                }
            },
            leptos_use::UseEventListenerOptions::default().capture(true),
        );
    }

    let on_toggle = move |_| {
        if open.get_untracked() {
            close();
            return;
        }
        #[cfg(feature = "hydrate")]
        if let Some(button) = button_ref.get_untracked() {
            anchor.set(Some(PanelAnchor::measure(
                &button,
                PanelEdge::Left,
                PANEL_CAP,
            )));
        }
        active.set(Some(value.get_untracked()));
        open.set(true);
        request_animation_frame(move || {
            if let Some(input) = input_ref.get_untracked() {
                let _ = input.focus();
            }
            reveal_active(list_ref);
        });
    };

    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        let step: isize = match ev.key().as_str() {
            "ArrowDown" => 1,
            "ArrowUp" => -1,
            "Enter" => {
                ev.prevent_default();
                let values = flat();
                let current = active.get_untracked();
                let picked = current
                    .filter(|v| values.contains(v))
                    .or_else(|| values.first().cloned());
                if let Some(picked) = picked {
                    pick(picked);
                }
                return;
            }
            "Escape" => {
                ev.prevent_default();
                close();
                if let Some(button) = button_ref.get_untracked() {
                    let _ = button.focus();
                }
                return;
            }
            _ => return,
        };
        ev.prevent_default();
        let values = flat();
        if values.is_empty() {
            return;
        }
        let at = active
            .get_untracked()
            .and_then(|v| values.iter().position(|x| *x == v));
        let next = match at {
            Some(i) => (i as isize + step).clamp(0, values.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => values.len() - 1,
        };
        active.set(Some(values[next].clone()));
        request_animation_frame(move || reveal_active(list_ref));
    };

    let current_label = move || {
        let current = value.get();
        groups.with_value(|groups| {
            groups
                .iter()
                .flat_map(|g| g.options.iter())
                .find(|o| o.value == current)
                .map(|o| o.label.clone())
                .unwrap_or_default()
        })
    };

    view! {
        <div node_ref=container_ref class="relative inline-flex max-w-full">
            <button
                node_ref=button_ref
                type="button"
                aria-label=aria_label
                aria-haspopup="listbox"
                aria-expanded=move || open.get().then_some("true")
                on:click=on_toggle
                class="flex min-h-9 max-w-full items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-1.5 text-left text-sm text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]"
            >
                <span>{current_label}</span>
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
                class="lj-panel z-50 flex max-w-[calc(100vw-1rem)] flex-col rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg"
            >
                <div class="border-b border-[var(--color-rule)] p-2">
                    <input
                        node_ref=input_ref
                        type="text"
                        autocomplete="off"
                        placeholder=filter_placeholder
                        aria-label=filter_placeholder
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            query.set(event_target_value(&ev));
                            active.set(flat().into_iter().next());
                        }
                        on:keydown=on_keydown
                        class="h-8 w-full rounded border border-[var(--color-rule)] bg-transparent px-2 text-sm text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-subtle)] focus:border-[var(--color-ink)]"
                    />
                </div>
                <div
                    node_ref=list_ref
                    role="listbox"
                    aria-label=filter_placeholder
                    class="min-h-0 flex-1 overflow-y-auto overscroll-contain py-1"
                >
                    <Show
                        when=move || visible.with(|g| !g.is_empty())
                        fallback=move || {
                            view! {
                                <p class="px-3 py-2 text-xs text-[var(--color-ink-subtle)]">
                                    {empty_text}
                                </p>
                            }
                        }
                    >
                        <For
                            each=move || visible.get()
                            key=|group| {
                                (
                                    group.label.clone(),
                                    group.options.iter().map(|o| o.value.clone()).collect::<Vec<_>>(),
                                )
                            }
                            let:group
                        >
                            <p class="px-3 pb-1 pt-2 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                                {group.label.clone()}
                            </p>
                            {group
                                .options
                                .into_iter()
                                .map(|opt| {
                                    let GroupedOption { value: opt_value, label, note } = opt;
                                    let is_selected = {
                                        let opt_value = opt_value.clone();
                                        move || value.get() == opt_value
                                    };
                                    let is_active = {
                                        let opt_value = opt_value.clone();
                                        move || active.get().as_deref() == Some(opt_value.as_str())
                                    };
                                    let hover_value = opt_value.clone();
                                    view! {
                                        <button
                                            type="button"
                                            role="option"
                                            aria-selected=move || is_selected().to_string()
                                            data-active=move || is_active().then_some("true")
                                            on:mouseenter=move |_| active.set(Some(hover_value.clone()))
                                            on:click=move |_| pick(opt_value.clone())
                                            class="block w-full whitespace-nowrap px-3 py-1.5 pl-5 text-left text-sm text-[var(--color-ink-muted)] transition-colors data-[active]:bg-[var(--color-bordeaux-soft)]/40 data-[active]:text-[var(--color-ink)] aria-selected:text-[var(--color-accent)]"
                                        >
                                            {label}
                                            {note
                                                .map(|n| {
                                                    view! {
                                                        <span class="ml-2 text-xs text-[var(--color-ink-subtle)]">
                                                            {n}
                                                        </span>
                                                    }
                                                })}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </For>
                    </Show>
                </div>
            </div>
        </div>
    }
}

fn reveal_active(list_ref: NodeRef<html::Div>) {
    #[cfg(feature = "hydrate")]
    if let Some(item) = list_ref
        .get_untracked()
        .and_then(|list| list.query_selector("[data-active]").ok().flatten())
    {
        let options = web_sys::ScrollIntoViewOptions::new();
        options.set_block(web_sys::ScrollLogicalPosition::Nearest);
        item.scroll_into_view_with_scroll_into_view_options(&options);
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = list_ref;
}
