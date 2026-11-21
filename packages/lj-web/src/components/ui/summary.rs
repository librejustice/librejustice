use std::ops::Range;

use leptos::prelude::*;
use lj_dtos::JurisdictionType;

use super::badge::{badge_classes, BadgeTone};
use crate::i18n::DecisionUi;

/// Names who wrote the summary.
#[component]
pub fn SummaryBadge(
    jurisdiction_type: JurisdictionType,
    court: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let (label, title) = author(&crate::i18n::use_ui().decision, jurisdiction_type, court);
    view! {
        <span
            class=badge_classes(BadgeTone::Outline, &format!("cursor-help {class}"))
            title=title
        >
            {label}
        </span>
    }
}

/// The label and the tooltip of the badge: each court publishes under its own name.
fn author(
    ui: &'static DecisionUi,
    jurisdiction_type: JurisdictionType,
    court: bool,
) -> (&'static str, &'static str) {
    if !court {
        return (ui.ai_summary, ui.ai_summary_title);
    }
    match jurisdiction_type {
        JurisdictionType::Ce
        | JurisdictionType::Caa
        | JurisdictionType::Ta
        | JurisdictionType::Tc => (ui.analysis, ui.analysis_title),
        JurisdictionType::Cedh => (ui.legal_summary, ui.legal_summary_title),
        _ => (ui.headnote, ui.headnote_title),
    }
}

/// Past this many characters a summary shows its head, and a « … » unfolds the rest in place.
pub const SUMMARY_FOLD_AT: usize = 700;

/// A fold hides at least this many words, or the summary shows whole.
const SUMMARY_FOLD_MIN_HIDDEN_WORDS: usize = 20;

/// The words of `text` within `max` characters, `None` when it fits or when fewer than
/// [`SUMMARY_FOLD_MIN_HIDDEN_WORDS`] words would be hidden.
fn folded_head(text: &str, max: usize) -> Option<String> {
    let cut = text.char_indices().nth(max)?.0;
    let head = text[..cut]
        .rfind(char::is_whitespace)
        .map_or(&text[..cut], |at| &text[..at]);
    let hidden = text[head.len()..].split_whitespace().count();
    (hidden >= SUMMARY_FOLD_MIN_HIDDEN_WORDS)
        .then(|| head.trim_end_matches([',', ';', ':', ' ']).to_string())
}

/// A summary past [`SUMMARY_FOLD_AT`] characters, folded on its head.
#[component]
pub fn SummaryFold(
    text: String,
    class: &'static str,
    #[prop(optional)] node_ref: NodeRef<leptos::html::P>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let head = folded_head(&text, SUMMARY_FOLD_AT);
    let folds = head.is_some();
    let open = RwSignal::new(false);
    let read_whole = crate::i18n::use_ui().decision.read_whole_summary;
    let shown = move || match (&head, open.get()) {
        (Some(head), false) => head.clone(),
        _ => text.clone(),
    };
    let more = move || {
        (folds && !open.get()).then(|| {
            view! {
                " "
                <button
                    type="button"
                    aria-expanded="false"
                    aria-label=read_whole
                    title=read_whole
                    on:click=move |_| open.set(true)
                    class="cursor-pointer font-semibold text-[var(--color-accent)] transition-colors hover:text-[var(--color-ink)]"
                >
                    "…"
                </button>
            }
        })
    };
    view! {
        <p node_ref=node_ref class=class>
            {children.map(|c| c())}
            {shown}
            {more}
        </p>
    }
}

/// A summary as a page shows it, and whether the court wrote it.
#[derive(Clone)]
pub struct ShownSummary {
    pub text: String,
    pub court: bool,
}

/// A court's summary is shown as its words alone. The Cour de cassation and the Conseil
/// d'Etat open theirs on the rule, so the model's is shown there without its opening.
pub fn shown_summary(
    jurisdiction_type: JurisdictionType,
    text: &str,
    quote: Option<&Range<usize>>,
    body: Option<&Range<usize>>,
) -> ShownSummary {
    let supreme = matches!(
        jurisdiction_type,
        JurisdictionType::Cc | JurisdictionType::Ce
    );
    match (quote, body) {
        (Some(quote), _) => ShownSummary {
            text: lj_dtos::court_words(text, quote),
            court: true,
        },
        (None, Some(body)) if supreme => ShownSummary {
            text: text.chars().skip(body.start).collect(),
            court: false,
        },
        _ => ShownSummary {
            text: text.to_owned(),
            court: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_folds_on_its_last_whole_word_and_only_to_hide_enough_words() {
        let tail = " mot".repeat(SUMMARY_FOLD_MIN_HIDDEN_WORDS - 3);
        let text = format!("La faute grave, prive de préavis.{tail}");
        assert_eq!(folded_head("La faute grave prive de préavis.", 40), None);
        assert_eq!(folded_head(&text, 17).as_deref(), Some("La faute grave"));
        assert_eq!(folded_head(&text, 22), None);
    }

    #[test]
    fn a_court_summary_is_shown_as_its_words_and_a_supreme_court_s_without_its_opening() {
        let text = "Un salarié conteste son licenciement. La faute grave prive de préavis. Rejet.";
        let shown = |jt, quote: Option<Range<usize>>, body: Option<Range<usize>>| {
            let s = shown_summary(jt, text, quote.as_ref(), body.as_ref());
            (s.text, s.court)
        };
        assert_eq!(
            shown(JurisdictionType::Cc, None, Some(38..70)),
            ("La faute grave prive de préavis. Rejet.".into(), false)
        );
        assert_eq!(
            shown(JurisdictionType::Ce, Some(38..70), Some(38..70)),
            ("La faute grave prive de préavis.".into(), true)
        );
        assert_eq!(
            shown(JurisdictionType::Ca, None, Some(38..70)),
            (text.into(), false)
        );
        assert_eq!(
            shown(JurisdictionType::Cjue, Some(0..77), Some(0..77)),
            (text.into(), true)
        );
    }
}
