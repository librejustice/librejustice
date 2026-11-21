//! What the text a decision shows is worth, when it is not the authentic one.

use leptos::prelude::*;
use lj_dtos::{DecisionDetail, TextStatus};

use crate::i18n::{use_ui, DecisionUi};

fn status_label(status: TextStatus, ui: &DecisionUi) -> Option<&'static str> {
    match status {
        TextStatus::Authentic => None,
        TextStatus::Official => Some(ui.official_translation),
        TextStatus::Translation => Some(ui.translation),
        TextStatus::Automatic => Some(ui.automatic_translation),
    }
}

/// The text shown when it is not the one read by default, its language.
pub fn chosen_text(detail: &DecisionDetail) -> Option<&str> {
    let lang = detail.lang.as_deref()?;
    (detail.texts.first().map(|t| t.lang.as_str()) != Some(lang)).then_some(lang)
}

/// The status of the text shown when it is not authentic, and the language of the one that is.
#[component]
pub fn DecisionTextStatus(detail: DecisionDetail) -> impl IntoView {
    let shown = detail
        .texts
        .iter()
        .find(|t| Some(&t.lang) == detail.lang.as_ref());
    let ui = use_ui();
    let Some(status) = shown.and_then(|t| status_label(t.status, &ui.decision)) else {
        return ().into_any();
    };
    let authentic = detail
        .texts
        .iter()
        .find(|t| t.status == TextStatus::Authentic)
        .and_then(|t| (ui.lang_name)(&t.lang))
        .map(ui.decision.authentic_in);
    view! {
        <p class="text-sm text-[var(--color-ink-muted)]">{status} {authentic} "."</p>
    }
    .into_any()
}
