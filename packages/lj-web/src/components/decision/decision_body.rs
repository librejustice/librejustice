//! Corps structuré de la décision (sections + paragraphes) + toolbar flottante
//! de sélection de texte. Port de `decision-body.tsx`.
//!
//! La géométrie de la sélection (`window.getSelection`, `Range`, `DomRect`),
//! non exposée par les features web-sys du crate, passe par le shim JS
//! `selection.js` bindé inline (cf. `InfiniteSentinel`). En SSR : article
//! statique, toolbar absente (interop gatée `hydrate`).
use leptos::prelude::*;
use lj_dtos::DecisionDetail;

use crate::components::search::compact_search::highlight::CitedParagraph;
use crate::pages::decision_page::reference::{build_decision_references, DecisionReferenceParts};
use crate::pages::decision_page::sections::RenderSection;

/// Position + texte normalisé de la sélection courante. `None` = pas de toolbar.
#[derive(Debug, Clone, PartialEq)]
struct SelState {
    text: String,
    top: f64,
    left: f64,
}

#[component]
pub fn DecisionBody(detail: DecisionDetail, sections: Vec<RenderSection>) -> impl IntoView {
    let has_paragraphs = sections.iter().any(|s| !s.paragraphs.is_empty());

    if !has_paragraphs {
        return view! {
            <p class="text-sm italic text-[var(--color-ink-subtle)]">
                "Texte intégral non disponible."
            </p>
        }
        .into_any();
    }

    let references = StoredValue::new(build_decision_references(&detail));
    // Date de lecture propagée aux hover cards d'articles : elles servent la
    // version en vigueur à la date de la décision (ADR 0168).
    let at_date = detail.date_lecture.clone();
    let article_ref = NodeRef::<leptos::html::Article>::new();
    let selection_state: RwSignal<Option<SelState>> = RwSignal::new(None);

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        use crate::pages::decision_page::reference::normalize_selection_text;

        // Closure JS recevant (texte, top, left) de l'observateur de sélection.
        type SelectionClosure = Closure<dyn FnMut(JsValue, f64, f64)>;

        #[wasm_bindgen(module = "/src/components/decision/selection.js")]
        extern "C" {
            #[wasm_bindgen(js_name = "observeSelection")]
            fn observe_selection(article: &JsValue, on_change: &SelectionClosure) -> JsValue;
        }

        // L'observateur et son `Closure` tiennent des handles JS (pas `Send`) :
        // gardés dans des slots `LocalStorage`, l'observateur précédent est
        // déconnecté à chaque passage (port du cleanup des 3 listeners).
        let disconnect_slot: StoredValue<Option<JsValue>, leptos::reactive::owner::LocalStorage> =
            StoredValue::new_local(None);
        let closure_slot: StoredValue<
            Option<SelectionClosure>,
            leptos::reactive::owner::LocalStorage,
        > = StoredValue::new_local(None);
        Effect::new(move |_| {
            if let Some(prev) = disconnect_slot.try_update_value(Option::take).flatten() {
                if let Ok(f) = prev.dyn_into::<js_sys::Function>() {
                    let _ = f.call0(&JsValue::NULL);
                }
            }
            let Some(node) = article_ref.get() else {
                return;
            };
            // Normalisation côté Rust (l'oracle normalise avant d'agir) :
            // texte vide après normalisation → None.
            let closure = Closure::new(move |text: JsValue, top: f64, left: f64| {
                let next = text.as_string().and_then(|raw| {
                    let text = normalize_selection_text(&raw);
                    (!text.is_empty()).then_some(SelState { text, top, left })
                });
                selection_state.set(next);
            });
            let node_js: JsValue = node.into();
            let disconnect = observe_selection(&node_js, &closure);
            disconnect_slot.set_value(Some(disconnect));
            closure_slot.set_value(Some(closure));
        });
    }

    let sections_view = sections
        .into_iter()
        .map(|section| {
            let RenderSection {
                id,
                title,
                paragraphs,
                paragraph_spans,
            } = section;
            // Cible d'ancre : `scroll-mt-20` (scroll-margin-top 80px) décale
            // l'atterrissage du jump natif #hash sous la top-bar sticky.
            // Chaque paragraphe est rendu avec ses citations cliquables (overlay
            // composé server-side, ADR 0134) ; spans vides ⇒ texte brut.
            let at_date = at_date.clone();
            let paragraphs_view = paragraphs
                .into_iter()
                .zip(paragraph_spans)
                .map(|(p, spans)| {
                    view! { <CitedParagraph text=p spans=spans at_date=at_date.clone() /> }
                })
                .collect_view();
            view! {
                <section aria-label=title.clone()>
                    <div id=id class="scroll-mt-20" aria-hidden="true" />
                    {paragraphs_view}
                </section>
            }
        })
        .collect_view();

    view! {
        <article node_ref=article_ref class="prose-decision">
            {sections_view}
        </article>
        <Show when=move || selection_state.get().is_some()>
            {move || {
                let state = selection_state.get().expect("présence garantie par `when`");
                view! {
                    <SelectionToolbar
                        state=state
                        references=references.get_value()
                        selection_state=selection_state
                    />
                }
            }}
        </Show>
    }
    .into_any()
}

/// Toolbar flottante (Rechercher / Copier / Copier + référence), positionnée
/// sur le rect de la sélection. Les actions sont gatées `hydrate`.
#[component]
fn SelectionToolbar(
    state: SelState,
    references: DecisionReferenceParts,
    selection_state: RwSignal<Option<SelState>>,
) -> impl IntoView {
    let _ = (&references, selection_state);
    let style = format!("left:{}px;top:{}px", state.left, state.top);

    let on_search = {
        let _text = state.text.clone();
        move |_| {
            #[cfg(feature = "hydrate")]
            {
                use crate::pages::decision_page::reference::flatten_selection_text;
                let flat = flatten_selection_text(&_text);
                let query = String::from(js_sys::encode_uri_component(
                    &flat.chars().take(512).collect::<String>(),
                ));
                let navigate = leptos_router::hooks::use_navigate();
                navigate(
                    &format!("/decisions?q={query}"),
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

    let on_copy_ref = {
        let _text = state.text.clone();
        let _reference = references.full.clone();
        move |_| {
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
        }
    };

    view! {
        <div
            class="fixed z-50 flex -translate-x-1/2 -translate-y-full items-center overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] shadow-lg print:hidden"
            style=style
        >
            <ToolbarButton label="Rechercher" on_click=on_search />
            <ToolbarButton label="Copier" on_click=on_copy />
            <ToolbarButton label="Copier + référence" on_click=on_copy_ref />
        </div>
    }
}

#[component]
fn ToolbarButton(
    label: &'static str,
    on_click: impl FnMut(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            on:click=on_click
            class="border-l border-[var(--color-rule)] px-3 py-2 text-sm text-[var(--color-ink)] first:border-l-0 hover:bg-[var(--color-vellum)]"
        >
            {label}
        </button>
    }
}

/// Vide la sélection courante (port de `removeAllRanges`).
#[cfg(feature = "hydrate")]
fn clear_selection() {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/components/decision/selection.js")]
    extern "C" {
        #[wasm_bindgen(js_name = "clearSelection")]
        fn js_clear_selection();
    }

    js_clear_selection();
}
