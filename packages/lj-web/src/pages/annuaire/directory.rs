//! Page `/annuaire/{kind}` (ADR 0192) : listing paginé d'une catégorie, trié par
//! contentieux décroissant (côté API). Filtre barreau pour les avocats. Gabarit
//! rail + contenu commun (cf. `pages::annuaire`) : rail des catégories dans la
//! gouttière gauche, chips de catégories en mobile. Rendu SSR crawlable
//! (`PartiallyBlocked`) : head (title / description / canonical, `noindex`
//! au-delà de la page 1) + listing **bloquant** dans le HTML initial.
//! Catégorie inconnue ⇒ 404 doux (noindex).

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::EntityDirectoryResponse;

use crate::helpers::group_thousands;
use crate::pages::annuaire::common::{
    category_chips, entity_row, fetch_directory, list_skeleton, max_decision_count, stats_resource,
    status_note, AnnuaireRail, Kind, MAX_PAGES, PAGE_SIZE,
};
use crate::pages::decision_page::data::{sendable, PageError};
use crate::seo::CANONICAL_BASE;

/// Slug `kind` de la route `/annuaire/:kind`.
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

/// Filtre barreau depuis `?barreau=` (vide si absent).
fn barreau_param() -> Signal<Option<String>> {
    let query = use_query_map();
    Signal::derive(move || {
        query
            .read()
            .get("barreau")
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
    })
}

#[component]
pub fn AnnuaireDirectoryPage() -> impl IntoView {
    let slug = kind_slug();
    let page = page_param();
    let barreau = barreau_param();

    move || match Kind::from_slug(&slug.get()) {
        Some(kind) => {
            Either::Left(view! { <DirectoryLoaded kind=kind page=page barreau=barreau /> })
        }
        None => Either::Right(view! { <DirectoryUnknown /> }),
    }
}

#[component]
fn DirectoryLoaded(
    kind: Kind,
    page: Signal<i64>,
    barreau: Signal<Option<String>>,
) -> impl IntoView {
    let stats = stats_resource();
    let title = format!("Annuaire des {} - LibreJustice", plural_lower(kind));
    let description = format!(
        "{} au registre, classés par volume de contentieux dans la jurisprudence \
         française. {}",
        kind.plural(),
        kind.tagline()
    );
    let base = format!("{CANONICAL_BASE}/annuaire/{}", kind.slug());
    let canonical = base.clone();
    // Convention listing : indexer la page 1, `noindex` au-delà.
    let robots = move || (page.get() > 1).then_some("noindex");

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=canonical />
        {move || robots().map(|r| view! { <Meta name="robots" content=r /> })}

        <div class="mx-auto w-full max-w-[92rem] flex-1 px-4 py-8 sm:px-6 lg:px-8">
            // Gabarit commun /decisions · /textes · /texte : gouttière 240px,
            // colonne contenu bornée 3xl.
            <div class="grid gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                <div class="hidden lg:block">
                    <AnnuaireRail current=kind stats=stats />
                </div>
                <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
                    <header class="flex flex-col gap-2">
                        <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                            <A
                                href="/annuaire"
                                attr:class="text-[var(--color-ink-subtle)] no-underline hover:text-[var(--color-accent)]"
                            >
                                "Annuaire"
                            </A>
                        </p>
                        <h1 class="font-sans text-3xl text-[var(--color-ink)]">{kind.plural()}</h1>
                        <p class="max-w-prose text-[var(--color-ink-muted)]">{kind.tagline()}</p>
                    </header>

                    // Sous `lg`, le rail est masqué : les chips donnent la
                    // navigation entre catégories.
                    {category_chips(Some(kind))}

                    {kind.has_barreau_filter().then(|| view! { <BarreauFilter kind=kind barreau=barreau /> })}

                    <DirectoryList kind=kind page=page barreau=barreau />
                </div>
            </div>
        </div>
    }
}

/// Filtre barreau (avocats) : formulaire GET natif vers `/annuaire/avocats`.
#[component]
fn BarreauFilter(kind: Kind, barreau: Signal<Option<String>>) -> impl IntoView {
    let action = format!("/annuaire/{}", kind.slug());
    let clear_href = action.clone();
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <form method="get" action=action role="search" class="flex min-w-0 flex-1 gap-2">
                <input
                    type="search"
                    name="barreau"
                    value=move || barreau.get().unwrap_or_default()
                    placeholder="Filtrer par barreau (ex. « paris »)…"
                    autocomplete="off"
                    aria-label="Filtrer par barreau"
                    class="min-w-0 flex-1 rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                />
                <button
                    type="submit"
                    class="shrink-0 rounded-lg border border-[var(--color-rule)] px-4 py-2 text-sm text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                >
                    "Filtrer"
                </button>
            </form>
            {move || {
                barreau
                    .get()
                    .map(|_| {
                        view! {
                            <A
                                href=clear_href.clone()
                                attr:class="shrink-0 text-sm text-[var(--color-ink-subtle)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                            >
                                "Réinitialiser"
                            </A>
                        }
                    })
            }}
        </div>
    }
}

/// Listing paginé, bloquant SSR (contenu dans le HTML initial, crawlable),
/// rechargé à chaque `?page=` / `?barreau=`. Erreur API rendue en note sobre.
#[component]
fn DirectoryList(kind: Kind, page: Signal<i64>, barreau: Signal<Option<String>>) -> impl IntoView {
    let slug = kind.slug();
    let listing = Resource::new_blocking(
        move || (page.get(), barreau.get()),
        move |(page, barreau)| sendable(fetch_directory(slug, barreau, page)),
    );

    view! {
        <Suspense fallback=list_skeleton>
            {move || Suspend::new(async move {
                directory_view(listing.await, slug, page.get(), barreau.get())
            })}
        </Suspense>
    }
}

fn directory_view(
    result: Result<EntityDirectoryResponse, PageError>,
    slug: &'static str,
    page: i64,
    barreau: Option<String>,
) -> AnyView {
    let response = match result {
        Ok(response) => {
            set_cache_control(200);
            response
        }
        Err(err) => {
            set_cache_control(err.status);
            return status_note(format!("Listing indisponible ({}).", err.message));
        }
    };
    if response.items.is_empty() {
        return status_note("Aucune entité dans cette catégorie pour l'instant.");
    }

    let total = response.total;
    // Pagination bornée au plafond de profondeur de l'API (ADR 0239).
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).clamp(1, MAX_PAGES);
    let count_label = match total {
        1 => "1 entité au registre".to_string(),
        n => format!("{} entités au registre", group_thousands(n)),
    };
    // Sous-ensemble « en justice » en contexte (ADR 0233/0239).
    let contentieux_label = (response.contentieux > 0)
        .then(|| format!("dont {} en justice", group_thousands(response.contentieux)));
    let max = max_decision_count(&response.items);
    let rows = response
        .items
        .into_iter()
        .map(|item| entity_row(item, max))
        .collect_view();
    let pagination = pagination_view(slug, barreau.as_deref(), page, total_pages);

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
            <span class="normal-case tracking-normal">"classées par volume de contentieux"</span>
        </p>
        <ul class="mt-4 flex flex-col gap-3">{rows}</ul>
        {pagination}
    }
    .into_any()
}

/// Href d'une page en préservant le filtre barreau.
fn page_href(slug: &str, barreau: Option<&str>, page: i64) -> String {
    match barreau {
        Some(b) => format!("/annuaire/{slug}?barreau={}&page={page}", encode_param(b)),
        None => format!("/annuaire/{slug}?page={page}"),
    }
}

/// Percent-encode minimal d'une valeur de query (espaces + caractères non
/// `A-Za-z0-9-._~`). Suffisant pour un slug de barreau réinjecté dans un href.
fn encode_param(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Pagination `<A>` (précédent / position / suivant), SSR-friendly (`?page=`).
fn pagination_view(
    slug: &str,
    barreau: Option<&str>,
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
                href=page_href(slug, barreau, page - 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                <span aria-hidden="true">"←"</span>
                "Précédent"
            </A>
        }
        .into_any()
    });
    let next = (page < total_pages).then(|| {
        view! {
            <A
                href=page_href(slug, barreau, page + 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                "Suivant"
                <span aria-hidden="true">"→"</span>
            </A>
        }
        .into_any()
    });
    Some(
        view! {
            <nav
                aria-label="Pagination"
                class="mt-6 flex items-center justify-between border-t border-[var(--color-rule)] pt-4"
            >
                <div class="min-w-0">{prev}</div>
                <span class="text-xs tabular-nums text-[var(--color-ink-subtle)]">
                    {format!("Page {page} sur {total_pages}")}
                </span>
                <div class="min-w-0 text-right">{next}</div>
            </nav>
        }
        .into_any(),
    )
}

/// Catégorie inconnue : 404 doux (message + noindex), parité `EntityError`.
#[component]
fn DirectoryUnknown() -> impl IntoView {
    mark_not_found();
    view! {
        <Title text="Catégorie introuvable - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Introuvable"
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">
                "Cette catégorie d'annuaire n'existe pas"
            </h1>
            <p class="mt-4 text-sm text-[var(--color-ink-muted)]">
                <A
                    href="/annuaire"
                    attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                >
                    "Retour à l'annuaire"
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

/// Nom pluriel minuscule pour le title (« annuaire des entreprises… »).
fn plural_lower(kind: Kind) -> &'static str {
    match kind {
        Kind::Entreprises => "entreprises",
        Kind::PersonnesPubliques => "personnes publiques",
        Kind::Associations => "associations",
        Kind::Avocats => "avocats",
        Kind::Cabinets => "cabinets d'avocats",
    }
}
