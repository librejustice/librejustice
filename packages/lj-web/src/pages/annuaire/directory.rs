//! Page `/entity-index/{juris}/{kind}`: a paginated listing for legal persons, blocking in
//! the SSR HTML and `noindex` past page 1.
//!
//! [`Kind::names_natural_persons`] takes the other branch: a name search with no
//! listing, `noindex` from page 1. SSR is anonymous there, so the API refuses it
//! and results only load from the client, where the session cookie rides along.

use leptos::either::EitherOf3;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{EntityDirectoryItemDto, EntityDirectoryResponse};

use crate::components::layout::{CONTENT_COL, GRID_2COL, SHELL};
use crate::components::own_page_description;
use crate::i18n::DirectoryUi;
#[cfg(feature = "hydrate")]
use crate::pages::annuaire::common::fetch_named;
use crate::pages::annuaire::common::{
    category_chips, entity_row, fetch_directory, list_skeleton, max_decision_count, stats_resource,
    status_note, AnnuaireRail, Kind, MAX_PAGES, PAGE_SIZE, SEARCH_MIN_QUERY,
};
use crate::pages::decision_page::data::{sendable, PageError};
use crate::seo::CANONICAL_BASE;

/// Slug `kind` de la route `/entity-index/:juris/:kind`.
fn kind_slug() -> Signal<String> {
    let params = use_params_map();
    Signal::derive(move || params.read().get("kind").unwrap_or_default())
}

/// Page courante depuis `?page=` (1 par défaut ; `< 1` ramené à 1).
fn page_param() -> Signal<i64> {
    let query = use_query_map();
    Signal::derive(move || {
        query
            .read()
            .get("page")
            .and_then(|p| p.parse::<i64>().ok())
            .filter(|&p| p >= 1)
            .unwrap_or(1)
    })
}

/// Name search term from `?q=`, empty when absent.
fn query_term() -> Signal<String> {
    let query = use_query_map();
    Signal::derive(move || query.read().get("q").unwrap_or_default().trim().to_string())
}

#[component]
pub fn AnnuaireDirectoryPage() -> impl IntoView {
    own_page_description();

    let slug = kind_slug();
    let page = page_param();
    let term = query_term();

    move || match Kind::from_slug(&slug.get()) {
        Some(kind) if kind.names_natural_persons() => {
            EitherOf3::A(view! { <NamedSearchPage kind=kind term=term /> })
        }
        Some(kind) => EitherOf3::B(view! { <DirectoryLoaded kind=kind page=page /> }),
        None => EitherOf3::C(view! { <DirectoryUnknown /> }),
    }
}

#[component]
fn DirectoryLoaded(kind: Kind, page: Signal<i64>) -> impl IntoView {
    let stats = stats_resource();
    let ui = &crate::i18n::use_ui().directory;
    let copy = kind.copy(ui);
    let title = (ui.listing_title)(copy.plural_lower);
    let description = (ui.listing_description)(copy.plural, copy.tagline);
    let base = format!("{CANONICAL_BASE}/entity-index/fr/{}", kind.slug());
    let canonical = base.clone();
    // Convention listing : indexer la page 1, `noindex` au-delà.
    let robots = move || (page.get() > 1).then_some("noindex");

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=canonical />
        {move || robots().map(|r| view! { <Meta name="robots" content=r /> })}

        <div class=format!("{SHELL} flex-1")>
            // Gabarit commun /decisions · /textes · /texte : gouttière 240px, colonne
            // contenu bornée 3xl.
            <div class=GRID_2COL>
                <div class="hidden lg:block">
                    <AnnuaireRail current=kind stats=stats />
                </div>
                <div class=CONTENT_COL>
                    <header class="flex flex-col gap-2">
                        <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                            <A
                                href=crate::i18n::use_reader_lang().path("/entity-search")
                                attr:class="text-[var(--color-ink-subtle)] no-underline hover:text-[var(--color-accent)]"
                            >
                                {ui.directory}
                            </A>
                        </p>
                        <h1 class="font-sans text-3xl text-[var(--color-ink)]">{copy.plural}</h1>
                        <p class="max-w-prose text-[var(--color-ink-muted)]">{copy.tagline}</p>
                    </header>

                    {category_chips(ui, Some(kind))}

                    <DirectoryList kind=kind page=page />
                </div>
            </div>
        </div>
    }
}

/// Blocking listing: in the initial HTML so a crawler reads it, reloaded on each
/// `?page=`.
#[component]
fn DirectoryList(kind: Kind, page: Signal<i64>) -> impl IntoView {
    let slug = kind.slug();
    let ui = &crate::i18n::use_ui().directory;
    let listing = Resource::new_blocking(
        move || page.get(),
        move |page| sendable(fetch_directory(slug, page)),
    );

    view! {
        <Suspense fallback=list_skeleton>
            {move || Suspend::new(async move {
                directory_view(ui, listing.await, slug, page.get())
            })}
        </Suspense>
    }
}

fn directory_view(
    ui: &'static DirectoryUi,
    result: Result<EntityDirectoryResponse, PageError>,
    slug: &'static str,
    page: i64,
) -> AnyView {
    let response = match result {
        Ok(response) => {
            set_cache_control(200);
            response
        }
        Err(err) => {
            set_cache_control(err.status);
            return status_note((ui.listing_unavailable)(
                &err.failure.text(&crate::i18n::use_ui().shell),
            ));
        }
    };
    if response.items.is_empty() {
        return status_note(ui.empty_category);
    }

    let total = response.total;
    // Pagination bornée au plafond de profondeur de l'API.
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).clamp(1, MAX_PAGES);
    let count_label = (ui.register_count)(total);
    let contentieux_label = (response.contentieux > 0).then(|| (ui.in_court)(response.contentieux));
    let max = max_decision_count(&response.items);
    let rows = response
        .items
        .into_iter()
        .map(|item| entity_row(ui, item, max))
        .collect_view();
    let pagination = pagination_view(ui, slug, page, total_pages);

    view! {
        <p class="flex flex-wrap items-baseline gap-2 text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
            <span>{count_label}</span>
            {contentieux_label
                .map(|label| {
                    view! {
                        <span aria-hidden="true">"·"</span>
                        <span class="normal-case tracking-normal tabular-nums">{label}</span>
                    }
                })}
            <span aria-hidden="true">"·"</span>
            <span class="normal-case tracking-normal">{ui.sorted_by_decisions}</span>
        </p>
        <ul class="mt-4 flex flex-col gap-3">{rows}</ul>
        {pagination}
    }
    .into_any()
}

fn page_href(slug: &str, page: i64) -> String {
    format!("/entity-index/fr/{slug}?page={page}")
}

/// Pagination `<A>` (précédent / position / suivant), SSR-friendly (`?page=`).
fn pagination_view(
    ui: &'static DirectoryUi,
    slug: &str,
    page: i64,
    total_pages: i64,
) -> Option<AnyView> {
    if total_pages <= 1 {
        return None;
    }
    let page = page.min(total_pages);
    let prev = (page > 1).then(|| {
        view! {
            <A
                href=page_href(slug, page - 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                <span aria-hidden="true">"←"</span>
                {ui.previous}
            </A>
        }
        .into_any()
    });
    let next = (page < total_pages).then(|| {
        view! {
            <A
                href=page_href(slug, page + 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                {ui.next}
                <span aria-hidden="true">"→"</span>
            </A>
        }
        .into_any()
    });
    Some(
        view! {
            <nav
                aria-label=ui.pagination
                class="mt-6 flex items-center justify-between border-t border-[var(--color-rule)] pt-4"
            >
                <div class="min-w-0">{prev}</div>
                <span class="text-xs tabular-nums text-[var(--color-ink-subtle)]">
                    {(ui.page_of)(page, total_pages)}
                </span>
                <div class="min-w-0 text-right">{next}</div>
            </nav>
        }
        .into_any(),
    )
}

/// `Loading` is what SSR renders: the request only fires on hydration, the one
/// pass that carries the cookie.
#[derive(Clone)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
enum NamedState {
    Idle,
    Loading,
    Anonymous,
    Failed(crate::api::Failure),
    Loaded(Vec<EntityDirectoryItemDto>),
}

#[cfg(feature = "hydrate")]
fn spawn_named_search(slug: &'static str, q: String, state: RwSignal<NamedState>) {
    leptos::task::spawn_local(async move {
        match fetch_named(slug, q.clone()).await {
            Ok(items) => {
                crate::reads::report_entity_search(q);
                state.set(NamedState::Loaded(items));
            }
            Err(err) if err.status == 401 => state.set(NamedState::Anonymous),
            Err(err) => state.set(NamedState::Failed(err.failure)),
        }
    });
}

#[cfg(not(feature = "hydrate"))]
fn spawn_named_search(_slug: &'static str, _q: String, _state: RwSignal<NamedState>) {}

fn named_results(kind: Kind, term: Signal<String>) -> RwSignal<NamedState> {
    let state = RwSignal::new(NamedState::Idle);
    let slug = kind.slug();
    Effect::new(move |_| {
        let q = term.get();
        if q.chars().count() < SEARCH_MIN_QUERY {
            state.set(NamedState::Idle);
            return;
        }
        state.set(NamedState::Loading);
        spawn_named_search(slug, q, state);
    });
    state
}

/// Natural-person category: name search, no listing.
#[component]
fn NamedSearchPage(kind: Kind, term: Signal<String>) -> impl IntoView {
    let stats = stats_resource();
    let state = named_results(kind, term);
    let ui = &crate::i18n::use_ui().directory;
    let copy = kind.copy(ui);
    let title = (ui.named_title)(copy.singular_lower);
    let description = (ui.named_description)(copy.singular_lower, copy.tagline);
    let canonical = format!("{CANONICAL_BASE}/entity-index/fr/{}", kind.slug());
    let action = format!("/entity-index/fr/{}", kind.slug());
    let login_href = format!("/login?next=%2Fannuaire%2F{}", kind.slug());
    let placeholder = (ui.named_placeholder)(copy.singular_lower);
    let aria = (ui.named_label)(copy.singular_lower);
    let empty = (ui.named_empty)(copy.singular_lower);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=canonical />
        <Meta name="robots" content="noindex" />

        <div class=format!("{SHELL} flex-1")>
            <div class=GRID_2COL>
                <div class="hidden lg:block">
                    <AnnuaireRail current=kind stats=stats />
                </div>
                <div class=CONTENT_COL>
                    <header class="flex flex-col gap-2">
                        <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                            <A
                                href=crate::i18n::use_reader_lang().path("/entity-search")
                                attr:class="text-[var(--color-ink-subtle)] no-underline hover:text-[var(--color-accent)]"
                            >
                                {ui.directory}
                            </A>
                        </p>
                        <h1 class="font-sans text-3xl text-[var(--color-ink)]">{copy.plural}</h1>
                    </header>

                    {category_chips(ui, Some(kind))}

                    <form method="get" action=action role="search" class="flex w-full gap-2">
                        <input
                            type="search"
                            name="q"
                            value=move || term.get()
                            placeholder=placeholder
                            autocomplete="off"
                            aria-label=aria
                            class="min-w-0 flex-1 rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                        <button
                            type="submit"
                            class="shrink-0 rounded-lg border border-[var(--color-accent)] bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                        >
                            {ui.search}
                        </button>
                    </form>

                    {move || named_view(ui, state.get(), &login_href, &empty)}
                </div>
            </div>
        </div>
    }
}

fn named_view(
    ui: &'static DirectoryUi,
    state: NamedState,
    login_href: &str,
    empty: &str,
) -> AnyView {
    match state {
        NamedState::Idle => status_note(ui.min_name),
        NamedState::Loading => list_skeleton().into_any(),
        NamedState::Anonymous => view! {
            <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">
                <A
                    href=login_href.to_string()
                    attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                >
                    {ui.sign_in}
                </A>
                {ui.sign_in_to_search}
            </p>
        }
        .into_any(),
        NamedState::Failed(failure) => status_note((ui.search_unavailable)(
            &failure.text(&crate::i18n::use_ui().shell),
        )),
        NamedState::Loaded(items) if items.is_empty() => status_note(empty.to_string()),
        NamedState::Loaded(items) => {
            let max = max_decision_count(&items);
            let rows = items
                .into_iter()
                .map(|item| entity_row(ui, item, max))
                .collect_view();
            view! { <ul class="mt-4 flex flex-col gap-3">{rows}</ul> }.into_any()
        }
    }
}

/// Catégorie inconnue : 404 doux (message + noindex), parité `EntityError`.
#[component]
fn DirectoryUnknown() -> impl IntoView {
    mark_not_found();
    crate::pages::not_found::signaler_lien_mort();
    let all = crate::i18n::use_ui();
    let ui = &all.directory;
    view! {
        <Title text=ui.unknown_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {all.shell.not_found}
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{ui.unknown_heading}</h1>
            <p class="mt-4 text-sm text-[var(--color-ink-muted)]">
                <A
                    href=crate::i18n::use_reader_lang().path("/entity-search")
                    attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                >
                    {ui.back}
                </A>
            </p>
        </div>
    }
}

/// Statut HTTP 404 SSR + cache court pour une catégorie inconnue (parité 404 /
/// pages décision/loi). No-op côté hydrate.
#[cfg(feature = "ssr")]
fn mark_not_found() {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        resp.set_status(StatusCode::NOT_FOUND);
        if let Ok(hv) = HeaderValue::from_str("public, max-age=0, s-maxage=300") {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn mark_not_found() {}

/// `Cache-Control` du listing (aligné entity/law/codes : 200 → 7 j au CDN,
/// 4xx → 5 min, 5xx → no-store). No-op côté hydrate.
#[cfg(feature = "ssr")]
fn set_cache_control(status: u16) {
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
fn set_cache_control(_status: u16) {}
