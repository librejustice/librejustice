//! Hubs of the norms catalogue, the browsable tree down to a jurisdiction's texts:
//! `/norm-index/{juris}` (catalogue by fond) → `/norm-index/{juris}/{fond}` (years)
//! → `/norm-index/{juris}/{fond}/{annee}?page=N` (paginated list of
//! `/norm/{juris}/{slug}` links; `annee` = a year or `sans-date`). Crawlable SSR
//! pages. The French `codes` fond has its own catalogue `/norm-index/fr/codes`
//! (table of contents per code, not a year hub, `CodeCataloguePage`).

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{NormCatalogueResponse, NormFondResponse, NormYearResponse};

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::helpers::norms_href;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, fmt_count, set_cache_control, HubBreadcrumb, HubError, HubSkeleton,
    YearPagination, DEEP_LINK,
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
// /norm-index/{juris}, catalogue par fond
// ---------------------------------------------------------------------------

#[component]
pub fn NormesPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let params = use_params_map();
    let catalogue = Resource::new_blocking(
        move || params.get().get("juris").unwrap_or_default(),
        |juris| {
            sendable(async move {
                let catalogue = ApiClient::from_context()
                    .fetch_normes()
                    .await
                    .map_err(PageError::from)?;
                if !catalogue
                    .jurisdictions
                    .iter()
                    .any(|j| j.jurisdiction == juris)
                {
                    return Err(PageError {
                        status: 404,
                        message: "Juridiction inconnue".to_string(),
                    });
                }
                Ok((juris, catalogue))
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match catalogue.await {
                    Ok((juris, r)) => {
                        set_cache_control(200);
                        Either::Left(view! { <NormesLoaded juris=juris response=r /> })
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
fn NormesLoaded(juris: String, response: NormCatalogueResponse) -> impl IntoView {
    let (current, others): (Vec<_>, Vec<_>) = response
        .jurisdictions
        .into_iter()
        .partition(|j| j.jurisdiction == juris);
    let fonds = current
        .into_iter()
        .flat_map(|j| j.fonds)
        .collect::<Vec<_>>();
    let title = if juris == "fr" {
        "Catalogue des normes - LibreJustice".to_string()
    } else {
        format!(
            "Catalogue des normes ({}) - LibreJustice",
            juris.to_uppercase()
        )
    };
    let description = "Tous les textes du corpus normatif — codes, lois et ordonnances, \
         décrets, arrêtés, conventions collectives, textes européens, traités, circulaires, \
         BOFiP — par fond puis par année.";
    let url = format!("https://librejustice.fr/norm-index/{juris}");
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", url.clone()),
    ]);
    let search_href = norms_href([("jurisdiction", lj_vocab::jurisdiction_of_segment(&juris))]);

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
                <A href=search_href attr:class=DEEP_LINK>"Rechercher dans ces textes"</A>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {fonds
                    .into_iter()
                    .map(|f| {
                        let href = format!("/norm-index/{juris}/{}", f.fond);
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

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Autres juridictions"
                </h2>
                <ul class="flex flex-wrap gap-x-4 gap-y-1 text-sm">
                    {others
                        .into_iter()
                        .map(|j| {
                            let href = format!("/norm-index/{}", j.jurisdiction);
                            let label = format!(
                                "{} ({})",
                                j.jurisdiction.to_uppercase(),
                                fmt_count(j.text_count),
                            );
                            view! {
                                <li>
                                    <A
                                        href=href
                                        attr:class="text-[var(--color-ink-muted)] hover:text-[var(--color-accent)]"
                                    >
                                        {label}
                                    </A>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </section>

            <p class="max-w-prose text-sm text-[var(--color-ink-muted)]">
                "Ce catalogue liste tous les textes du fonds. Pour ne garder que ceux qui s'appliquent en un territoire, à une date donnée : "
                <A
                    href="/norm-index/fr/in-force"
                    attr:class="text-[var(--color-accent)] underline underline-offset-2 hover:opacity-80"
                >
                    "ce qui s'applique en un lieu"
                </A>
                "."
            </p>
        </div>
    }
}

// ---------------------------------------------------------------------------
// /norm-index/{juris}/{fond}, hub d'un fond (années)
// ---------------------------------------------------------------------------

#[component]
pub fn NormFondPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let params = use_params_map();
    let hub = Resource::new_blocking(
        move || {
            let p = params.get();
            (
                p.get("juris").unwrap_or_default(),
                p.get("fond").unwrap_or_default(),
            )
        },
        |(juris, fond)| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_norm_fond(&juris, &fond)
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
    let index = format!("/norm-index/{}", hub.jurisdiction);
    let url = format!("https://librejustice.fr{index}/{}", hub.fond);
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", format!("https://librejustice.fr{index}")),
        (&hub.label, url.clone()),
    ]);
    let dated: Vec<i32> = hub.years.iter().filter_map(|y| y.year).collect();
    let span = match (dated.iter().min(), dated.iter().max()) {
        (Some(min), Some(max)) => format!(", de {min} à {max}"),
        _ => String::new(),
    };
    let search_href = norms_href(hub.search_paths.into_iter().map(|p| ("jurisdiction", p)));

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Normes".to_string(), Some(index.clone())),
                    (hub.label.clone(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Textes & codes"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{hub.label.clone()}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {format!("{} textes{span}.", fmt_count(hub.text_count))}
                </p>
                <A href=search_href attr:class=DEEP_LINK>"Rechercher dans ces textes"</A>
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
                            let href = format!("{index}/{}/{}", hub.fond, year_token(y.year));
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
// /norm-index/{juris}/{fond}/{annee}, liste paginée des textes d'une année
// ---------------------------------------------------------------------------

#[component]
pub fn NormYearPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let params = use_params_map();
    let query = use_query_map();
    let page_data = Resource::new_blocking(
        move || {
            (
                params.get().get("juris").unwrap_or_default(),
                params.get().get("fond").unwrap_or_default(),
                params.get().get("annee").unwrap_or_default(),
                query
                    .get()
                    .get("page")
                    .and_then(|p| p.parse::<u32>().ok())
                    .unwrap_or(1),
            )
        },
        |(juris, fond, annee, page)| {
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
                    .fetch_norm_year(&juris, &fond, year, page)
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
    let index = format!("/norm-index/{}", page.jurisdiction);
    let hub_href = format!("{index}/{}", page.fond);
    let year_href = format!("{hub_href}/{}", year_token(page.year));
    let base_url = format!("https://librejustice.fr{year_href}");
    // Canonical par page (?page=N au-delà de la première) : chaque page de la
    // pagination est un document distinct, pas un doublon de la première.
    let canonical = if page.page > 1 {
        format!("{base_url}?page={}", page.page)
    } else {
        base_url.clone()
    };
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", format!("https://librejustice.fr{index}")),
        (&page.label, format!("https://librejustice.fr{hub_href}")),
        (&ylabel, base_url.clone()),
    ]);
    let bounds = page.year.into_iter().flat_map(|y| {
        [
            ("valid_from", format!("{y}-01-01")),
            ("as_of", format!("{y}-12-31")),
        ]
    });
    let search_href = norms_href(
        page.search_paths
            .iter()
            .map(|p| ("jurisdiction", p.clone()))
            .chain(bounds),
    );
    let search_label = match page.year {
        Some(y) => format!("Rechercher dans les textes de {y}"),
        None => "Rechercher dans ces textes".to_string(),
    };

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=canonical />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Normes".to_string(), Some(index.clone())),
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
                <A href=search_href attr:class=DEEP_LINK>{search_label}</A>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {page
                    .texts
                    .into_iter()
                    .map(|t| {
                        let href = lj_vocab::norm_path(&t.jurisdiction, &t.slug);
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
                base=year_href
                page=page.page
                page_count=page_count
            />
        </div>
    }
}
