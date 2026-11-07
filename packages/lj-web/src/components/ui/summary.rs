use std::ops::Range;

use leptos::prelude::*;
use lj_dtos::JurisdictionType;

use super::badge::{badge_classes, BadgeTone};

/// Names who wrote the summary, its dot in the colour [`SummaryFold`] gives that author's text.
#[component]
pub fn SummaryBadge(
    jurisdiction_type: JurisdictionType,
    court: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let (label, title) = author(jurisdiction_type, court);
    let tone = if court {
        "text-[var(--color-ink)]"
    } else {
        "text-[var(--color-ink-muted)]"
    };
    view! {
        <span
            class=badge_classes(BadgeTone::Outline, &format!("cursor-help {class}"))
            title=title
        >
            <span aria-hidden="true" class=format!("h-1.5 w-1.5 rounded-full bg-current {tone}")></span>
            <span class=tone>{label}</span>
        </span>
    }
}

/// The label and the tooltip of the badge: each court publishes under its own name.
fn author(jurisdiction_type: JurisdictionType, court: bool) -> (&'static str, &'static str) {
    if !court {
        return ("Résumé IA", "Rédigé par une IA à partir de la décision.");
    }
    match jurisdiction_type {
        JurisdictionType::Ce
        | JurisdictionType::Caa
        | JurisdictionType::Ta
        | JurisdictionType::Tc => ("Analyse", "Publiée par la juridiction."),
        JurisdictionType::Cedh => ("Résumé juridique", "Rédigé par le greffe de la Cour."),
        _ => ("Sommaire", "Publié par la juridiction."),
    }
}

/// Past `fold_at` characters, a summary opens on the lines `clamp` keeps, the rest a click away.
/// The court's words read in the body colour, the model's stay muted.
#[component]
pub fn SummaryFold(
    text: String,
    court: bool,
    fold_at: usize,
    clamp: &'static str,
    class: &'static str,
    unfold: &'static str,
    #[prop(optional)] node_ref: NodeRef<leptos::html::P>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let folds = text.chars().count() > fold_at;
    let open = RwSignal::new(false);
    let shown_class = move || {
        if folds && !open.get() {
            format!("{class} {clamp}")
        } else {
            class.to_string()
        }
    };
    let toggle = folds.then(|| {
        view! {
            <button
                type="button"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
                class="mt-1 cursor-pointer text-xs text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]"
            >
                {move || if open.get() { "Replier le résumé" } else { unfold }}
            </button>
        }
    });
    let text = if court {
        view! { <span class="text-[var(--color-ink)]">{text}</span> }.into_any()
    } else {
        text.into_any()
    };
    view! {
        <p node_ref=node_ref class=shown_class>
            {children.map(|c| c())}
            {text}
        </p>
        {toggle}
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
