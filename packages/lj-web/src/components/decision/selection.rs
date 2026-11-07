//! Text selection: rect tracking and the floating toolbar, shared by the
//! decision body and the summary. The summary offers no « Copier + référence »:
//! a model may have written it, so it is not quoted as the court's words.
use leptos::prelude::*;
use lj_dtos::UiAction;

use crate::components::search::CardViewPref;

/// Position and normalised text of the current selection. `None` = no toolbar.
#[derive(Debug, Clone, PartialEq)]
pub struct SelState {
    pub text: String,
    pub top: f64,
    pub bottom: f64,
    pub center: f64,
}

/// Subscribes `state` to the selection made inside `node`. A selection outside
/// `node` yields `None` (`crate::dom` tests containment), so two hosts
/// subscribed on the same page never show a toolbar at the same time.
///
/// The subscription holds JS handles (not `Send`): it lives in a `LocalStorage`
/// slot, the previous one dropped on each pass, which removes the listeners.
#[cfg(feature = "hydrate")]
pub fn watch_selection<E>(node: NodeRef<E>, state: RwSignal<Option<SelState>>)
where
    E: leptos::html::ElementType + 'static,
    E::Output: wasm_bindgen::JsCast + Clone + Into<web_sys::Element> + 'static,
{
    use crate::pages::decision_page::reference::normalize_selection_text;

    let slot: StoredValue<
        Option<crate::dom::SelectionSubscription>,
        leptos::reactive::owner::LocalStorage,
    > = StoredValue::new_local(None);
    Effect::new(move |_| {
        slot.try_update_value(Option::take);
        let Some(host) = node.get() else {
            return;
        };
        let element: web_sys::Element = host.clone().into();
        let subscription = crate::dom::observe_selection(element, move |mesure| {
            let next = mesure.and_then(|(raw, top, bottom, center)| {
                let text = normalize_selection_text(&raw);
                (!text.is_empty()).then_some(SelState {
                    text,
                    top,
                    bottom,
                    center,
                })
            });
            state.set(next);
        });
        slot.set_value(subscription);
    });
}

/// Floating toolbar placed on the selection rect: Rechercher, Copier, and
/// Copier + référence when `reference` is given.
#[component]
pub fn SelectionToolbar(
    state: SelState,
    /// Full reference of the decision, `None` for a text that is not quotable.
    reference: Option<String>,
    selection_state: RwSignal<Option<SelState>>,
) -> impl IntoView {
    let _ = selection_state;
    let toolbar_ref = NodeRef::<leptos::html::Div>::new();
    let pose: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);

    #[cfg(feature = "hydrate")]
    {
        const MARGIN: f64 = 8.0;
        const GAP: f64 = 12.0;
        /// Height of the sticky top bar, which anchors clear with `scroll-mt-20`.
        const STICKY_BAR: f64 = 80.0;

        let (top, bottom, center) = (state.top, state.bottom, state.center);
        Effect::new(move |_| {
            let Some(node) = toolbar_ref.get() else {
                return;
            };
            let rect = node.get_bounding_client_rect();
            let window = leptos::prelude::window();
            let viewport = |size: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
                size.ok().and_then(|v| v.as_f64()).unwrap_or_default()
            };
            let (vw, vh) = (
                viewport(window.inner_width()),
                viewport(window.inner_height()),
            );
            let left = (center - rect.width() / 2.0)
                .clamp(MARGIN, (vw - rect.width() - MARGIN).max(MARGIN));
            let above = top - GAP - rect.height();
            let placed_top = if above >= STICKY_BAR {
                above
            } else {
                (bottom + GAP).min((vh - rect.height() - MARGIN).max(MARGIN))
            };
            pose.set(Some((left, placed_top)));
        });
    }

    // Tailwind v4 centres through `translate`, which `transform:none` leaves standing.
    let naive = format!(
        "left:{}px;top:{}px",
        state.center,
        (state.top - 12.0).max(16.0)
    );
    let style = move || match pose.get() {
        Some((left, top)) => format!("left:{left}px;top:{top}px;translate:none"),
        None => naive.clone(),
    };

    let on_search = {
        let _text = state.text.clone();
        let _card_view = expect_context::<CardViewPref>();
        move |_| {
            #[cfg(feature = "hydrate")]
            {
                use crate::components::search::CardView;
                use crate::pages::decision_page::reference::flatten_selection_text;
                let flat = flatten_selection_text(&_text);
                // The results page keys the pinned view on the trimmed `q`.
                let q = flat
                    .chars()
                    .take(512)
                    .collect::<String>()
                    .trim_end()
                    .to_string();
                let query = String::from(js_sys::encode_uri_component(&q));
                _card_view.pin(q, CardView::Snippet);
                let navigate = leptos_router::hooks::use_navigate();
                navigate(
                    &format!("/decision-search?q={query}"),
                    leptos_router::NavigateOptions::default(),
                );
                clear_selection();
                selection_state.set(None);
            }
        }
    };

    let on_copy = {
        let _text = state.text.clone();
        move |_| {
            #[cfg(feature = "hydrate")]
            {
                let text = _text.clone();
                leptos::task::spawn_local(async move {
                    crate::dom::copy_text(&text).await;
                    clear_selection();
                    selection_state.set(None);
                });
            }
        }
    };

    let copy_ref_view = reference.map(|_reference| {
        let _text = state.text.clone();
        let on_copy_ref = move |_| {
            #[cfg(feature = "hydrate")]
            {
                use crate::pages::decision_page::reference::format_selection_with_reference;
                let value = format_selection_with_reference(&_text, &_reference);
                leptos::task::spawn_local(async move {
                    crate::dom::copy_text(&value).await;
                    clear_selection();
                    selection_state.set(None);
                });
            }
        };
        view! { <ToolbarButton
            label="Copier + référence"
            usage=UiAction::DecisionSelectionCopyReference
            on_click=on_copy_ref
        /> }
    });

    view! {
        <div
            node_ref=toolbar_ref
            class="fixed z-50 flex w-max -translate-x-1/2 -translate-y-full items-center overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg print:hidden"
            style=style
        >
            <ToolbarButton label="Rechercher" usage=UiAction::DecisionSelectionSearch on_click=on_search />
            <ToolbarButton label="Copier" usage=UiAction::DecisionSelectionCopy on_click=on_copy />
            {copy_ref_view}
        </div>
    }
}

#[component]
fn ToolbarButton(
    label: &'static str,
    usage: UiAction,
    on_click: impl FnMut(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            on:click=on_click
            data-usage=usage.as_str()
            class="border-l border-[var(--color-rule)] px-3 py-2 text-sm text-[var(--color-ink)] first:border-l-0 hover:bg-[var(--color-vellum)]"
        >
            {label}
        </button>
    }
}

#[cfg(feature = "hydrate")]
fn clear_selection() {
    crate::dom::clear_selection();
}
