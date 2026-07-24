//! Hubs juridiction (ADR 0253) — l'arborescence navigable vers les décisions :
//! `/juridictions` (catalogue par famille) → `/juridiction/{code}` (années)
//! → `/juridiction/{code}/{annee}?page=N` (liste paginée de liens décision).
//! Pages SSR crawlables : chaque décision est à quelques clics de la home.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{
    JurisdictionCatalogueResponse, JurisdictionHubResponse, JurisdictionTypeGroup,
    JurisdictionYearResponse,
};

use crate::api::ApiClient;
use crate::pages::law_page::data::{sendable, PageError};

/// Nombre avec séparateur de milliers FR (espace fine insécable).
pub(crate) fn fmt_count(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// JSON-LD `BreadcrumbList` schema.org d'un fil d'Ariane (label, url absolue).
/// Partagé avec la page décision (maillage retour, ADR 0253).
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
    view! {
        <Title text="Page introuvable - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Erreur"
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{err.message}</h1>
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

/// Fil d'Ariane visible des pages hub.
#[component]
pub(crate) fn HubBreadcrumb(items: Vec<(String, Option<String>)>) -> impl IntoView {
    view! {
        <nav aria-label="Fil d'Ariane" class="flex flex-wrap items-center gap-1.5 text-sm text-[var(--color-ink-subtle)]">
            {items
                .into_iter()
                .enumerate()
                .map(|(i, (label, href))| {
                    view! {
                        {(i > 0).then(|| view! { <span aria-hidden="true">"›"</span> })}
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
// /juridictions — catalogue par famille
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionsPage() -> impl IntoView {
    let catalogue = Resource::new_blocking(
        || (),
        |_| {
            sendable(async {
                ApiClient::from_context()
                    .fetch_juridictions()
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
                        Either::Left(view! { <JuridictionsLoaded response=r /> })
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
fn JuridictionsLoaded(response: JurisdictionCatalogueResponse) -> impl IntoView {
    let title = "Juridictions - LibreJustice";
    let description = "Toutes les juridictions du corpus — Conseil d'État, Cour de cassation, \
         cours d'appel, tribunaux administratifs et judiciaires, cours européennes — avec \
         leurs décisions par année.";
    let url = "https://librejustice.fr/juridictions".to_string();
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Juridictions", url.clone()),
    ]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Jurisprudence"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Juridictions"</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    "Parcourez les décisions du corpus par juridiction, puis par année. "
                    "Chaque juridiction ouvre la liste complète de ses décisions."
                </p>
            </header>

            {response
                .groups
                .into_iter()
                .map(|group| view! { <JuridictionGroup group=group /> })
                .collect_view()}
        </div>
    }
}

#[component]
fn JuridictionGroup(group: JurisdictionTypeGroup) -> impl IntoView {
    view! {
        <section class="flex flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {group.label}
            </h2>
            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {group
                    .jurisdictions
                    .into_iter()
                    .map(|j| {
                        let href = format!("/juridiction/{}", j.code);
                        let count = format!(
                            "{} décision{}",
                            fmt_count(j.decision_count),
                            if j.decision_count > 1 { "s" } else { "" },
                        );
                        view! {
                            <li class="py-2.5">
                                <A
                                    href=href
                                    attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1"
                                >
                                    <span class="text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                                        {j.label}
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
// /juridiction/{code} — hub d'une juridiction (années)
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionHubPage() -> impl IntoView {
    let params = use_params_map();
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
                match hub.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <JuridictionHubLoaded hub=r /> })
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
fn JuridictionHubLoaded(hub: JurisdictionHubResponse) -> impl IntoView {
    let title = format!("{} : décisions - LibreJustice", hub.label);
    let description = format!(
        "Les {} décisions de la juridiction « {} », par année, en texte intégral.",
        fmt_count(hub.decision_count),
        hub.label,
    );
    let url = format!("https://librejustice.fr/juridiction/{}", hub.code);
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        (
            "Juridictions",
            "https://librejustice.fr/juridictions".to_string(),
        ),
        (&hub.label, url.clone()),
    ]);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Juridictions".to_string(), Some("/juridictions".to_string())),
                    (hub.label.clone(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {hub.type_label.clone()}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{hub.label.clone()}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {format!(
                        "{} décisions en texte intégral, de {} à {}.",
                        fmt_count(hub.decision_count),
                        hub.years.last().map(|y| y.year).unwrap_or_default(),
                        hub.years.first().map(|y| y.year).unwrap_or_default(),
                    )}
                </p>
            </header>

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Décisions par année"
                </h2>
                <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                    {hub
                        .years
                        .into_iter()
                        .map(|y| {
                            let href = format!("/juridiction/{}/{}", hub.code, y.year);
                            let count = format!(
                                "{} décision{}",
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
// /juridiction/{code}/{annee} — liste paginée des décisions d'une année
// ---------------------------------------------------------------------------

#[component]
pub fn JuridictionYearPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
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
        |(code, annee, page)| {
            sendable(async move {
                let year: i32 = annee.parse().map_err(|_| PageError {
                    status: 404,
                    message: "Année invalide".to_string(),
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
                match page_data.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <JuridictionYearLoaded page=r /> })
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
fn JuridictionYearLoaded(page: JurisdictionYearResponse) -> impl IntoView {
    let page_size = i64::from(page.page_size);
    let page_count = ((page.total + page_size - 1) / page_size).max(1) as u32;
    let title = if page.page > 1 {
        format!(
            "{} : décisions {} (page {}) - LibreJustice",
            page.label, page.year, page.page,
        )
    } else {
        format!("{} : décisions {} - LibreJustice", page.label, page.year)
    };
    let description = format!(
        "Les {} décisions rendues en {} par « {} », en texte intégral.",
        fmt_count(page.total),
        page.year,
        page.label,
    );
    let base_url = format!(
        "https://librejustice.fr/juridiction/{}/{}",
        page.code, page.year,
    );
    // Canonical par page (?page=N au-delà de la première) : chaque page de la
    // pagination est un document distinct, pas un doublon de la première.
    let canonical = if page.page > 1 {
        format!("{base_url}?page={}", page.page)
    } else {
        base_url.clone()
    };
    let hub_href = format!("/juridiction/{}", page.code);
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        (
            "Juridictions",
            "https://librejustice.fr/juridictions".to_string(),
        ),
        (
            &page.label,
            format!("https://librejustice.fr/juridiction/{}", page.code),
        ),
        (&page.year.to_string(), base_url.clone()),
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
                    ("Juridictions".to_string(), Some("/juridictions".to_string())),
                    (page.label.clone(), Some(hub_href.clone())),
                    (page.year.to_string(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {format!("{} — décisions {}", page.label, page.year)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {format!(
                        "{} décisions rendues en {}, de la plus récente à la plus ancienne.",
                        fmt_count(page.total),
                        page.year,
                    )}
                </p>
            </header>

            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {page
                    .decisions
                    .into_iter()
                    .map(|d| {
                        let href = format!("/decision/{}", d.public_id);
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
                base=format!("/juridiction/{}/{}", page.code, page.year)
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
                    <nav aria-label="Pagination" class="flex flex-wrap items-center gap-2">
                        {(page > 1)
                            .then(|| {
                                view! {
                                    <A href=href(1) attr:class=link_class>"Première"</A>
                                    <A href=href(page - 1) attr:class=link_class>"Précédente"</A>
                                }
                            })}
                        <span class="px-2 text-sm tabular-nums text-[var(--color-ink-subtle)]">
                            {format!("Page {page} / {page_count}")}
                        </span>
                        {(page < page_count)
                            .then(|| {
                                view! {
                                    <A href=href(page + 1) attr:class=link_class>"Suivante"</A>
                                    <A href=href(page_count) attr:class=link_class>"Dernière"</A>
                                }
                            })}
                    </nav>
                }
            })}
    }
}
