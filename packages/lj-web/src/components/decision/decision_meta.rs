//! Bloc « Synthèse » : résumé + champs structurés + références juridiques.
//! Port de `decision-meta.tsx`.
use leptos::prelude::*;
use lj_dtos::{DecisionDetail, FacetTag, LegalReference};

use leptos_router::components::A;

use crate::components::hover_preview::{HoverPreview, PreviewKind};
use crate::helpers::{format_iso_date, format_juridiction};
use crate::pages::decision_page::labels::{publication_label, significance_label};

/// Champ de synthèse (label + valeur, `mono` = police monospace).
#[derive(Clone)]
struct MetaField {
    label: &'static str,
    value: String,
    mono: bool,
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
    });
    // 2. Numéros de dossier (si non vide).
    if let Some(dockets) = detail.docket_numbers.as_ref().filter(|d| !d.is_empty()) {
        fields.push(MetaField {
            label: "Numéros de dossier",
            value: dockets.join(", "),
            mono: true,
        });
    }
    // 3. Date de lecture.
    if let Some(date) = detail.date_lecture.as_deref() {
        fields.push(MetaField {
            label: "Date de lecture",
            value: format_iso_date(Some(date)),
            mono: false,
        });
    }
    // 4-7. Tags référentiels résolus par l'API (ADR 0146) : rendu direct des
    // libellés servis. `voie` absente = procédure ordinaire (pas de champ).
    let tags: [(&'static str, &Option<FacetTag>); 3] = [
        ("Solution", &detail.solution),
        ("Procédure", &detail.procedure),
        ("Domaine", &detail.legal_domain),
    ];
    for (label, tag) in tags {
        if let Some(tag) = tag {
            fields.push(MetaField {
                label,
                value: tag.label.clone(),
                mono: false,
            });
        }
    }
    // 9. Siège (position de chambre qualifiée, axes ADR 0170).
    if let Some(seat) = detail.seat.clone() {
        fields.push(MetaField {
            label: "Formation",
            value: seat,
            mono: false,
        });
    }
    // 10. Date d'audience.
    if let Some(date) = detail.date_audience.as_deref() {
        fields.push(MetaField {
            label: "Date d'audience",
            value: format_iso_date(Some(date)),
            mono: false,
        });
    }
    // 11. Publication + portée (ADR 0167 : lecture normalisée des codes au
    // rang le plus fort ; indéterminée = pas de champ).
    if let Some(pub_label) = publication_label(&detail.publication_codes) {
        fields.push(MetaField {
            label: "Publication",
            value: pub_label,
            mono: false,
        });
    }
    if let Some(significance) = significance_label(&detail.publication_codes) {
        fields.push(MetaField {
            label: "Portée",
            value: significance.to_string(),
            mono: false,
        });
    }
    // 12. Nomenclature (nac, code brut). ADR 0090.
    if let Some(nac) = detail.nac.clone() {
        fields.push(MetaField {
            label: "Nomenclature",
            value: nac,
            mono: true,
        });
    }
    fields
}

#[component]
pub fn DecisionMeta(
    detail: DecisionDetail,
    #[prop(optional, into)] section_id: Option<String>,
) -> impl IntoView {
    let section_id = section_id.unwrap_or_else(|| "synthese".to_string());
    let fields = build_fields(&detail);
    let themes = detail.themes.clone();
    let legal_references = detail
        .legal_references
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_default();

    // Résumé garanti en base et porté par le détail (ADR 0051) : rendu direct,
    // aucun fetch. Absent (cas résiduel) ⇒ bloc masqué.
    let summary_view = detail.summary.clone().map(|text| {
        view! {
            <p class="mt-3 text-sm leading-relaxed text-[var(--color-ink-muted)]">{text}</p>
        }
    });

    let fields_view = fields
        .into_iter()
        .map(|f| view! { <Field label=f.label value=f.value mono=f.mono /> })
        .collect_view();

    // Chips de matière (themes Judilibre, ADR 0090).
    let themes_view = (!themes.is_empty()).then(|| {
        let chips = themes
            .into_iter()
            .map(|theme| {
                view! {
                    <li class="rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-1 text-xs text-[var(--color-ink-muted)]">
                        {theme}
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="mt-5 border-t border-[var(--color-rule)] pt-4">
                <h3 class="font-sans text-sm font-medium text-[var(--color-ink)]">"Matières"</h3>
                <ul class="mt-3 flex flex-wrap gap-2">{chips}</ul>
            </div>
        }
    });

    let refs_view = (!legal_references.is_empty()).then(|| {
        // Date de lecture propagée aux hover cards d'articles : version en
        // vigueur à la date de la décision (ADR 0168).
        let at_date = detail.date_lecture.clone();
        let rows = legal_references
            .into_iter()
            .map(|r| view! { <LegalRefRow reference=r at_date=at_date.clone() /> })
            .collect_view();
        view! {
            <div class="mt-5 border-t border-[var(--color-rule)] pt-4">
                <h3 class="font-sans text-sm font-medium text-[var(--color-ink)]">
                    "Références juridiques"
                </h3>
                <ul class="mt-3 flex flex-col gap-2">{rows}</ul>
            </div>
        }
    });

    view! {
        <section
            aria-label="Synthèse"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6"
        >
            <div id=section_id class="relative -top-24" aria-hidden="true" />
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Synthèse"</h2>
            {summary_view}
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">{fields_view}</dl>
            {themes_view}
            {refs_view}
        </section>
    }
}

#[component]
fn Field(label: &'static str, value: String, mono: bool) -> impl IntoView {
    let dd_class = if mono {
        "font-mono text-sm text-[var(--color-ink)]"
    } else {
        "text-sm text-[var(--color-ink)]"
    };
    view! {
        <div class="flex flex-col gap-1">
            <dt class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {label}
            </dt>
            <dd class=dd_class>{value}</dd>
        </div>
    }
}

#[component]
fn LegalRefRow(
    reference: LegalReference,
    /// Date de la décision hôte, transmise aux hover cards d'articles.
    #[prop(optional_no_strip)]
    at_date: Option<String>,
) -> impl IntoView {
    let LegalReference {
        instrument,
        slug,
        articles,
    } = reference;
    // Chaque article devient un lien `/texte/{slug}/{numKey}` quand la citation est
    // **résolue** au catalogue (FK déterministe à l'ingest, ADR 0123 §2) : `slug`
    // du `legal_text` + `numKey` canonique posés au DTO ; sinon texte brut. Plus de
    // re-slugification côté front. Articles séparés par « , ».
    let articles_view = (!articles.is_empty()).then(|| {
        let last = articles.len() - 1;
        let items = articles
            .into_iter()
            .enumerate()
            .map(|(i, article)| {
                let sep = (i < last).then_some(", ");
                let target = slug.as_ref().filter(|_| !article.num_key.is_empty()).map(
                    |s| {
                        (
                            format!("/texte/{s}/{}", article.num_key),
                            PreviewKind::Article {
                                code: s.to_string(),
                                num: article.num_key.clone(),
                                date: at_date.clone(),
                            },
                        )
                    },
                );
                let num = article.num;
                let body = match target {
                    // Hover card d'article sur le lien (ADR 0168).
                    Some((href, kind)) => view! {
                        <HoverPreview kind=kind>
                            <A href=href attr:class="underline underline-offset-4 hover:text-[var(--color-accent)]">
                                {num}
                            </A>
                        </HoverPreview>
                    }
                    .into_any(),
                    None => view! { <span>{num}</span> }.into_any(),
                };
                view! {
                    {body}
                    {sep}
                }
            })
            .collect_view();
        // `art.&nbsp;` : espace insécable U+00A0 (parité `&nbsp;`).
        view! {
            <span class="text-[var(--color-ink-muted)]">"art.\u{00A0}"{items}</span>
        }
    });
    // Nom de l'instrument cliquable vers `/texte/{slug}` (le texte entier) dès que la
    // citation est résolue au catalogue — même doctrine que le corps, où une
    // mention nue mène à `/texte/{slug}` (decisions.rs, 2026-07-05). Sans ce lien, un
    // texte cité EN BLOC (articles vides) n'ouvrait rien depuis l'encart.
    let instrument_view = match slug {
        Some(s) => view! {
            <A
                href=format!("/texte/{s}")
                attr:class="font-medium text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {instrument}
            </A>
        }
        .into_any(),
        None => view! {
            <span class="font-medium text-[var(--color-ink)]">{instrument}</span>
        }
        .into_any(),
    };
    view! {
        <li class="flex flex-wrap items-baseline gap-2 text-sm">
            {instrument_view}
            {articles_view}
        </li>
    }
}
