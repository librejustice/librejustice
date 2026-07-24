//! Hubs du catalogue des normes (ADR 0255) — l'arborescence navigable vers
//! les textes : `/normes` (catalogue par fond) → `/normes/{fond}` (années)
//! → `/normes/{fond}/{annee}?page=N` (liste paginée de liens `/texte/{slug}` ;
//! `annee` = année ou `sans-date`). Pages SSR crawlables. Le fond `codes`
//! renvoie vers le catalogue `/codes` existant.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{NormCatalogueResponse, NormFondResponse, NormYearResponse};

use crate::api::ApiClient;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, fmt_count, set_cache_control, HubBreadcrumb, HubError, HubSkeleton,
    YearPagination,
};
use crate::pages::law_page::data::{sendable, PageError};

/// Token d'URL du bucket des textes sans date de parcours.
const UNDATED_TOKEN: &str = "sans-date";

/// Libellé d'une année de hub (`None` = bucket sans date).
fn year_label(year: Option<i32>) -> String {
    match year {
        Some(y) => y.to_string(),
        None => "Sans date".to_string(),
    }
}

/// Segment d'URL d'une année de hub.
fn year_token(year: Option<i32>) -> String {
    match year {
        Some(y) => y.to_string(),
        None => UNDATED_TOKEN.to_string(),
    }
}

// ---------------------------------------------------------------------------
// /normes — catalogue par fond
// ---------------------------------------------------------------------------

#[component]
pub fn NormesPage() -> impl IntoView {
    let catalogue = Resource::new_blocking(
        || (),
        |_| {
            sendable(async {
                ApiClient::from_context()
                    .fetch_normes()
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match catalogue.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <NormesLoaded response=r /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn NormesLoaded(response: NormCatalogueResponse) -> impl IntoView {
    let title = "Catalogue des normes - LibreJustice";
    let description = "Tous les textes du corpus normatif — codes, lois et ordonnances, \
         décrets, arrêtés, conventions collectives, textes européens, traités, circulaires, \
         BOFiP — par fond puis par année.";
    let url = "https://librejustice.fr/normes".to_string();
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", url.clone()),
    ]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Textes & codes"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Catalogue des normes"</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    "Parcourez les textes du corpus par fond, puis par année. "
                    "Chaque fond ouvre la liste complète de ses textes."
                </p>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {response
                    .fonds
                    .into_iter()
                    .map(|f| {
                        // Le fond `codes` a son propre catalogue navigable (sommaire
                        // par code), pas un hub année.
                        let href = if f.fond == "codes" {
                            "/codes".to_string()
                        } else {
                            format!("/normes/{}", f.fond)
                        };
                        let count = format!(
                            "{} texte{}",
                            fmt_count(f.text_count),
                            if f.text_count > 1 { "s" } else { "" },
                        );
                        view! {
                            <li class="py-2.5">
                                <A
                                    href=href
                                    attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                >
                                    <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                        {f.label}
                                    </span>
                                    <span class="tabular-nums text-sm text-[var(--color-ink-subtle)]">
                                        {count}
                                    </span>
                                </A>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </div>
    }
}

// ---------------------------------------------------------------------------
// /normes/{fond} — hub d'un fond (années)
// ---------------------------------------------------------------------------

#[component]
pub fn NormFondPage() -> impl IntoView {
    let params = use_params_map();
    let hub = Resource::new_blocking(
        move || params.get().get("fond").unwrap_or_default(),
        |fond| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_norm_fond(&fond)
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match hub.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <NormFondLoaded hub=r /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn NormFondLoaded(hub: NormFondResponse) -> impl IntoView {
    let title = format!("{} : textes - LibreJustice", hub.label);
    let description = format!(
        "Les {} textes du fond « {} », par année.",
        fmt_count(hub.text_count),
        hub.label,
    );
    let url = format!("https://librejustice.fr/normes/{}", hub.fond);
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", "https://librejustice.fr/normes".to_string()),
        (&hub.label, url.clone()),
    ]);
    let dated: Vec<i32> = hub.years.iter().filter_map(|y| y.year).collect();
    let span = match (dated.iter().min(), dated.iter().max()) {
        (Some(min), Some(max)) => format!(", de {min} à {max}"),
        _ => String::new(),
    };

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Normes".to_string(), Some("/normes".to_string())),
                    (hub.label.clone(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Textes & codes"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{hub.label.clone()}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {format!("{} textes{span}.", fmt_count(hub.text_count))}
                </p>
            </header>

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Textes par année"
                </h2>
                <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                    {hub
                        .years
                        .into_iter()
                        .map(|y| {
                            let href = format!("/normes/{}/{}", hub.fond, year_token(y.year));
                            let count = format!(
                                "{} texte{}",
                                fmt_count(y.count),
                                if y.count > 1 { "s" } else { "" },
                            );
                            view! {
                                <li class="py-2.5">
                                    <A
                                        href=href
                                        attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                    >
                                        <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                            {year_label(y.year)}
                                        </span>
                                        <span class="tabular-nums text-sm text-[var(--color-ink-subtle)]">
                                            {count}
                                        </span>
                                    </A>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </section>
        </div>
    }
}

// ---------------------------------------------------------------------------
// /normes/{fond}/{annee} — liste paginée des textes d'une année
// ---------------------------------------------------------------------------

#[component]
pub fn NormYearPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let page_data = Resource::new_blocking(
        move || {
            (
                params.get().get("fond").unwrap_or_default(),
                params.get().get("annee").unwrap_or_default(),
                query
                    .get()
                    .get("page")
                    .and_then(|p| p.parse::<u32>().ok())
                    .unwrap_or(1),
            )
        },
        |(fond, annee, page)| {
            sendable(async move {
                let year: Option<i32> = if annee == UNDATED_TOKEN {
                    None
                } else {
                    Some(annee.parse().map_err(|_| PageError {
                        status: 404,
                        message: "Année invalide".to_string(),
                    })?)
                };
                ApiClient::from_context()
                    .fetch_norm_year(&fond, year, page)
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match page_data.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <NormYearLoaded page=r /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn NormYearLoaded(page: NormYearResponse) -> impl IntoView {
    let page_size = i64::from(page.page_size);
    let page_count = ((page.total + page_size - 1) / page_size).max(1) as u32;
    let ylabel = year_label(page.year);
    let title = if page.page > 1 {
        format!(
            "{} : textes {} (page {}) - LibreJustice",
            page.label, ylabel, page.page,
        )
    } else {
        format!("{} : textes {} - LibreJustice", page.label, ylabel)
    };
    let description = match page.year {
        Some(y) => format!(
            "Les {} textes de {} du fond « {} », en texte intégral.",
            fmt_count(page.total),
            y,
            page.label,
        ),
        None => format!(
            "Les {} textes sans date du fond « {} ».",
            fmt_count(page.total),
            page.label,
        ),
    };
    let base_url = format!(
        "https://librejustice.fr/normes/{}/{}",
        page.fond,
        year_token(page.year),
    );
    // Canonical par page (?page=N au-delà de la première) : chaque page de la
    // pagination est un document distinct, pas un doublon de la première.
    let canonical = if page.page > 1 {
        format!("{base_url}?page={}", page.page)
    } else {
        base_url.clone()
    };
    let hub_href = format!("/normes/{}", page.fond);
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", "https://librejustice.fr/normes".to_string()),
        (
            &page.label,
            format!("https://librejustice.fr/normes/{}", page.fond),
        ),
        (&ylabel, base_url.clone()),
    ]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=canonical />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Normes".to_string(), Some("/normes".to_string())),
                    (page.label.clone(), Some(hub_href.clone())),
                    (ylabel.clone(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {format!("{} — {}", page.label, ylabel)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {match page.year {
                        Some(_) => format!(
                            "{} textes, du plus récent au plus ancien.",
                            fmt_count(page.total),
                        ),
                        None => format!(
                            "{} textes sans date de signature ni de publication connue.",
                            fmt_count(page.total),
                        ),
                    }}
                </p>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {page
                    .texts
                    .into_iter()
                    .map(|t| {
                        let href = format!("/texte/{}", t.slug);
                        view! {
                            <li class="py-2.5">
                                <A
                                    href=href
                                    attr:class="block text-base text-[var(--color-ink)] hover:text-[var(--color-accent)]"
                                >
                                    {t.title}
                                </A>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>

            <YearPagination
                base=format!("/normes/{}/{}", page.fond, year_token(page.year))
                page=page.page
                page_count=page_count
            />
        </div>
    }
}
