//! Bloc « Synthèse » : résumé + champs structurés.
use leptos::prelude::*;
use lj_dtos::{DecisionDetail, FacetTag, UiAction};

use crate::components::decision::selection::{SelState, SelectionToolbar};
use crate::components::ui::{shown_summary, SummaryBadge, SummaryFold};
use crate::helpers::{format_iso_date, format_juridiction};
use crate::pages::decision_page::labels::publication_label;

/// A Synthèse field: its label, its value in a monospace font when `mono`, the action a copy counts.
#[derive(Clone)]
struct MetaField {
    label: &'static str,
    value: String,
    mono: bool,
    usage: UiAction,
}

fn build_fields(detail: &DecisionDetail) -> Vec<MetaField> {
    let mut fields: Vec<MetaField> = Vec::new();
    // 1. Juridiction (toujours).
    fields.push(MetaField {
        label: "Juridiction",
        value: detail
            .jurisdiction_name
            .clone()
            .unwrap_or_else(|| format_juridiction(detail.jurisdiction_type).to_string()),
        mono: false,
        usage: UiAction::DecisionMetaCopyJurisdiction,
    });
    // Nom d'usage : c'est par lui que la décision se cite.
    if let Some(nom) = detail.case_name.clone() {
        fields.push(MetaField {
            label: "Nom d'usage",
            value: nom,
            mono: false,
            usage: UiAction::DecisionMetaCopyCaseName,
        });
    }
    // 2. Numéros de dossier (si non vide).
    if let Some(dockets) = detail.docket_numbers.as_ref().filter(|d| !d.is_empty()) {
        fields.push(MetaField {
            label: "Numéros de dossier",
            value: dockets.join(", "),
            mono: true,
            usage: UiAction::DecisionMetaCopyDockets,
        });
    }
    // 3. Date de la décision.
    if let Some(date) = detail.date_lecture.as_deref() {
        fields.push(MetaField {
            label: detail.jurisdiction_type.date_label(),
            value: format_iso_date(Some(date)),
            mono: false,
            usage: UiAction::DecisionMetaCopyDate,
        });
    }
    // 4-7. Tags référentiels résolus par l'API : rendu direct des libellés
    // servis.
    let tags: [(&'static str, &Option<FacetTag>, UiAction); 2] = [
        (
            "Solution",
            &detail.solution,
            UiAction::DecisionMetaCopySolution,
        ),
        (
            "Domaine",
            &detail.legal_domain,
            UiAction::DecisionMetaCopyDomain,
        ),
    ];
    for (label, tag, usage) in tags {
        if let Some(tag) = tag {
            fields.push(MetaField {
                label,
                value: tag.label.clone(),
                mono: false,
                usage,
            });
        }
    }
    // 9. Siège (position de chambre qualifiée).
    if let Some(seat) = detail.seat.clone() {
        fields.push(MetaField {
            label: "Chambre",
            value: seat,
            mono: false,
            usage: UiAction::DecisionMetaCopyChamber,
        });
    }
    if !detail.formation.is_empty() {
        let labels: Vec<&str> = detail.formation.iter().map(|t| t.label.as_str()).collect();
        fields.push(MetaField {
            label: "Formation",
            value: labels.join(", "),
            mono: false,
            usage: UiAction::DecisionMetaCopyFormation,
        });
    }
    // 10. Date d'audience.
    if let Some(date) = detail.date_audience.as_deref() {
        fields.push(MetaField {
            label: "Date d'audience",
            value: format_iso_date(Some(date)),
            mono: false,
            usage: UiAction::DecisionMetaCopyHearingDate,
        });
    }
    // 11. Publication + portée : lecture normalisée des codes au rang le plus
    // fort ; indéterminée = pas de champ.
    if let Some(pub_label) = publication_label(&detail.publication_codes) {
        fields.push(MetaField {
            label: "Publication",
            value: pub_label,
            mono: false,
            usage: UiAction::DecisionMetaCopyPublication,
        });
    }
    // 12. Nomenclature (nac, code brut).
    if let Some(nac) = detail.nac.clone() {
        fields.push(MetaField {
            label: "Nomenclature",
            value: nac,
            mono: true,
            usage: UiAction::DecisionMetaCopyNomenclature,
        });
    }
    fields
}

/// Past this length a summary folds.
const SUMMARY_FOLD_AT: usize = 1_500;

#[component]
pub fn DecisionMeta(
    detail: DecisionDetail,
    #[prop(optional, into)] section_id: Option<String>,
) -> impl IntoView {
    let section_id = section_id.unwrap_or_else(|| "synthese".to_string());
    let fields = build_fields(&detail);

    // Résumé garanti en base et porté par le détail : rendu direct, aucun fetch.
    // Absent (cas résiduel) ⇒ bloc masqué.
    let summary_ref = NodeRef::<leptos::html::P>::new();
    let selection_state: RwSignal<Option<SelState>> = RwSignal::new(None);
    #[cfg(feature = "hydrate")]
    crate::components::decision::selection::watch_selection(summary_ref, selection_state);
    let shown = detail.summary.as_deref().map(|summary| {
        shown_summary(
            detail.jurisdiction_type,
            summary,
            detail.summary_quote.as_ref(),
            detail.summary_body.as_ref(),
        )
    });
    let summary_badge = shown.as_ref().map(|s| {
        view! { <SummaryBadge jurisdiction_type=detail.jurisdiction_type court=s.court /> }
    });
    let summary_view = shown.map(|shown| {
        view! {
            <SummaryFold
                text=shown.text
                court=shown.court
                node_ref=summary_ref
                fold_at=SUMMARY_FOLD_AT
                clamp="line-clamp-12"
                class="mt-3 whitespace-pre-line text-sm leading-relaxed text-[var(--color-ink-muted)]"
                unfold="Lire le résumé entier"
            />
        }
    });

    let fields_view = fields
        .into_iter()
        .map(|f| view! { <Field label=f.label value=f.value mono=f.mono usage=f.usage /> })
        .collect_view();

    view! {
        // Never printed: the sheet goes into a case file, where only the court's own text counts.
        <section
            aria-label="Synthèse"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6 print:hidden"
        >
            <div id=section_id class="relative -top-24" aria-hidden="true" />
            <div class="flex flex-wrap items-center gap-3">
                <h2 class="font-sans text-base text-[var(--color-ink)]">"Synthèse"</h2>
                {summary_badge}
            </div>
            {summary_view}
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">{fields_view}</dl>
        </section>
        <Show when=move || selection_state.get().is_some()>
            {move || {
                let state = selection_state.get().expect("présence garantie par `when`");
                view! {
                    <SelectionToolbar state=state reference=None selection_state=selection_state />
                }
            }}
        </Show>
    }
}

#[component]
fn Field(label: &'static str, value: String, mono: bool, usage: UiAction) -> impl IntoView {
    let dd_class = if mono {
        "font-mono text-sm text-[var(--color-ink)]"
    } else {
        "text-sm text-[var(--color-ink)]"
    };
    view! {
        <div class="flex flex-col gap-1" data-usage-copy=usage.as_str()>
            <dt class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {label}
            </dt>
            <dd class=dd_class>{value}</dd>
        </div>
    }
}
