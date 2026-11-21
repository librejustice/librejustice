//! Cap on the submitted query, said before it applies.
//!
//! No `maxlength` on the input: the browser drops the tail of a paste with no
//! event, leaving the page no way to say it happened. The text stays whole in
//! the field, the notice names both counts, and the submit cuts at the cap.

use leptos::prelude::*;

use lj_dtos::QUERY_MAX_CHARS;

/// Cuts on a character boundary, never mid-codepoint.
pub fn truncate_to_cap(query: &str) -> String {
    if query.chars().count() <= QUERY_MAX_CHARS {
        return query.to_string();
    }
    query.chars().take(QUERY_MAX_CHARS).collect()
}

/// Renders nothing under the cap, which is every ordinary search.
#[component]
pub fn QueryLengthNotice(query: Signal<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let over = Signal::derive(move || {
        let n = query.get().chars().count();
        (n > QUERY_MAX_CHARS).then_some(n)
    });
    view! {
        <Show when=move || over.get().is_some()>
            <p role="status" class="text-xs text-[var(--color-ink-muted)]">
                {move || {
                    let n = over.get().unwrap_or_default();
                    (ui.query_too_long)(n, QUERY_MAX_CHARS)
                }}
            </p>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_whole_characters() {
        let accented = "é".repeat(QUERY_MAX_CHARS + 10);
        let cut = truncate_to_cap(&accented);
        assert_eq!(cut.chars().count(), QUERY_MAX_CHARS);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn an_ordinary_query_is_untouched() {
        assert_eq!(truncate_to_cap("bail commercial"), "bail commercial");
    }
}
