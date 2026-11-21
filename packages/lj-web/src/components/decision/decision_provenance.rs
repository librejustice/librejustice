//! A decision's « Source » block. Its rows come from [`lj_dtos::provenance_rows`],
//! which the PDF export reads too.
use crate::components::ui::{Look, ProvenanceBlock};
use leptos::prelude::*;
use lj_dtos::{DecisionDetail, ProvenanceField};

#[component]
pub fn DecisionProvenance(detail: DecisionDetail) -> impl IntoView {
    let source_href = detail
        .source
        .as_deref()
        .and_then(lj_dtos::source_anchor)
        .map(|anchor| format!("/sources#{anchor}"));
    let ui = &crate::i18n::use_ui().provenance;
    let rows = lj_dtos::provenance_rows(&detail)
        .into_iter()
        .map(|(field, value)| match field {
            ProvenanceField::Source => (ui.source, value, Look::Source),
            ProvenanceField::Ecli => (ui.ecli, value, Look::Code),
            ProvenanceField::OfficialPage => (ui.official_page, value, Look::Text),
            ProvenanceField::Permalink => (ui.permalink, value, Look::Text),
        })
        .collect();
    view! { <ProvenanceBlock rows=rows source_href=source_href /> }
}
