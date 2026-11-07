//! Corps structuré de la décision (sections + paragraphes) + toolbar flottante
//! de sélection de texte.
use std::ops::Range;

use leptos::prelude::*;
use lj_dtos::DecisionDetail;
use lj_textsearch::highlight::Highlight;

use crate::components::decision::selection::{SelState, SelectionToolbar};
use crate::components::decision_bar::use_decision_bar;
use crate::components::search::compact_search::highlight::CitedParagraph;
use crate::pages::decision_page::reference::build_decision_references;
use crate::pages::decision_page::sections::RenderSection;

/// What the search matched in each paragraph of a section, in codepoints.
pub type SectionHits = Vec<Vec<Range<usize>>>;

/// The marks of the search the decision was opened from, section by section.
/// They come after mount, on the client alone: the server renders no mark, and
/// hydration finds the same tree before the marks come.
pub fn use_search_hits(
    detail: &DecisionDetail,
    sections: &[RenderSection],
) -> Memo<Vec<SectionHits>> {
    let path = lj_dtos::decision_path(
        detail.jurisdiction_type,
        detail.jurisdiction_code.as_deref(),
        &detail.id,
    );
    let bar = use_decision_bar();
    let query = Memo::new(move |_| {
        bar.with(|b| {
            b.as_ref()
                .filter(|b| b.path == path)
                .and_then(|b| b.from_search.as_ref()?.highlight.clone())
        })
    });
    let texts: Vec<Vec<String>> = sections.iter().map(|s| s.paragraphs.clone()).collect();
    Memo::new(move |_| {
        let Some(q) = query.get() else {
            return Vec::new();
        };
        let highlight = Highlight::parse(&q);
        texts
            .iter()
            .map(|section| section.iter().map(|p| highlight.marks(p)).collect())
            .collect()
    })
}

#[component]
pub fn DecisionBody(
    detail: DecisionDetail,
    sections: Vec<RenderSection>,
    hits: Memo<Vec<SectionHits>>,
) -> impl IntoView {
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
    // version en vigueur à la date de la décision.
    let at_date = detail.date_lecture.clone();
    let article_ref = NodeRef::<leptos::html::Article>::new();
    let selection_state: RwSignal<Option<SelState>> = RwSignal::new(None);

    #[cfg(feature = "hydrate")]
    crate::components::decision::selection::watch_selection(article_ref, selection_state);

    let sections = StoredValue::new(sections);
    let body = move || {
        let mut hits = hits.get().into_iter();
        sections.with_value(|sections| {
            sections
                .iter()
                .map(|section| {
                    let at_date = at_date.clone();
                    let mut paragraph_hits = hits.next().unwrap_or_default().into_iter();
                    let paragraphs_view = section
                        .paragraphs
                        .iter()
                        .zip(&section.paragraph_spans)
                        .map(|(p, spans)| {
                            view! {
                                <CitedParagraph
                                    text=p.clone()
                                    spans=spans.clone()
                                    at_date=at_date.clone()
                                    hits=paragraph_hits.next().unwrap_or_default()
                                />
                            }
                        })
                        .collect_view();
                    // One wordless break, on the first moyens section alone: the court
                    // heads its own later branches.
                    let rule = (section.id == "moyens").then(|| {
                        view! {
                            <div class="mt-10 mb-6 h-px bg-[var(--color-rule)]" aria-hidden="true" />
                        }
                    });
                    // Cible d'ancre : `scroll-mt-20` (scroll-margin-top 80px) décale
                    // l'atterrissage du jump natif #hash sous la top-bar sticky.
                    view! {
                        <section aria-label=section.title.clone()>
                            <div id=section.id.clone() class="scroll-mt-20" aria-hidden="true" />
                            {rule}
                            {paragraphs_view}
                        </section>
                    }
                })
                .collect_view()
        })
    };

    view! {
        <article node_ref=article_ref class="prose-decision">
            {body}
        </article>
        <Show when=move || selection_state.get().is_some()>
            {move || {
                let state = selection_state.get().expect("présence garantie par `when`");
                view! {
                    <SelectionToolbar
                        state=state
                        reference=Some(references.get_value().full)
                        selection_state=selection_state
                    />
                }
            }}
        </Show>
    }
    .into_any()
}
