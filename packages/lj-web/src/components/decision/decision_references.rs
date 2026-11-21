//! The decision's legal references: one row per cited text, its articles under
//! it as chips.
use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{DecisionDetail, LegalReference};

use crate::components::hover_preview::{HoverPreview, PreviewKind};

#[component]
pub fn DecisionReferences(
    detail: DecisionDetail,
    /// Appended to the root: the page renders the block twice, once per layout.
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    let references = detail
        .legal_references
        .filter(|r| !r.is_empty())
        .unwrap_or_default();
    if references.is_empty() {
        return ().into_any();
    }
    // The decision's date reaches the article hover cards: the version in force on that day.
    let ui = &crate::i18n::use_ui().decision;
    let at_date = detail.date_lecture;
    let place = detail.court_place;
    let rows = references
        .into_iter()
        .map(|r| view! { <LegalRefRow reference=r at_date=at_date.clone() place=place.clone() /> })
        .collect_view();
    view! {
        <section
            aria-label=ui.legal_references
            class=format!("flex flex-col gap-3 print:hidden {class}")
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">{ui.legal_references}</h2>
            <ul class="flex flex-col gap-3">{rows}</ul>
        </section>
    }
    .into_any()
}

#[component]
fn LegalRefRow(
    reference: LegalReference,
    /// The host decision's date, handed to the article hover cards.
    #[prop(optional_no_strip)]
    at_date: Option<String>,
    /// The court's place, which the links carry as the body's do.
    #[prop(optional_no_strip)]
    place: Option<String>,
) -> impl IntoView {
    let LegalReference {
        instrument,
        slug,
        jurisdiction,
        articles,
    } = reference;
    // Each article becomes a `/norm/{juris}/{slug}/{numKey}/{at_date}`
    // link once the citation is **resolved** against the catalogue: the
    // `legal_text` slug plus the canonical `numKey`, both set on the DTO.
    // Otherwise plain text.
    let text = slug
        .as_deref()
        .map(|s| lj_vocab::norm_path(&jurisdiction, s));
    let segment = lj_vocab::url_segment(&jurisdiction);
    let suffix = crate::pages::law_page::data::reader_suffix(place.as_deref());
    let articles_view = (!articles.is_empty()).then(|| {
        let items = articles
            .into_iter()
            .map(|article| {
                let target = text
                    .as_ref()
                    .zip(slug.as_ref())
                    .filter(|_| !article.num_key.is_empty())
                    .map(|(path, s)| {
                        let href = match at_date.as_deref() {
                            Some(d) => format!("{path}/{}/{d}{suffix}", article.num_key),
                            None => format!("{path}/{}{suffix}", article.num_key),
                        };
                        (
                            href,
                            PreviewKind::Article {
                                juris: segment.clone(),
                                code: s.to_string(),
                                num: article.num_key.clone(),
                                date: at_date.clone(),
                                place: place.clone(),
                            },
                        )
                    });
                let num = article.num;
                match target {
                    Some((href, kind)) => view! {
                        <HoverPreview kind=kind>
                            <A
                                href=href
                                attr:class="inline-block whitespace-nowrap rounded-sm border border-[var(--color-rule)] px-1.5 text-xs leading-5 text-[var(--color-ink-muted)] no-underline transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                            >
                                {num}
                            </A>
                        </HoverPreview>
                    }
                    .into_any(),
                    None => view! {
                        <span class="inline-block whitespace-nowrap rounded-sm border border-[var(--color-rule)] px-1.5 text-xs leading-5 text-[var(--color-ink-subtle)]">
                            {num}
                        </span>
                    }
                    .into_any(),
                }
            })
            .collect_view();
        view! { <div class="flex flex-wrap gap-1">{items}</div> }
    });
    // The instrument name links to the whole text as soon as the citation
    // resolves against the catalogue, the same doctrine as the body, where a
    // bare mention leads to the summary.
    // Two lines at most: an arrêté's title runs to fifteen at the rail's 240 px.
    let title = instrument.clone();
    let instrument_view = match text {
        Some(path) => view! {
            <A
                href=format!("{path}{suffix}")
                attr:title=title
                attr:class="line-clamp-2 text-[var(--color-ink)] underline decoration-[var(--color-rule)] underline-offset-4 transition-colors hover:text-[var(--color-accent)] hover:decoration-current"
            >
                {instrument}
            </A>
        }
        .into_any(),
        None => view! {
            <span title=title class="line-clamp-2 text-[var(--color-ink)]">
                {instrument}
            </span>
        }
        .into_any(),
    };
    view! {
        <li class="flex flex-col gap-1.5 text-sm">
            {instrument_view}
            {articles_view}
        </li>
    }
}
