//! Page `/decision/:id` (SEO-critique).
//!
//! - Détail : `Resource::new_blocking` ⇒ le SSR attend la résolution avant
//!   d'émettre le HTML, donc `<Title>`/`<Meta>`/JSON-LD sont dans le document
//!   initial (crawlables).
//! - Similaires : `Resource` non bloquante + `<Suspense>` ⇒ streamées après le
//!   shell (SsrMode::PartiallyBlocked posé par la substrate dans `app.rs`).
//! - Statut HTTP + `Cache-Control` (SSR) posés sur la réponse document via
//!   `ResponseOptions` (200 → 7 j CDN, 404/400 → 5 min, 5xx → no-store).

pub mod data;
pub mod labels;
pub mod reference;
pub mod sections;
pub mod toc_spy;

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Script, Title};
use lj_dtos::UiAction;

use crate::components::decision::{
    use_search_hits, DecisionActions, DecisionBody, DecisionCitantes, DecisionCommentaires,
    DecisionHeader, DecisionLayout, DecisionMeta, DecisionParties, DecisionProvenance,
    DecisionReferences, DecisionSimilar, DecisionSkeleton, DecisionToc,
};
use crate::components::own_page_description;
use crate::seo::decision::{build_json_ld, meta_description, seo_title};
use crate::seo::{canonical_url, OG_IMAGE};

use data::{
    decision_id, fetch_detail, fetch_parties, fetch_similar, sendable, PageError, PartiesResult,
    SimilarResult,
};
use reference::build_decision_references;
use sections::{resolve_decision_sections, toc_hit_counts, toc_sections};

#[component]
pub fn DecisionPage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) : on la
    // déclare ici, synchroniquement, sinon la générique du shell sort aussi.
    own_page_description();

    let id = decision_id();

    // Détail bloquant (SEO dans le document initial).
    let detail = Resource::new_blocking(move || id.get(), |id| sendable(fetch_detail(id)));
    let similar = Resource::new(move || id.get(), |id| sendable(fetch_similar(id)));
    let parties = Resource::new(move || id.get(), |id| sendable(fetch_parties(id)));

    view! {
        <Suspense fallback=DecisionSkeleton>
            {move || Suspend::new(async move {
                match detail.await {
                    Ok(detail) => {
                        set_cache_control(200, detail.summary.is_some());
                        Either::Left(
                            view! {
                                <DecisionLoaded detail=detail similar=similar parties=parties />
                            },
                        )
                    }
                    Err(err) => {
                        set_cache_control(err.status, false);
                        Either::Right(view! { <DecisionError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// Pose le statut HTTP et le `Cache-Control` de la réponse document SSR. Le
/// détail étant bloquant, tout est posé avant le flush des headers. Une décision
/// inconnue part en 404, pas en 200 avec `noindex`. No-op côté hydrate.
#[cfg(feature = "ssr")]
fn set_cache_control(status: u16, summary_present: bool) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    let value = match status {
        // Résumé manquant (décision pas encore résumée par le cron ou le rerank) : on
        // ne fige pas la page sans synthèse 7 j au CDN. TTL court ⇒ la prochaine
        // requête, une fois le résumé backfillé, met en cache la bonne page.
        200 if !summary_present => "public, max-age=0, s-maxage=300",
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 => "public, max-age=0, s-maxage=300",
        _ => "no-store",
    };
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if status != 200 {
            if let Ok(code) = StatusCode::from_u16(status) {
                resp.set_status(code);
            }
        }
        if let Ok(hv) = HeaderValue::from_str(value) {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn set_cache_control(_status: u16, _summary_present: bool) {}

/// Page d'erreur (id invalide / 404 / autre). Pose `robots noindex`.
#[component]
fn DecisionError(err: PageError) -> impl IntoView {
    if err.status == 404 {
        crate::pages::not_found::signaler_lien_mort();
    }
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    // Détail absent ⇒ noindex + titre « introuvable » (le cas « Chargement… »
    // correspond au fallback Suspense).
    let title = "Décision introuvable - LibreJustice";
    view! {
        <Title text=title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {eyebrow}
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{err.message}</h1>
        </div>
    }
}

/// Page chargée : SEO (title/meta/OG/twitter/canonical/JSON-LD) + layout
/// toc/main/similar — la colonne similaires défile avec la page (non sticky),
/// seul le sommaire est un rail.
#[component]
fn DecisionLoaded(
    detail: lj_dtos::DecisionDetail,
    similar: Resource<SimilarResult>,
    parties: Resource<PartiesResult>,
) -> impl IntoView {
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(
        detail.jurisdiction_type,
        detail.jurisdiction_code.as_deref(),
        &detail.id,
    );
    #[cfg(feature = "hydrate")]
    {
        let document = url.clone();
        Effect::new(move |_| crate::reads::report(lj_dtos::ReadKind::Decision, &document));
    }
    let jsonld = serde_json::to_string(&build_json_ld(&detail, &title, &description))
        .unwrap_or_else(|_| "{}".to_string());
    // Fil d'Ariane structuré vers l'arborescence juridiction — absent si la
    // décision n'a pas de code juridiction résolu.
    let juris = detail
        .jurisdiction_type
        .url_segment(detail.jurisdiction_code.as_deref());
    let breadcrumb_jsonld = detail.jurisdiction_code.as_ref().map(|code| {
        let label = detail
            .jurisdiction_name
            .clone()
            .unwrap_or_else(|| code.clone());
        let mut items = vec![
            ("Accueil", "https://librejustice.fr/".to_string()),
            (
                "Juridictions",
                format!("https://librejustice.fr/decision-index/{juris}"),
            ),
            (
                label.as_str(),
                format!("https://librejustice.fr/decision-index/{juris}/{code}"),
            ),
        ];
        let year = detail
            .date_lecture
            .as_deref()
            .and_then(|d| d.get(..4))
            .map(str::to_string);
        if let Some(y) = &year {
            items.push((
                y.as_str(),
                format!("https://librejustice.fr/decision-index/{juris}/{code}/{y}"),
            ));
        }
        items.push((title.as_str(), url.clone()));
        crate::pages::juridictions_page::breadcrumb_jsonld(&items)
    });

    let page_title = format!("{} - LibreJustice", seo_title(&detail));
    let references_full = build_decision_references(&detail).full;

    let body_sections = resolve_decision_sections(&detail);
    // Décision sans texte intégral (lacune de la source) : la page rend « Texte
    // intégral non disponible » en 200 — noindex pour ne pas faire indexer une
    // coquille comme une vraie décision.
    let has_text = body_sections.iter().any(|s| !s.paragraphs.is_empty());
    let toc = toc_sections(&detail, &body_sections);
    let hits = use_search_hits(&detail, &body_sections);
    let toc_hits = {
        let toc = toc.clone();
        let section_ids: Vec<String> = body_sections.iter().map(|s| s.id.clone()).collect();
        Memo::new(move |_| {
            hits.with(|h| {
                let per_section: Vec<usize> =
                    h.iter().map(|s| s.iter().map(Vec::len).sum()).collect();
                toc_hit_counts(&toc, &section_ids, &per_section)
            })
        })
    };

    let detail_header = detail.clone();
    let detail_meta = detail.clone();
    let detail_body = detail.clone();
    let detail_provenance = detail.clone();
    let detail_references = detail.clone();
    let detail_references_side = detail.clone();

    let toc_mobile = toc.clone();
    let chronology_mobile = detail.chronology.clone();
    let detail_actions = detail.clone();
    let detail_actions_rail = detail.clone();

    // Streamed, hidden when empty. An error stays silent: the block enriches the page, it carries nothing.
    let parties_view = move |class: &'static str| {
        view! {
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    let resolved = parties.await;
                    match resolved.error {
                        Some(_) => ().into_any(),
                        None => {
                            view! { <DecisionParties parties=resolved.parties class=class /> }
                                .into_any()
                        }
                    }
                })}
            </Suspense>
        }
        .into_any()
    };
    // Desktop left rail, sticky as one block. When it outgrows the viewport, the parties scroll on
    // their own, the actions and the sommaire staying in place.
    let toc_view = view! {
        <div class="flex flex-col gap-6 lg:sticky lg:top-20 lg:max-h-[calc(100dvh-6rem)]">
            <div class="flex shrink-0 flex-col gap-6">
                <DecisionActions detail=detail_actions_rail />
                <DecisionToc sections=toc chronology=detail.chronology.clone() hits=toc_hits />
            </div>
            <div class="flex min-h-0 flex-col gap-6 overflow-y-auto">
                <div class="contents" data-usage=UiAction::DecisionParties.as_str()>
                    {parties_view("")}
                </div>
                // Pinned to the bottom of the visible area, it fades what scrolls on; past
                // the end of the content it covers blank space only.
                <div
                    aria-hidden="true"
                    class="pointer-events-none sticky bottom-0 -mt-6 h-8 shrink-0 bg-linear-to-b from-transparent to-[var(--color-background)]"
                />
            </div>
        </div>
    }
    .into_any();

    let main_view = view! {
        <article class="flex flex-col gap-10">
            <h1 class="sr-only lj-doc-title">{references_full}</h1>
            <DecisionHeader detail=detail_header />
            <div class="contents" data-usage=UiAction::DecisionMeta.as_str()>
                <DecisionMeta detail=detail_meta section_id="synthese" />
            </div>
            // Below lg the side columns go: the actions and the sommaire join the reading flow
            // before the text, the references and the parties after its source.
            <div class="flex flex-col gap-6 lg:hidden print:hidden">
                <DecisionActions detail=detail_actions />
                <DecisionToc sections=toc_mobile chronology=chronology_mobile hits=toc_hits />
            </div>
            <div class="contents" data-usage=UiAction::DecisionText.as_str()>
                <DecisionBody detail=detail_body sections=body_sections hits=hits />
            </div>
            <div class="contents" data-usage=UiAction::DecisionCommentaries.as_str()>
                <DecisionCommentaires commentaires=detail.commentaires.clone() />
            </div>
            <div class="contents" data-usage=UiAction::DecisionProvenance.as_str()>
                <DecisionProvenance detail=detail_provenance />
            </div>
            <div class="contents" data-usage=UiAction::DecisionReferences.as_str()>
                <DecisionReferences detail=detail_references class="lg:hidden" />
            </div>
            <div class="contents" data-usage=UiAction::DecisionParties.as_str()>
                {parties_view("lg:hidden")}
            </div>
            <div class="contents" data-usage=UiAction::DecisionCiting.as_str()>
                <DecisionCitantes
                    citantes=detail.citantes.clone()
                    total=detail.citantes_total
                />
            </div>
        </article>
    }
    .into_any();
    // Desktop right column: the legal references above the neighbours.
    let has_references = detail
        .legal_references
        .as_ref()
        .is_some_and(|r| !r.is_empty());
    let references_side = has_references.then(|| {
        view! {
            <div class="hidden lg:block" data-usage=UiAction::DecisionReferences.as_str()>
                <DecisionReferences detail=detail_references_side />
            </div>
        }
    });
    let similar_view = view! {
        <div class="flex flex-col gap-8">
            {references_side}
            <div class="contents" data-usage=UiAction::DecisionSimilar.as_str()>
                <Suspense fallback=move || {
                view! { <DecisionSimilar principe=vec![] espece=vec![] loading=true /> }
            }>
                {move || {
                    Suspend::new(async move {
                        let resolved = similar.await;
                        view! {
                            <DecisionSimilar
                                principe=resolved.principe
                                espece=resolved.espece
                                abstained=resolved.abstained
                                error=resolved.error
                            />
                        }
                    })
                }}
                </Suspense>
            </div>
        </div>
    }
    .into_any();

    view! {
        <Title text=page_title />
        <Meta name="description" content=description.clone() />
        <Meta property="og:type" content="article" />
        <Meta property="og:site_name" content="LibreJustice" />
        <Meta property="og:title" content=title.clone() />
        <Meta property="og:description" content=description.clone() />
        <Meta property="og:url" content=url.clone() />
        <Meta property="og:locale" content="fr_FR" />
        <Meta property="og:image" content=OG_IMAGE />
        <Meta property="og:image:width" content="1200" />
        <Meta property="og:image:height" content="630" />
        <Meta name="twitter:card" content="summary_large_image" />
        {(!has_text).then(|| view! { <Meta name="robots" content="noindex" /> })}
        <Link rel="canonical" href=url />
        <Script type_="application/ld+json">{jsonld}</Script>
        {breadcrumb_jsonld
            .map(|bc| view! { <Script type_="application/ld+json">{bc}</Script> })}
        <DecisionLayout toc=toc_view main=main_view similar=similar_view />
    }
}
