//! `SortSelect` : tri des résultats via `DropdownSelect`.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;

use crate::components::ui::{DropdownSelect, SelectOption};
use crate::i18n::{use_ui, SearchUi};

use super::compact_search::query_state;

fn options(ui: &SearchUi) -> Vec<SelectOption> {
    vec![
        SelectOption {
            value: "relevance".into(),
            label: ui.sort_relevance.into(),
        },
        SelectOption {
            value: "date_desc".into(),
            label: ui.sort_newest.into(),
        },
        SelectOption {
            value: "date_asc".into(),
            label: ui.sort_oldest.into(),
        },
    ]
}

#[component]
pub fn SortSelect() -> impl IntoView {
    let ui = &use_ui().search;
    let query_map = use_query_map();
    let navigate = use_navigate();

    // Without a question `order_by` serves date descending whatever the sort
    // asked for, so « Pertinence » would name an order nothing produces.
    let sans_question = Signal::derive(move || {
        query_map
            .get()
            .get("q")
            .map(|q| q.trim().is_empty())
            .unwrap_or(true)
    });
    let options = Signal::derive(move || {
        let toutes = options(ui);
        match sans_question.get() {
            true => toutes
                .into_iter()
                .filter(|o| o.value != "relevance")
                .collect(),
            false => toutes,
        }
    });

    let current = Signal::derive(move || {
        let demande = query_map
            .get()
            .get("sort")
            .unwrap_or_else(|| "relevance".to_string());
        match sans_question.get() && demande == "relevance" {
            true => "date_desc".to_string(),
            false => demande,
        }
    });

    let on_change = Callback::new(move |value: String| {
        // The default sort never reaches the URL: `relevance` with a question,
        // `date_desc` without one, both being what the server serves untold.
        let defaut =
            value == "relevance" || (sans_question.get_untracked() && value == "date_desc");
        let next = if defaut {
            query_state::with_param(&query_map.get_untracked(), "sort", None, true)
        } else {
            query_state::with_param(&query_map.get_untracked(), "sort", Some(&value), true)
        };
        navigate(
            &query_state::search_href(&next),
            NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    });

    view! {
        <DropdownSelect
            value=current
            on_change=on_change
            options=options
            aria_label=ui.sort_label
        />
    }
}
