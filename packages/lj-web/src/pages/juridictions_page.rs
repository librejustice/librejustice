//! Hubs juridiction — l'arborescence navigable vers les décisions :
//! `/decision-index/{juris}` (catalogue par famille) →
//! `/decision-index/{juris}/{code}` (années) →
//! `/decision-index/{juris}/{code}/{year}?page=N` (liste paginée de liens
//! décision).
//! Pages SSR crawlables : chaque décision est à quelques clics de la home.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{
    JurisdictionCatalogueResponse, JurisdictionHubResponse, JurisdictionType,
    JurisdictionTypeGroup, JurisdictionYearResponse,
};

use crate::api::{ApiClient, Failure};
use crate::components::own_page_description;
use crate::helpers::decisions_href;
use crate::helpers::group_thousands;
use crate::i18n::HubsUi;
use crate::pages::law_page::data::{sendable, PageError};

fn not_found(ui: &HubsUi) -> PageError {
    PageError {
        status: 404,
        failure: Failure::Said(ui.unknown_jurisdiction.to_string()),
    }
}

/// Keeps a court only when it belongs to the jurisdiction the URL names.
fn in_jurisdiction(jurisdiction_type: &str, code: &str, juris: &str) -> bool {
    JurisdictionType::from_code(jurisdiction_type)
        .is_some_and(|t| t.url_segment(Some(code)) == juris)
}

pub(crate) const DEEP_LINK: &str =
    "w-fit text-sm text-[var(--color-accent)] underline-offset-4 hover:underline";

/// JSON-LD `BreadcrumbList` schema.org d'un fil d'Ariane (label, url absolue).
/// Partagé avec la page décision.
pub(crate) fn breadcrumb_jsonld(items: &[(&str, String)]) -> String {
    let elements: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(i, (name, url))| {
            serde_json::json!({
                "@type": "ListItem",
                "position": i + 1,
                "name": name,
                "item": url,
            })
        })
        .collect();
    let json = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": elements,
    });
    serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(feature = "ssr")]
pub(crate) fn set_cache_control(status: u16) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    let value = match status {
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 | 422 => "public, max-age=0, s-maxage=300",
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
pub(crate) fn set_cache_control(_status: u16) {}

#[component]
pub(crate) fn HubError(err: PageError) -> impl IntoView {
    let all = crate::i18n::use_ui();
    view! {
        <Title text=all.hubs.not_found_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {all.shell.error}
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{err.failure.text(&crate::i18n::use_ui().shell)}</h1>
        </div>
    }
}

#[component]
pub(crate) fn HubSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-9 w-1/2" />
            <Skeleton class="h-4 w-2/3" />
            <Skeleton class="h-4 w-1/2" />
        </div>
    }
}

/// The visible breadcrumb of a hub page, after the home page of the reader's jurisdiction.
#[component]
pub(crate) fn HubBreadcrumb(items: Vec<(String, Option<String>)>) -> impl IntoView {
    let shell = &crate::i18n::use_ui().shell;
    let home = crate::jurisdiction::use_reader_scope().href("/");
    view! {
        <nav aria-label=shell.breadcrumb class="flex flex-wrap items-center gap-1.5 text-sm text-[var(--color-ink-subtle)]">
            <A href=home attr:class="hover:text-[var(--color-accent)]">
                {shell.home}
            </A>
            {items
                .into_iter()
                .map(|(label, href)| {
                    view! {
                        <span aria-hidden="true">"›"</span>
                        {match href {
                            Some(href) => Either::Left(
                                view! {
                                    <A href=href attr:class="hover:text-[var(--color-accent)]">
                                        {label.clone()}
                                    </A>
                                },
                            ),
                            None => Either::Right(
                                view! { <span class="text-[var(--color-ink-muted)]">{label.clone()}</span> },
                            ),
                        }}
                    }
                })
                .collect_view()}
        </nav>
    }
}

// ---------------------------------------------------------------------------
// /decision-index/{juris}, catalogue par famille
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionsPage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let ui = &crate::i18n::use_ui().hubs;
    let params = use_params_map();
    let catalogue = Resource::new_blocking(
        move || params.get().get("juris").unwrap_or_default(),
        move |juris| {
            sendable(async move {
                let mut catalogue = ApiClient::from_context()
                    .fetch_juridictions()
                    .await
                    .map_err(PageError::from)?;
                for group in &mut catalogue.groups {
                    let kind = group.jurisdiction_type.clone();
                    group
                        .jurisdictions
                        .retain(|j| in_jurisdiction(&kind, &j.code, &juris));
                }
                catalogue.groups.retain(|g| !g.jurisdictions.is_empty());
                if catalogue.groups.is_empty() {
                    return Err(not_found(ui));
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
                        Either::Left(view! { <JuridictionsLoaded juris=juris response=r /> })
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
fn JuridictionsLoaded(juris: String, response: JurisdictionCatalogueResponse) -> impl IntoView {
    let all = crate::i18n::use_ui();
    let ui = &all.hubs;
    let title = ui.jurisdictions_title;
    let description = ui.jurisdictions_description;
    let url = format!("https://librejustice.fr/decision-index/{juris}");
    let jsonld = breadcrumb_jsonld(&[
        (all.shell.home, "https://librejustice.fr/".to_string()),
        (ui.jurisdictions, url.clone()),
    ]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.case_law}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.jurisdictions}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">{ui.jurisdictions_intro}</p>
                <A href=crate::jurisdiction::use_reader_scope().href("/decision-search") attr:class=DEEP_LINK>{ui.search_decisions}</A>
            </header>

            {response
                .groups
                .into_iter()
                .map(|group| view! { <JuridictionGroup juris=juris.clone() group=group /> })
                .collect_view()}
        </div>
    }
}

#[component]
fn JuridictionGroup(juris: String, group: JurisdictionTypeGroup) -> impl IntoView {
    let lang = crate::i18n::use_reader_lang().get();
    let numbers = &lang.ui().numbers;
    let ui = &lang.ui().hubs;
    let heading = (lang.ui().referential.jurisdiction_type)(&group.jurisdiction_type)
        .map_or(group.label, str::to_string);
    view! {
        <section class="flex flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {heading}
            </h2>
            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {group
                    .jurisdictions
                    .into_iter()
                    .map(|j| {
                        let href = format!("/decision-index/{juris}/{}", j.code);
                        let label = j
                            .court_names
                            .get(lang.code())
                            .map_or(j.label, |named| named.name.clone());
                        let count = (ui.n_decisions)(&group_thousands(numbers, j.decision_count), j.decision_count);
                        view! {
                            <li class="py-2.5">
                                <A
                                    href=href
                                    attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                >
                                    <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                        {label}
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
    }
}

// ---------------------------------------------------------------------------
// /decision-index/{juris}/{code}, hub d'une juridiction (années)
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionHubPage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let params = use_params_map();
    let scope = crate::jurisdiction::use_reader_scope();
    let known = scope.known_resource();
    let hub = Resource::new_blocking(
        move || params.get().get("code").unwrap_or_default(),
        |code| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_juridiction_hub(&code)
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match (hub.await, known.await) {
                    (Ok(r), Ok(list)) => {
                        scope.keep_known(list);
                        set_cache_control(200);
                        Either::Left(view! { <JuridictionHubLoaded hub=r /> })
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn JuridictionHubLoaded(mut hub: JurisdictionHubResponse) -> impl IntoView {
    let lang = crate::i18n::use_reader_lang().get();
    let all = lang.ui();
    let numbers = &all.numbers;
    if let Some(named) = hub.court_names.get(lang.code()) {
        hub.label = named.name.clone();
    }
    if let Some(label) = (all.referential.jurisdiction_type)(&hub.jurisdiction_type) {
        hub.type_label = label.to_string();
    }
    let ui = &all.hubs;
    let title = (ui.court_title)(&hub.label);
    let description =
        (ui.court_description)(&group_thousands(numbers, hub.decision_count), &hub.label);
    let index = format!(
        "/decision-index/{}",
        JurisdictionType::from_code(&hub.jurisdiction_type)
            .expect("a stored jurisdiction_type is a known variant")
            .url_segment(Some(&hub.code))
    );
    let hub_href = format!("{index}/{}", hub.code);
    let url = format!("https://librejustice.fr{hub_href}");
    let jsonld = breadcrumb_jsonld(&[
        (all.shell.home, "https://librejustice.fr/".to_string()),
        (ui.jurisdictions, format!("https://librejustice.fr{index}")),
        (&hub.label, url.clone()),
    ]);
    let search_href = crate::jurisdiction::use_reader_scope().seat_href(
        hub.seat_path.clone(),
        decisions_href([("seat", hub.seat_path.clone())]),
    );

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (ui.jurisdictions.to_string(), Some(index.clone())),
                    (hub.label.clone(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {hub.type_label.clone()}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{hub.label.clone()}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.court_intro)(
                        &group_thousands(numbers, hub.decision_count),
                        hub.years.last().map(|y| y.year).unwrap_or_default(),
                        hub.years.first().map(|y| y.year).unwrap_or_default(),
                    )}
                </p>
                <A href=search_href attr:class=DEEP_LINK>{ui.search_decisions}</A>
            </header>

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.decisions_by_year}
                </h2>
                <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                    {hub
                        .years
                        .into_iter()
                        .map(|y| {
                            let href = format!("{hub_href}/{}", y.year);
                            let count = (ui.n_decisions)(&group_thousands(numbers, y.count), y.count);
                            view! {
                                <li class="py-2.5">
                                    <A
                                        href=href
                                        attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                    >
                                        <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                            {y.year}
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
// /decision-index/{juris}/{code}/{year}, liste paginée des décisions d'une année
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionYearPage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let params = use_params_map();
    let query = use_query_map();
    let scope = crate::jurisdiction::use_reader_scope();
    let known = scope.known_resource();
    let ui = &crate::i18n::use_ui().hubs;
    let page_data = Resource::new_blocking(
        move || {
            (
                params.get().get("code").unwrap_or_default(),
                params.get().get("annee").unwrap_or_default(),
                query
                    .get()
                    .get("page")
                    .and_then(|p| p.parse::<u32>().ok())
                    .unwrap_or(1),
            )
        },
        move |(code, annee, page)| {
            sendable(async move {
                let year: i32 = annee.parse().map_err(|_| PageError {
                    status: 404,
                    failure: Failure::Said(ui.invalid_year.to_string()),
                })?;
                ApiClient::from_context()
                    .fetch_juridiction_year(&code, year, page)
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match (page_data.await, known.await) {
                    (Ok(r), Ok(list)) => {
                        scope.keep_known(list);
                        set_cache_control(200);
                        Either::Left(view! { <JuridictionYearLoaded page=r /> })
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn JuridictionYearLoaded(mut page: JurisdictionYearResponse) -> impl IntoView {
    let lang = crate::i18n::use_reader_lang().get();
    let numbers = &lang.ui().numbers;
    for d in &mut page.decisions {
        d.title = crate::helpers::retitle(&lang.ui().dates, &d.title, Some(&d.date_lecture));
    }
    if let Some(named) = page.court_names.get(lang.code()) {
        for d in &mut page.decisions {
            d.title = crate::helpers::renamed(named, &d.title, Some(&page.label));
        }
        page.label = named.name.clone();
    }
    let jurisdiction_type = page.jurisdiction_type;
    let page_size = i64::from(page.page_size);
    let page_count = ((page.total + page_size - 1) / page_size).max(1) as u32;
    let all = lang.ui();
    let ui = &all.hubs;
    let title = (ui.court_year_title)(&page.label, page.year, page.page);
    let total = group_thousands(numbers, page.total);
    let description = (ui.court_year_description)(&total, page.year, &page.label);
    let index = format!(
        "/decision-index/{}",
        jurisdiction_type.url_segment(Some(&page.code))
    );
    let hub_href = format!("{index}/{}", page.code);
    let year_href = format!("{hub_href}/{}", page.year);
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
        (ui.jurisdictions, format!("https://librejustice.fr{index}")),
        (&page.label, format!("https://librejustice.fr{hub_href}")),
        (&page.year.to_string(), base_url.clone()),
    ]);
    let search_href = crate::jurisdiction::use_reader_scope().seat_href(
        page.seat_path.clone(),
        decisions_href([
            ("seat", page.seat_path.clone()),
            ("dateFrom", format!("{}-01-01", page.year)),
            ("dateTo", format!("{}-12-31", page.year)),
        ]),
    );

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=canonical />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (ui.jurisdictions.to_string(), Some(index.clone())),
                    (page.label.clone(), Some(hub_href.clone())),
                    (page.year.to_string(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {(ui.court_year_heading)(&page.label, page.year)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.court_year_intro)(&total, page.year)}
                </p>
                <A href=search_href attr:class=DEEP_LINK>
                    {(ui.search_year_decisions)(page.year)}
                </A>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {page
                    .decisions
                    .into_iter()
                    .map(|d| {
                        let href = lj_dtos::decision_path(
                            jurisdiction_type,
                            Some(&page.code),
                            &d.public_id,
                        );
                        view! {
                            <li class="py-2.5">
                                <A
                                    href=href
                                    attr:class="block text-base text-[var(--color-ink)] hover:text-[var(--color-accent)]"
                                >
                                    {d.title}
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

/// Pagination par liens `<a>` crawlables : première / précédente / suivante /
/// dernière, avec la position courante.
#[component]
pub(crate) fn YearPagination(base: String, page: u32, page_count: u32) -> impl IntoView {
    let ui = &crate::i18n::use_ui().hubs;
    let href = move |p: u32| {
        if p <= 1 {
            base.clone()
        } else {
            format!("{base}?page={p}")
        }
    };
    let link_class = "rounded-lg border border-[var(--color-rule)] px-3 py-1.5 text-sm \
         text-[var(--color-ink-muted)] hover:border-[var(--color-accent)] \
         hover:text-[var(--color-accent)]";
    view! {
        {(page_count > 1)
            .then(|| {
                view! {
                    <nav aria-label=ui.pagination class="flex flex-wrap items-center gap-2">
                        {(page > 1)
                            .then(|| {
                                view! {
                                    <A href=href(1) attr:class=link_class>{ui.first}</A>
                                    <A href=href(page - 1) attr:class=link_class>{ui.previous}</A>
                                }
                            })}
                        <span class="px-2 text-sm tabular-nums text-[var(--color-ink-subtle)]">
                            {(ui.page_of)(page, page_count)}
                        </span>
                        {(page < page_count)
                            .then(|| {
                                view! {
                                    <A href=href(page + 1) attr:class=link_class>{ui.next}</A>
                                    <A href=href(page_count) attr:class=link_class>{ui.last}</A>
                                }
                            })}
                    </nav>
                }
            })}
    }
}
