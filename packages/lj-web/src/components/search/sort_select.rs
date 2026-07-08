//! `SortSelect` (port de `sort-select.tsx`). Tri des résultats via `DropdownSelect`.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;

use crate::components::ui::{DropdownSelect, SelectOption};

use super::compact_search::query_state;

fn options() -> Vec<SelectOption> {
    vec![
        SelectOption {
            value: "relevance".into(),
            label: "Pertinence".into(),
        },
        SelectOption {
            value: "date_desc".into(),
            label: "Récents".into(),
        },
        SelectOption {
            value: "date_asc".into(),
            label: "Anciens".into(),
        },
    ]
}

#[component]
pub fn SortSelect() -> impl IntoView {
    let query_map = use_query_map();
    let navigate = use_navigate();

    let current = Signal::derive(move || {
        query_map
            .get()
            .get("sort")
            .unwrap_or_else(|| "relevance".to_string())
    });

    let on_change = Callback::new(move |value: String| {
        // `relevance` (défaut) ⇒ on retire `sort` ; drop `page` ; replace.
        let next = if value == "relevance" {
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
            options=Signal::derive(options)
            aria_label="Trier les résultats"
        />
    }
}
