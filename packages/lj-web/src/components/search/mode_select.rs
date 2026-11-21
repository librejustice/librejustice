//! `ModeSelect`: the requested search mode, under the label of the resolved one.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;
use lj_dtos::QueryMode;

use super::compact_search::{query_state, search_state};
use crate::i18n::{use_ui, SearchUi};

/// One pill: URL value, label, tooltip.
pub type ModeChoice = (&'static str, &'static str, &'static str);

fn auto(ui: &'static SearchUi) -> ModeChoice {
    ("auto", ui.mode_auto, ui.mode_auto_help)
}

fn lexical(ui: &'static SearchUi) -> ModeChoice {
    ("lexical", ui.mode_lexical, ui.mode_lexical_help)
}

pub fn decision_modes(ui: &'static SearchUi) -> Vec<ModeChoice> {
    vec![
        auto(ui),
        lexical(ui),
        ("semantic", ui.mode_semantic, ui.mode_semantic_help),
        (
            "reference",
            ui.mode_reference,
            ui.mode_reference_decision_help,
        ),
    ]
}

/// No semantic pill: the textes engine has no vector leg, and a position that
/// resolves to nothing is a promise the engine cannot keep.
pub fn texts_modes(ui: &'static SearchUi) -> Vec<ModeChoice> {
    vec![
        auto(ui),
        lexical(ui),
        (
            "reference",
            ui.mode_reference,
            ui.mode_reference_article_help,
        ),
    ]
}

/// Browse is not one of the four: without a query, no pill greys.
fn resolved_value(mode: Option<QueryMode>) -> Option<&'static str> {
    match mode? {
        QueryMode::Lexical => Some("lexical"),
        QueryMode::Hybrid => Some("semantic"),
        QueryMode::Reference => Some("reference"),
        QueryMode::Browse => None,
    }
}

#[component]
pub fn ModeSelect(
    #[prop(into)] resolved: Signal<Option<QueryMode>>,
    modes: Vec<ModeChoice>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let query_map = use_query_map();
    let navigate = use_navigate();

    let current =
        Signal::derive(move || search_state::parse_mode(query_map.get().get("mode").as_deref()));

    let select = move |value: &'static str| {
        let value = (value != "auto").then_some(value);
        let next = query_state::with_param(&query_map.get_untracked(), "mode", value, true);
        navigate(
            &query_state::search_href(&next),
            NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    };

    let group = crate::helpers::cn([
        "flex items-center gap-0.5 rounded border border-[var(--color-rule)] p-0.5",
        &class,
    ]);
    view! {
        <div role="group" aria-label=use_ui().search.mode_group class=group>
            {modes
                .into_iter()
                .map(|(value, label, aide)| {
                    let selected = Signal::derive(move || current.get() == value);
                    let echo = Signal::derive(move || {
                        current.get() == "auto" && resolved_value(resolved.get()) == Some(value)
                    });
                    let select = select.clone();
                    view! {
                        <ModePill
                            label=label
                            aide=aide
                            selected=selected
                            echo=echo
                            on_select=Callback::new(move |_| select(value))
                        />
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// Three states: requested (accent), what `auto` settled on (grey, it informs
/// without claiming to have been clicked), neither.
#[component]
fn ModePill(
    label: &'static str,
    aide: &'static str,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] echo: Signal<bool>,
    #[prop(into)] on_select: Callback<()>,
) -> impl IntoView {
    let class = move || {
        crate::helpers::cn([
            "flex-1 whitespace-nowrap rounded px-1.5 py-0.5 text-center text-xs font-medium transition-colors",
            if selected.get() {
                "bg-[var(--color-accent)] text-[var(--color-accent-foreground)]"
            } else if echo.get() {
                "bg-[var(--color-rule)]/60 text-[var(--color-ink-subtle)]"
            } else {
                "text-[var(--color-ink-subtle)] hover:text-[var(--color-ink)]"
            },
        ])
    };
    let auto_chose_this = use_ui().search.auto_chose_this;
    let title = move || {
        if echo.get() {
            auto_chose_this(aide)
        } else {
            aide.to_string()
        }
    };
    view! {
        <button
            type="button"
            aria-pressed=move || selected.get().to_string()
            title=title
            on:click=move |_| on_select.run(())
            class=class
        >
            {label}
        </button>
    }
}
