//! Page `/decision/:id` (SEO-critique). Port de `apps/web/src/pages/decision-page.tsx`.
//!
//! - Détail : `Resource::new_blocking` ⇒ le SSR attend la résolution avant
//!   d'émettre le HTML, donc `<Title>`/`<Meta>`/JSON-LD sont dans le document
//!   initial (crawlables). Équivaut au loader bloquant + export `meta` RR.
//! - Similaires : `Resource` non bloquante + `<Suspense>` ⇒ streamées après le
//!   shell (SsrMode::PartiallyBlocked posé par la substrate dans `app.rs`).
//! - Statut HTTP + `Cache-Control` (SSR) posés sur la réponse document via
//!   `ResponseOptions` (200 → 7 j CDN, 404/400 → 5 min, 5xx → no-store). Port de
//!   `decisionCacheControl` (`loaders.ts`). ADR 0061 : le cache CDN est posé par
//!   l'app (Caddy supprimé), plus par un reverse-proxy.

pub mod data;
pub mod labels;
pub mod reference;
pub mod sections;
pub mod toc_spy;

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Script, Title};
use leptos_router::components::A;

use crate::components::decision::{
    DecisionBody, DecisionCommentaires, DecisionHeader, DecisionLayout, DecisionMeta,
    DecisionParties, DecisionProvenance, DecisionSimilar, DecisionSkeleton, DecisionToc,
};
use crate::seo::decision::{build_json_ld, meta_description};
use crate::seo::{canonical_url, OG_IMAGE};

use data::{
    decision_id, fetch_detail, fetch_parties, fetch_similar, sendable, PageError, PartiesResult,
    SimilarResult,
};
use reference::build_decision_references;
use sections::{resolve_decision_sections, toc_sections};

#[component]
pub fn DecisionPage() -> impl IntoView {
    let id = decision_id();

    // Détail bloquant (SEO dans le document initial).
    let detail = Resource::new_blocking(move || id.get(), |id| sendable(fetch_detail(id)));
    // Voisins non bloquants (streamés via <Suspense>).
    let similar = Resource::new(move || id.get(), |id| sendable(fetch_similar(id)));
    // Parties (encart) non bloquantes (streamées via <Suspense>).
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
/// détail étant bloquant, tout est posé avant le flush des headers. Sans le
/// statut, une décision inconnue partirait en 200 (soft 404) — Bing indexe
/// alors ces URLs comme valides malgré le `noindex`. No-op côté hydrate.
#[cfg(feature = "ssr")]
fn set_cache_control(status: u16, summary_present: bool) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    let value = match status {
        // Résumé manquant (cas résiduel : décision pas encore résumée par le cron
        // ou le rerank) : on NE fige PAS la page sans synthèse 7 j au CDN — sinon
        // la description fallback resterait cachée. TTL court ⇒ la prochaine
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

/// Page d'erreur (id invalide / 404 / autre). Pose `robots noindex`. Port du
/// `DecisionError` + branche `meta` `!data.detail` de `decision-page.tsx`.
#[component]
fn DecisionError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    // Détail absent ⇒ noindex + titre « introuvable » (parité `meta` branche
    // `!data.detail` ; le cas « Chargement… » correspond au fallback Suspense).
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
/// toc/main/similar. Port de la branche `DecisionLayout` de `decision-page.tsx`.
#[component]
fn DecisionLoaded(
    detail: lj_dtos::DecisionDetail,
    similar: Resource<SimilarResult>,
    parties: Resource<PartiesResult>,
) -> impl IntoView {
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(&detail.id);
    let jsonld = serde_json::to_string(&build_json_ld(&detail, &title, &description))
        .unwrap_or_else(|_| "{}".to_string());
    // Fil d'Ariane structuré vers l'arborescence juridiction (ADR 0253) —
    // absent si la décision n'a pas de code juridiction résolu.
    let breadcrumb_jsonld = detail.jurisdiction_code.as_ref().map(|code| {
        let label = detail
            .jurisdiction_name
            .clone()
            .unwrap_or_else(|| code.clone());
        let mut items = vec![
            ("Accueil", "https://librejustice.fr/".to_string()),
            (
                "Juridictions",
                "https://librejustice.fr/juridictions".to_string(),
            ),
            (
                label.as_str(),
                format!("https://librejustice.fr/juridiction/{code}"),
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
                format!("https://librejustice.fr/juridiction/{code}/{y}"),
            ));
        }
        items.push((title.as_str(), url.clone()));
        crate::pages::juridictions_page::breadcrumb_jsonld(&items)
    });

    // `<title>` = titre canonique complet (avec siège). Bing le marque « Title
    // too long » (> ~60 caractères) : accepté — la norme du secteur (de référence,
    // , Predictice) est le titre complet, tronqué à l'affichage par le
    // moteur ; les mots-clés priment sur la borne d'affichage.
    let page_title = format!("{title} - LibreJustice");
    let references_full = build_decision_references(&detail).full;

    let body_sections = resolve_decision_sections(&detail);
    // Décision sans texte intégral (lacune de la source) : la page rend
    // « Texte intégral non disponible » en 200 — noindex pour ne pas faire
    // indexer une coquille comme une vraie décision.
    let has_text = body_sections.iter().any(|s| !s.paragraphs.is_empty());
    let toc = toc_sections(&detail, &body_sections);

    let detail_header = detail.clone();
    let detail_meta = detail.clone();
    let detail_body = detail.clone();
    let detail_provenance = detail.clone();
    let detail_fallback = detail.clone();
    let detail_similar = detail.clone();

    let toc_view =
        view! { <DecisionToc sections=toc chronology=detail.chronology.clone() /> }.into_any();
    // Encart « Parties » (ADR 0189) sous la synthèse : streamé, masqué si vide.
    let parties_view = view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                let resolved = parties.await;
                match resolved.error {
                    // Erreur silencieuse : l'encart est un enrichissement, pas un
                    // contenu porteur — on ne pollue pas la page décision.
                    Some(_) => ().into_any(),
                    None => view! { <DecisionParties parties=resolved.parties /> }.into_any(),
                }
            })}
        </Suspense>
    }
    .into_any();

    let main_view = view! {
        <article class="flex flex-col gap-10">
            <h1 class="sr-only lj-doc-title">{references_full}</h1>
            <DecisionHeader detail=detail_header />
            <DecisionMeta detail=detail_meta section_id="synthese" />
            {parties_view}
            <DecisionBody detail=detail_body sections=body_sections />
            <DecisionCommentaires commentaires=detail.commentaires.clone() />
            <DecisionProvenance detail=detail_provenance />
            <DecisionHubLinks detail=detail.clone() />
        </article>
    }
    .into_any();
    let similar_view = view! {
        <Suspense fallback=move || {
            let detail_fallback = detail_fallback.clone();
            view! {
                <DecisionSimilar detail=detail_fallback hits=vec![] loading=true />
            }
        }>
            {
                let detail_similar = detail_similar.clone();
                move || {
                    let detail_similar = detail_similar.clone();
                    Suspend::new(async move {
                        let resolved = similar.await;
                        view! {
                            <DecisionSimilar
                                detail=detail_similar
                                hits=resolved.hits
                                error=resolved.error
                            />
                        }
                    })
                }
            }
        </Suspense>
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

/// Maillage retour vers l'arborescence navigable (ADR 0253) : la juridiction
/// de la décision et son année. Rien sans code juridiction résolu.
#[component]
fn DecisionHubLinks(detail: lj_dtos::DecisionDetail) -> impl IntoView {
    detail.jurisdiction_code.clone().map(|code| {
        let label = detail
            .jurisdiction_name
            .clone()
            .unwrap_or_else(|| code.clone());
        let year = detail
            .date_lecture
            .as_deref()
            .and_then(|d| d.get(..4))
            .map(str::to_string);
        let hub_href = format!("/juridiction/{code}");
        view! {
            <nav
                aria-label="Autres décisions"
                class="border-t border-[var(--color-rule)] pt-4 text-sm text-[var(--color-ink-muted)]"
            >
                "Autres décisions : "
                <A href=hub_href attr:class="text-[var(--color-accent)] hover:underline">
                    {label}
                </A>
                {year
                    .map(|y| {
                        let year_href = format!("/juridiction/{code}/{y}");
                        view! {
                            " · "
                            <A href=year_href attr:class="text-[var(--color-accent)] hover:underline">
                                {format!("année {y}")}
                            </A>
                        }
                    })}
            </nav>
        }
    })
}
