//! Hubs of the norms catalogue, the browsable tree down to a jurisdiction's texts:
//! `/norm-index/{juris}` (catalogue by fond) → `/norm-index/{juris}/{fond}` (years)
//! → `/norm-index/{juris}/{fond}/{annee}?page=N` (paginated list of
//! `/norm/{juris}/{slug}` links; `annee` = a year or `sans-date`). Crawlable SSR
//! pages. The French `codes` fond has its own catalogue `/norm-index/fr/codes`
//! (table of contents per code, not a year hub, `CodeCataloguePage`).

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{NormCatalogueResponse, NormFondResponse, NormYearResponse};

use crate::api::{ApiClient, Failure};
use crate::components::own_page_description;
use crate::helpers::group_thousands;
use crate::helpers::norms_href;
use crate::i18n::HubsUi;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, set_cache_control, HubBreadcrumb, HubError, HubSkeleton, YearPagination,
    DEEP_LINK,
};
use crate::pages::law_page::data::{sendable, PageError};

/// Token d'URL du bucket des textes sans date de parcours.
const UNDATED_TOKEN: &str = "sans-date";

/// Libellé d'une année de hub (`None` = bucket sans date).
fn year_label(ui: &HubsUi, year: Option<i32>) -> String {
    match year {
        Some(y) => y.to_string(),
        None => ui.undated.to_string(),
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

    let ui = &crate::i18n::use_ui().hubs;
    let params = use_params_map();
    let catalogue = Resource::new_blocking(
        move || params.get().get("juris").unwrap_or_default(),
        move |juris| {
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
                        failure: Failure::Said(ui.unknown_jurisdiction.to_string()),
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
    let numbers = &crate::i18n::use_ui().numbers;
    let all = crate::i18n::use_ui();
    let ui = &all.hubs;
    let (current, others): (Vec<_>, Vec<_>) = response
        .jurisdictions
        .into_iter()
        .partition(|j| j.jurisdiction == juris);
    let fonds = current
        .into_iter()
        .flat_map(|j| j.fonds)
        .collect::<Vec<_>>();
    let title = if juris == "fr" {
        ui.catalogue_title.to_string()
    } else {
        (ui.catalogue_title_for)(&juris.to_uppercase())
    };
    let description = ui.catalogue_description;
    let url = format!("https://librejustice.fr/norm-index/{juris}");
    let jsonld = breadcrumb_jsonld(&[
        (all.shell.home, "https://librejustice.fr/".to_string()),
        (all.norm.norms, url.clone()),
    ]);
    let search_href = norms_href([("jurisdiction", lj_vocab::jurisdiction_of_segment(&juris))]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.texts_and_codes}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.catalogue_heading}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">{ui.catalogue_intro}</p>
                <A href=crate::jurisdiction::use_reader_scope().href(search_href) attr:class=DEEP_LINK>{ui.search_these}</A>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {fonds
                    .into_iter()
                    .map(|f| {
                        let href = format!("/norm-index/{juris}/{}", f.fond);
                        let count = (ui.n_texts)(&group_thousands(numbers, f.text_count), f.text_count);
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
                    {ui.other_jurisdictions}
                </h2>
                <ul class="flex flex-wrap gap-x-4 gap-y-1 text-sm">
                    {others
                        .into_iter()
                        .map(|j| {
                            let href = format!("/norm-index/{}", j.jurisdiction);
                            let label = format!(
                                "{} ({})",
                                j.jurisdiction.to_uppercase(),
                                group_thousands(numbers, j.text_count),
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
                {ui.in_force_lead}
                <A
                    href=crate::i18n::use_reader_lang().path("/norm-index/fr/in-force")
                    attr:class="text-[var(--color-accent)] underline underline-offset-2 hover:opacity-80"
                >
                    {ui.in_force_link}
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
    let numbers = &crate::i18n::use_ui().numbers;
    let all = crate::i18n::use_ui();
    let ui = &all.hubs;
    let title = (ui.fond_title)(&hub.label);
    let description = (ui.fond_description)(&group_thousands(numbers, hub.text_count), &hub.label);
    let index = format!("/norm-index/{}", hub.jurisdiction);
    let url = format!("https://librejustice.fr{index}/{}", hub.fond);
    let jsonld = breadcrumb_jsonld(&[
        (all.shell.home, "https://librejustice.fr/".to_string()),
        (all.norm.norms, format!("https://librejustice.fr{index}")),
        (&hub.label, url.clone()),
    ]);
    let dated: Vec<i32> = hub.years.iter().filter_map(|y| y.year).collect();
    let span = dated
        .iter()
        .min()
        .zip(dated.iter().max())
        .map(|(min, max)| (*min, *max));
    let search_href = norms_href(hub.search_paths.into_iter().map(|p| ("jurisdiction", p)));

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (all.norm.norms.to_string(), Some(index.clone())),
                    (hub.label.clone(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.texts_and_codes}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{hub.label.clone()}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.fond_intro)(&group_thousands(numbers, hub.text_count), span)}
                </p>
                <A href=crate::jurisdiction::use_reader_scope().href(search_href) attr:class=DEEP_LINK>{ui.search_these}</A>
            </header>

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.by_year}
                </h2>
                <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                    {hub
                        .years
                        .into_iter()
                        .map(|y| {
                            let href = format!("{index}/{}/{}", hub.fond, year_token(y.year));
                            let count = (ui.n_texts)(&group_thousands(numbers, y.count), y.count);
                            view! {
                                <li class="py-2.5">
                                    <A
                                        href=href
                                        attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                    >
                                        <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                            {year_label(ui, y.year)}
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

    let ui = &crate::i18n::use_ui().hubs;
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
        move |(juris, fond, annee, page)| {
            sendable(async move {
                let year: Option<i32> = if annee == UNDATED_TOKEN {
                    None
                } else {
                    Some(annee.parse().map_err(|_| PageError {
                        status: 404,
                        failure: Failure::Said(ui.invalid_year.to_string()),
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
    let lang = crate::i18n::use_reader_lang().get();
    let numbers = &crate::i18n::use_ui().numbers;
    let page_size = i64::from(page.page_size);
    let page_count = ((page.total + page_size - 1) / page_size).max(1) as u32;
    let all = crate::i18n::use_ui();
    let ui = &all.hubs;
    let ylabel = year_label(ui, page.year);
    let title = (ui.year_title)(&page.label, &ylabel, page.page);
    let total = group_thousands(numbers, page.total);
    let description = (ui.year_description)(&total, page.year, &page.label);
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
        (all.shell.home, "https://librejustice.fr/".to_string()),
        (all.norm.norms, format!("https://librejustice.fr{index}")),
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
        Some(y) => (ui.search_year)(y),
        None => ui.search_these.to_string(),
    };

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=canonical />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (all.norm.norms.to_string(), Some(index.clone())),
                    (page.label.clone(), Some(hub_href.clone())),
                    (ylabel.clone(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {(ui.year_heading)(&page.label, &ylabel)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.year_intro)(&total, page.year)}
                </p>
                <A href=crate::jurisdiction::use_reader_scope().href(search_href) attr:class=DEEP_LINK>{search_label}</A>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {page
                    .texts
                    .into_iter()
                    .map(|mut t| {
                        crate::i18n::relabel_title(lang, &mut t.title);
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
