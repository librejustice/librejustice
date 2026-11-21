//! Annonces pot: the legal events under way.
//!
//! Three routes, two regimes:
//!
//! - `/{juris}/journal-search` is the **engine page**, sister of `/decision-search` and
//!   `/norm-search`: same template, same filter bar, same facet mechanics, rendered
//!   client side (`ClientOnly`). The body lives in
//!   `components/search/journal_results.rs`.
//! - `/journal-index/fr`, `/journal-index/fr/{code}` and `/notice/fr/{uid}` are the
//!   **crawl pages**: SSR, no JS, in the sitemap. That is where the fond gets
//!   found, the engine page being a shell like its sisters; the catalogue plays for
//!   notices the role `/decision-index/fr` plays for decisions. Only the `marche`
//!   fond appears there: the proxied DILA views are not offered to the crawl.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use lj_dtos::{
    AnnonceCatalogueResponse, AnnonceDetailResponse, FacetTag, JournalHit, JournalSearchRequest,
    JournalSearchResponse, JournalSort,
};
use lj_vocab::{zone_est_departement, JOURNAL_FOND_MARCHE};

use crate::api::{ApiClient, Failure};
use crate::components::client_only::ClientOnly;
use crate::components::layout::{CONTENT_COL, GRID_3COL, SHELL};
use crate::components::own_page_description;
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::journal_results::{description, numeric_date};
use crate::components::search::JournalsView;
use crate::helpers::group_thousands;
use crate::helpers::portee_zones;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, set_cache_control, HubBreadcrumb, HubError, HubSkeleton, YearPagination,
};
use crate::pages::law_page::data::{sendable, PageError};

/// Annonces par page sur les pages de crawl.
const PAGE_SIZE: u32 = 20;
const BASE: &str = "https://librejustice.fr";

// ── /{juris}/journal-search : la page moteur ─────────────────────────────────────────

#[component]
pub fn JournalsPage() -> impl IntoView {
    let ui = &crate::i18n::use_ui().notices;
    let query_map = use_query_map();
    // `try_get` : `leptos_meta` réévalue cette clôture après le démontage de la
    // route, quand `query_map` est déjà libéré.
    let title = move || {
        let q = query_map
            .try_get()
            .and_then(|params| params.get("q"))
            .unwrap_or_default();
        let q = q.trim();
        if q.is_empty() {
            ui.title.to_string()
        } else {
            (ui.title_for)(q)
        }
    };
    view! {
        <Title text=title />
        <ClientOnly fallback=|| {
            view! { <JournalsPageSkeleton /> }
        }>
            <JournalsPageBody />
        </ClientOnly>
    }
}

/// Squelette du fallback SSR/1er rendu : même grille que le corps, aucun signal
/// dépendant de l'URL ou du navigateur → hydratation sans divergence.
#[component]
fn JournalsPageSkeleton() -> impl IntoView {
    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().notices.heading}</h1>
            <div class=GRID_3COL>
                <div class="hidden lg:block"></div>
                <div class=CONTENT_COL>
                    <div class="h-12 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/40"></div>
                    <div class="flex flex-col gap-4">
                        {(0..4)
                            .map(|_| {
                                view! {
                                    <div class="h-20 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/30"></div>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn JournalsPageBody() -> impl IntoView {
    let query_map = use_query_map();
    // Texte de la barre partagé avec les filtres, comme sur les pages sœurs.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));
    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().notices.heading}</h1>
            <JournalsView />
        </div>
    }
}

// ── /journal-index/fr : le catalogue de crawl ───────────────────────────

#[component]
pub fn JournalsCataloguePage() -> impl IntoView {
    own_page_description();
    let catalogue = Resource::new_blocking(
        || (),
        |()| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_journals_catalogue()
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match catalogue.await {
                    Ok(c) => {
                        set_cache_control(200);
                        Either::Left(view! { <CatalogueLoaded catalogue=c /> })
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
fn CatalogueLoaded(catalogue: AnnonceCatalogueResponse) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let shell = &crate::i18n::use_ui().shell;
    let ui = &crate::i18n::use_ui().notices;
    let total = group_thousands(numbers, catalogue.total);
    let description = (ui.index_description)(&total);
    let jsonld = breadcrumb_jsonld(&[
        (shell.home, format!("{BASE}/")),
        (shell.notices, format!("{BASE}/fr/journal-search")),
        (ui.by_department, format!("{BASE}/journal-index/fr")),
    ]);

    view! {
        <Title text=ui.index_title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=format!("{BASE}/journal-index/fr") />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (shell.notices.to_string(), Some("/fr/journal-search".to_string())),
                    (ui.by_department.to_string(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.index_heading}</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">{(ui.index_intro)(&total)}</p>
                <p class="text-sm">
                    <A
                        href=crate::i18n::use_reader_lang().path("/fr/journal-search")
                        attr:class="text-[var(--color-accent)] hover:underline"
                    >
                        {ui.search_all}
                    </A>
                </p>
            </header>

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.departments}
                </h2>
                <ul class="grid grid-cols-2 gap-x-6 gap-y-1 sm:grid-cols-3 lg:grid-cols-4">
                    {catalogue
                        .zones
                        .into_iter()
                        .map(|d| {
                            view! {
                                <li>
                                    <A
                                        href=format!("/journal-index/fr/{}", d.code)
                                        attr:class="flex items-baseline justify-between gap-2 text-sm text-[var(--color-ink-muted)] hover:text-[var(--color-accent)]"
                                    >
                                        <span>{d.label}</span>
                                        <span class="tabular-nums text-[var(--color-ink-subtle)]">
                                            {group_thousands(numbers, d.count)}
                                        </span>
                                    </A>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </section>

            {(!catalogue.familles.is_empty())
                .then(|| {
                    view! {
                        <section class="flex flex-col gap-3 border-t border-[var(--color-rule)] pt-6">
                            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                                {ui.by_nature}
                            </h2>
                            <ul class="flex flex-wrap gap-x-5 gap-y-1">
                                {catalogue
                                    .familles
                                    .into_iter()
                                    .map(|f| {
                                        view! {
                                            <li>
                                                <A
                                                    href=format!(
                                                        "/fr/journal-search?fond={JOURNAL_FOND_MARCHE}&cpv=nature_marche:{}",
                                                        f.value,
                                                    )
                                                    attr:class="text-sm text-[var(--color-ink-muted)] hover:text-[var(--color-accent)]"
                                                >
                                                    {format!("{} ({})", f.label, group_thousands(numbers, f.count))}
                                                </A>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                        </section>
                    }
                })}
        </div>
    }
}

// ── /journal-index/fr/{code} : page de crawl ────────────────────────────

#[component]
pub fn JournalsDepartementPage() -> impl IntoView {
    own_page_description();
    let ui = &crate::i18n::use_ui().notices;
    let params = use_params_map();
    let query = use_query_map();
    let liste = Resource::new_blocking(
        move || {
            let code = params.get().get("code").unwrap_or_default().to_uppercase();
            let page: u32 = query
                .get()
                .get("page")
                .and_then(|p| p.parse().ok())
                .filter(|p| *p >= 1)
                .unwrap_or(1);
            (code, page)
        },
        move |(code, numero)| {
            sendable(async move {
                // La page de crawl ne sert qu'un fond : les consultations de
                // marchés ouvertes du département, par échéance. Les filtres
                // libres vivent sur la page moteur.
                let req = JournalSearchRequest {
                    fond: vec![JOURNAL_FOND_MARCHE.to_string()],
                    zone: vec![code.clone()],
                    sort: JournalSort::Echeance,
                    limit: PAGE_SIZE,
                    offset: (numero - 1) * PAGE_SIZE,
                    ..JournalSearchRequest::default()
                };
                let page = ApiClient::from_context()
                    .search_journals(&req)
                    .await
                    .map_err(PageError::from)?;
                // Un département sans aucune consultation ouverte n'est pas une
                // page : 404 plutôt qu'une liste vide indexable.
                if page.total == 0 {
                    return Err(PageError {
                        status: 404,
                        failure: Failure::Said(ui.department_empty.to_string()),
                    });
                }
                Ok::<_, PageError>((code, numero, page))
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match liste.await {
                    Ok((code, numero, page)) => {
                        set_cache_control(200);
                        Either::Left(
                            view! { <DepartementLoaded code=code numero=numero page=page /> },
                        )
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
fn DepartementLoaded(code: String, numero: u32, page: JournalSearchResponse) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let hits: Vec<JournalHit> = page.groupes.into_iter().flat_map(|g| g.hits).collect();
    // Le nom du département vient de l'API (les hits le portent résolu).
    let nom = hits
        .first()
        .and_then(|h| h.zones.iter().find(|z| z.key == code))
        .map(|z| z.label.clone())
        .unwrap_or_else(|| code.clone());
    let shell = &crate::i18n::use_ui().shell;
    let ui = &crate::i18n::use_ui().notices;
    let base_path = format!("/journal-index/fr/{code}");
    let title = (ui.department_title)(&nom, numero);
    let description = (ui.department_description)(&group_thousands(numbers, page.total), &nom);
    let canonical = if numero > 1 {
        format!("{BASE}{base_path}?page={numero}")
    } else {
        format!("{BASE}{base_path}")
    };
    let filtre = format!("?fond={JOURNAL_FOND_MARCHE}&zone={code}");
    let jsonld = breadcrumb_jsonld(&[
        (shell.home, format!("{BASE}/")),
        (shell.notices, format!("{BASE}/fr/journal-search")),
        (&nom, format!("{BASE}{base_path}")),
    ]);
    let taille = i64::from(PAGE_SIZE);
    let total_pages = ((page.total + taille - 1) / taille).max(1) as u32;

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=canonical />
        <Link rel="alternate" type_="application/rss+xml" href=format!("{BASE}/journal-search.rss{filtre}") />
        <Link rel="alternate" type_="text/calendar" href=format!("{BASE}/journal-search.ics{filtre}") />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    (shell.notices.to_string(), Some("/fr/journal-search".to_string())),
                    (nom.clone(), None),
                ] />
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {(ui.department_heading)(&nom)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.department_intro)(&group_thousands(numbers, page.total))}
                </p>
                <p class="text-sm">
                    <A
                        href=format!("/fr/journal-search{filtre}")
                        attr:class="text-[var(--color-accent)] hover:underline"
                    >
                        {ui.filter_by_trade}
                    </A>
                </p>
            </header>

            <ul class="flex flex-col">
                {hits.into_iter().map(|h| view! { <AnnonceLigne hit=h /> }).collect_view()}
            </ul>

            <YearPagination base=base_path page=numero page_count=total_pages />

            <section class="flex flex-col gap-2 border-t border-[var(--color-rule)] pt-6">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.follow_department}
                </h2>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    {ui.follow_text}
                </p>
                <p class="flex flex-wrap gap-4 text-sm">
                    <a
                        href=format!("/journal-search.rss{filtre}")
                        class="text-[var(--color-accent)] hover:underline"
                    >
                        {ui.rss}
                    </a>
                    <a
                        href=format!("/journal-search.ics{filtre}")
                        class="text-[var(--color-accent)] hover:underline"
                    >
                        {ui.calendar}
                    </a>
                </p>
            </section>
        </div>
    }
}

/// Une consultation en liste, côté pages de crawl (SSR, sans JS).
#[component]
fn AnnonceLigne(hit: JournalHit) -> impl IntoView {
    let href = format!("/notice/fr/{}", hit.uid);
    let acteur = acteur(&hit);
    let resume = description(&hit);
    let meta = ligne_meta(&hit, &crate::i18n::use_ui().notices);
    view! {
        <li class="flex flex-col gap-1 border-t border-[var(--color-rule)] py-3">
            <A
                href=href
                attr:class="text-base text-[var(--color-ink)] hover:text-[var(--color-accent)]"
            >
                {hit.titre.clone()}
            </A>
            {acteur.map(|a| view! { <p class="text-sm text-[var(--color-ink-muted)]">{a}</p> })}
            {resume
                .map(|d| {
                    view! {
                        <p class="line-clamp-2 text-sm leading-snug text-[var(--color-ink-muted)]">
                            {d}
                        </p>
                    }
                })}
            <p class="text-xs text-[var(--color-ink-subtle)]">{meta}</p>
        </li>
    }
}

/// Le libellé d'acteur quand il en existe un. Une part du stock ouvert n'en
/// a pas : les instances Atexo sur PLACE ne servent `organisme` que comme URI de
/// référentiel, illisible sans JWT. Une chaîne vide n'est pas un nom — elle ne
/// rend ni ligne, ni fil d'Ariane, ni titre.
fn acteur(hit: &JournalHit) -> Option<String> {
    Some(hit.acteur.trim())
        .filter(|a| !a.is_empty())
        .map(str::to_string)
}

/// Ligne de métadonnées d'une consultation : échéance, lieu, nature, canaux.
fn ligne_meta(hit: &JournalHit, ui: &crate::i18n::NoticesUi) -> String {
    let dates = &crate::i18n::use_ui().dates;
    let mut bouts: Vec<String> = Vec::new();
    if let Some(d) = hit.echeance.as_deref().and_then(|d| numeric_date(dates, d)) {
        bouts.push((ui.due_before)(&d));
    }
    if let Some(f) = hit.famille.as_ref() {
        bouts.push(f.label.clone());
    }
    if let Some(c) = hit.cpv.as_ref() {
        bouts.push(c.label.clone());
    }
    for m in &hit.metiers {
        bouts.push(m.label.clone());
    }
    if hit.canaux.len() > 1 {
        bouts.push((ui.channels)(hit.canaux.len()));
    }
    bouts.join(" · ")
}

// ── /notice/fr/{uid} ───────────────────────────────────────────────────────────

#[component]
pub fn MarchePage() -> impl IntoView {
    use leptos::either::EitherOf3;
    own_page_description();
    let merged = crate::i18n::use_ui().notices.merged;
    let params = use_params_map();
    let navigate = use_navigate();
    let fiche = Resource::new_blocking(
        move || params.get().get("uid").unwrap_or_default(),
        |uid| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_annonce(&uid)
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || {
                let navigate = navigate.clone();
                Suspend::new(async move {
                    match fiche.await {
                        // L'API a suivi une redirection (uid absorbé par une
                        // fusion) : la fiche rendue est celle de
                        // l'élu, l'URL doit suivre — 301 côté SSR, navigation
                        // remplacée côté client.
                        Ok(r) if params.get_untracked().get("uid").as_deref() != Some(&r.annonce.uid) => {
                            let cible = format!("/notice/fr/{}", r.annonce.uid);
                            redirection_permanente(&cible);
                            Effect::new({
                                let cible = cible.clone();
                                move |_| {
                                    navigate(
                                        &cible,
                                        leptos_router::NavigateOptions {
                                            replace: true,
                                            ..Default::default()
                                        },
                                    );
                                }
                            });
                            EitherOf3::A(view! {
                                <p class="p-6">
                                    <a href=cible.clone()>{merged}</a>
                                </p>
                            })
                        }
                        Ok(r) => {
                            set_cache_control(200);
                            EitherOf3::B(view! { <MarcheLoaded fiche=r /> })
                        }
                        Err(err) => {
                            set_cache_control(err.status);
                            EitherOf3::C(view! { <HubError err=err /> })
                        }
                    }
                })
            }}
        </Suspense>
    }
}

/// Pose un 301 vers la fiche canonique : un uid absorbé par une fusion
/// transfère son indexation au survivant.
#[cfg(feature = "ssr")]
fn redirection_permanente(cible: &str) {
    use axum::http::{header::LOCATION, HeaderValue, StatusCode};
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        resp.set_status(StatusCode::MOVED_PERMANENTLY);
        if let Ok(hv) = HeaderValue::from_str(cible) {
            resp.insert_header(LOCATION, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn redirection_permanente(_cible: &str) {}

#[component]
fn MarcheLoaded(fiche: AnnonceDetailResponse) -> impl IntoView {
    let ui = &crate::i18n::use_ui().notices;
    let dates = &crate::i18n::use_ui().dates;
    let hit = fiche.annonce;
    let acheteur = acteur(&hit);
    let attribue = hit.attribue;
    let title = match &acheteur {
        Some(a) => format!("{} — {a} - LibreJustice", hit.titre),
        None => format!("{} - LibreJustice", hit.titre),
    };
    let echeance = hit.echeance.as_deref().and_then(|d| numeric_date(dates, d));
    let qui = acheteur.clone().unwrap_or_else(|| hit.titre.clone());
    let deps: Vec<&FacetTag> = hit
        .zones
        .iter()
        .filter(|z| zone_est_departement(&z.key))
        .collect();
    let departement = (deps.len() == 1).then(|| deps[0]);
    let portee = portee_zones(ui, &hit.zones);
    let description = match (attribue, &echeance, departement.map(|z| z.label.as_str())) {
        (true, _, _) => (ui.description_awarded)(&qui),
        (false, Some(d), Some(dep)) => (ui.description_open)(dep, &qui, d),
        _ => (ui.description_other)(&qui),
    };
    let url = format!("{BASE}/notice/fr/{}", hit.uid);
    #[cfg(feature = "hydrate")]
    {
        let document = url.clone();
        Effect::new(move |_| crate::reads::report(lj_dtos::ReadKind::Annonce, &document));
    }
    let liens = liens_utiles(&hit, &fiche.liens, ui);
    let releve = releve(&fiche.canaux_detail);
    let dep_href = departement.map(|z| format!("/journal-index/fr/{}", z.key));
    // Fil d'Ariane structuré seulement, comme `/decision/{id}` : la fiche ne
    // rend pas de fil visible. Les échelons sont les pages de crawl réelles
    // (catalogue → département).
    let jsonld = {
        let mut items = vec![
            (crate::i18n::use_ui().shell.home, format!("{BASE}/")),
            (ui.public_markets, format!("{BASE}/journal-index/fr")),
        ];
        if let Some(z) = departement {
            items.push((
                z.label.as_str(),
                format!("{BASE}/journal-index/fr/{}", z.key),
            ));
        }
        items.push((&hit.titre, url.clone()));
        breadcrumb_jsonld(&items)
    };

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <crate::seo::Canonical href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{hit.titre.clone()}</h1>
                {acheteur
                    .clone()
                    .map(|a| view! { <p class="text-[var(--color-ink-muted)]">{a}</p> })}
                {hit.closed
                    .then(|| {
                        view! {
                            <p class="rounded-lg border border-[var(--color-rule)] px-3 py-2 text-sm text-[var(--color-ink-muted)]">
                                {ui.closed}
                            </p>
                        }
                    })}
            </header>

            {hit.description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .map(|d| {
                    view! {
                        <p class="whitespace-pre-line text-[15px] leading-relaxed text-[var(--color-ink)]">
                            {crate::components::search::compact_search::highlight::plain_text(d)}
                        </p>
                    }
                })}

            <dl class="grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                <Fait
                    terme=ui.fact_due
                    valeur=echeance.unwrap_or_else(|| ui.not_published.to_string())
                />
                <Fait terme=ui.fact_published valeur=numeric_date(dates, &hit.date_publication).unwrap_or(hit.date_publication.clone()) />
                {hit.famille.as_ref().map(|f| view! { <Fait terme=ui.fact_nature valeur=f.label.clone() /> })}
                {hit.cpv
                    .as_ref()
                    .map(|c| {
                        let terme = match hit.cpv_deduit {
                            true => ui.fact_cpv_inferred,
                            false => ui.fact_cpv,
                        };
                        view! { <Fait terme=terme valeur=c.label.clone() /> }
                    })}
                {(!hit.metiers.is_empty())
                    .then(|| {
                        let metiers = hit
                            .metiers
                            .iter()
                            .map(|m| m.label.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let terme = match hit.metiers_deduits {
                            true => ui.fact_trade_inferred,
                            false => ui.fact_trade,
                        };
                        view! { <Fait terme=terme valeur=metiers /> }
                    })}
                {hit.reference
                    .as_ref()
                    .map(|r| view! { <Fait terme=ui.fact_reference valeur=r.clone() /> })}
                {hit.procedure
                    .as_ref()
                    .map(|p| view! { <Fait terme=ui.fact_procedure valeur=p.clone() /> })}
                {portee
                    .clone()
                    .map(|d| {
                        let valeur = match &dep_href {
                            // Le lien vers le hub départemental remplace le fil
                            // d'Ariane : un chemin de crawl, pas une hiérarchie.
                            Some(href) => {
                                Either::Left(
                                    view! {
                                        <A
                                            href=href.clone()
                                            attr:class="text-[var(--color-accent)] hover:underline"
                                        >
                                            {d.clone()}
                                        </A>
                                    },
                                )
                            }
                            None => Either::Right(view! { <span>{d}</span> }),
                        };
                        view! {
                            <div class="flex flex-col gap-0.5">
                                <dt class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                                    {ui.fact_place}
                                </dt>
                                <dd class="text-sm text-[var(--color-ink)]">{valeur}</dd>
                            </div>
                        }
                    })}
                {fiche
                    .montant_estime
                    .map(|m| {
                        view! {
                            <Fait
                                terme=ui.fact_estimate
                                valeur=somme(m.valeur, m.devise.as_deref())
                            />
                        }
                    })}
                {releve.map(|r| view! { <Fait terme=ui.fact_seen valeur=r /> })}
            </dl>

            {(!fiche.lots_ouverts.is_empty())
                .then(|| view! { <Allotissement lots=fiche.lots_ouverts /> })}
            {(!fiche.criteres.is_empty()).then(|| view! { <Criteres criteres=fiche.criteres /> })}

            {fiche.resultat.map(|r| view! { <Resultat resultat=r /> })}

            <section class="flex flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {if attribue { ui.see_notice } else { ui.file_bid }}
                </h2>
                <ul class="flex flex-col gap-1.5">
                    {liens
                        .into_iter()
                        .map(|(label, url)| {
                            view! {
                                <li>
                                    <a
                                        href=url
                                        rel="nofollow noopener"
                                        class="text-sm text-[var(--color-accent)] hover:underline"
                                    >
                                        {label}
                                    </a>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </section>

            {(!fiche.entites.is_empty())
                .then(|| {
                    view! {
                        <section class="flex flex-col gap-2">
                            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                                {ui.actors}
                            </h2>
                            <ul class="flex flex-col gap-1">
                                {fiche
                                    .entites
                                    .into_iter()
                                    .map(|e| {
                                        let label = match e.href {
                                            Some(href) => Either::Left(
                                                view! {
                                                    <A
                                                        href=href
                                                        attr:class="text-[var(--color-accent)] hover:underline"
                                                    >
                                                        {e.label}
                                                    </A>
                                                },
                                            ),
                                            None => Either::Right(view! { <span>{e.label}</span> }),
                                        };
                                        view! {
                                            <li class="text-sm">
                                                {label}
                                                <span class="text-[var(--color-ink-subtle)]">
                                                    {format!(" — {}", e.role)}
                                                </span>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                        </section>
                    }
                })}

        </div>
    }
}

/// Les liens de la fiche, dédupliqués par URL. Un marché à canal unique porte
/// souvent la même adresse en `url_action`, en lien d'avis et en URL de canal.
/// Le premier libellé rencontré gagne, `url_action` d'abord — c'est le « où
/// agir » du fond.
/// On a market that only the BOAMP or the TED publishes, `url_action` is that
/// notice and no platform page.
fn libelle_action(url: &str, ui: &crate::i18n::NoticesUi) -> String {
    let hote = url.split('/').nth(2).unwrap_or_default();
    match hote.trim_start_matches("www.") {
        "boamp.fr" => ui.boamp_notice.to_string(),
        "ted.europa.eu" => ui.joue_notice.to_string(),
        _ => ui.platform_page.to_string(),
    }
}

fn liens_utiles(
    hit: &JournalHit,
    liens: &[lj_dtos::AnnonceLienDto],
    ui: &crate::i18n::NoticesUi,
) -> Vec<(String, String)> {
    let mut vus = vec![hit.url_action.clone()];
    let mut out = vec![(libelle_action(&hit.url_action, ui), hit.url_action.clone())];
    for l in liens {
        if vus.iter().any(|u| u == &l.url) {
            continue;
        }
        vus.push(l.url.clone());
        out.push((format!("{} ({})", l.label, l.moteur), l.url.clone()));
    }
    out
}

/// Depuis quand et par où nous voyons l'avis. Un fait de la fiche, pas une
/// section : sur un marché à canal unique il tient en une ligne.
fn releve(canaux: &[lj_dtos::AnnonceCanalDto]) -> Option<String> {
    let premier = canaux.iter().min_by_key(|c| c.first_seen_at.clone())?;
    let date = numeric_date(&crate::i18n::use_ui().dates, &premier.first_seen_at)?;
    let moteurs: Vec<&str> = canaux.iter().map(|c| c.moteur.as_str()).collect();
    Some(format!("{date} · {}", moteurs.join(", ")))
}

/// The allotment of a still-open tender, before any awardee.
#[component]
fn Allotissement(lots: Vec<lj_dtos::AnnonceLotOuvertDto>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().notices;
    let nombre = lots.len();
    view! {
        <section class="flex flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {(ui.lots)(nombre)}
            </h2>
            <ul class="flex flex-col gap-3">
                {lots
                    .into_iter()
                    .enumerate()
                    .map(|(i, lot)| {
                        let titre = lot.nom.unwrap_or_else(|| (ui.lot_n)(i + 1));
                        view! {
                            <li class="flex flex-col gap-0.5 border-t border-[var(--color-rule)] pt-3">
                                <p class="text-sm text-[var(--color-ink)]">{titre}</p>
                                {lot.description
                                    .map(|d| {
                                        view! {
                                            <p class="text-sm leading-snug text-[var(--color-ink-muted)]">
                                                {d}
                                            </p>
                                        }
                                    })}
                                {lot.cpv
                                    .map(|c| {
                                        view! {
                                            <p class="text-xs text-[var(--color-ink-subtle)]">
                                                {format!("CPV {c}")}
                                            </p>
                                        }
                                    })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

/// Award criteria, verbatim: a notice weighs them, splits them into
/// sub-criteria or defers to its own rules, and none of that normalises.
#[component]
fn Criteres(criteres: Vec<String>) -> impl IntoView {
    let criteria = crate::i18n::use_ui().notices.criteria;
    view! {
        <section class="flex flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {criteria}
            </h2>
            <ul class="flex list-disc flex-col gap-1.5 pl-5">
                {criteres
                    .into_iter()
                    .map(|c| {
                        view! { <li class="text-sm leading-snug text-[var(--color-ink)]">{c}</li> }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

/// L'attribution d'un marché : les lots, et le renvoi vers l'avis
/// de résultat au BOAMP quand il ne partage pas la page de la consultation. Un
/// avis sans lot exploitable (infructueux, blob illisible) n'affiche que le
/// paragraphe et le renvoi.
#[component]
fn Resultat(resultat: lj_dtos::AnnonceResultatDto) -> impl IntoView {
    let ui = &crate::i18n::use_ui().notices;
    let renvoi = resultat
        .avis_url
        .as_ref()
        .map(|url| (ui.award_notice, url.clone()));
    view! {
        <section class="flex flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {ui.award}
            </h2>
            {resultat
                .texte
                .map(|t| {
                    view! {
                        <p class="whitespace-pre-line text-sm text-[var(--color-ink)]">{crate::components::search::compact_search::highlight::plain_text(t)}</p>
                    }
                })}
            <ul class="flex flex-col gap-3">
                {resultat
                    .lots
                    .into_iter()
                    .map(|lot| {
                        let meta = lot_meta(&lot, ui);
                        let titulaire = match &lot.href {
                            Some(href) => {
                                Either::Left(
                                    view! {
                                        <A
                                            href=href.clone()
                                            attr:class="text-[var(--color-accent)] hover:underline"
                                        >
                                            {lot.titulaire.clone()}
                                        </A>
                                    },
                                )
                            }
                            None => Either::Right(view! { <span>{lot.titulaire.clone()}</span> }),
                        };
                        view! {
                            <li class="flex flex-col gap-0.5 border-t border-[var(--color-rule)] pt-3">
                                {lot.description
                                    .map(|d| {
                                        view! {
                                            <p class="text-xs text-[var(--color-ink-subtle)]">{d}</p>
                                        }
                                    })}
                                <p class="text-sm text-[var(--color-ink)]">{titulaire}</p>
                                {(!meta.is_empty())
                                    .then(|| {
                                        view! {
                                            <p class="text-sm text-[var(--color-ink-muted)]">{meta}</p>
                                        }
                                    })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
            {renvoi
                .map(|(label, href)| {
                    view! {
                        <p class="text-sm">
                            <A href=href attr:class="text-[var(--color-accent)] hover:underline">
                                {label}
                            </A>
                        </p>
                    }
                })}
        </section>
    }
}

/// Montant, offres reçues et date d'attribution d'un lot, quand la source les
/// donne. Montant en euros pleins : les avis n'ont pas de centimes fiables.
fn lot_meta(lot: &lj_dtos::AnnonceLotDto, ui: &crate::i18n::NoticesUi) -> String {
    let mut bouts: Vec<String> = Vec::new();
    if let Some(m) = lot.montant {
        bouts.push(somme(m, lot.devise.as_deref()));
    }
    if let Some(n) = lot.nb_offres {
        bouts.push((ui.offers_received)(n));
    }
    if let Some(d) = lot
        .date_attribution
        .as_deref()
        .and_then(|d| numeric_date(&crate::i18n::use_ui().dates, d))
    {
        bouts.push((ui.awarded_on)(&d));
    }
    bouts.join(" · ")
}

/// Whole euros, grouped by thousands: notices carry no reliable cents.
fn somme(valeur: f64, devise: Option<&str>) -> String {
    let entier = format!("{:.0}", valeur.max(0.0));
    let groupe: String = entier
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\u{202f}");
    match devise {
        Some("EUR") | None => format!("{groupe} €"),
        Some(d) => format!("{groupe} {d}"),
    }
}

/// Une paire terme/valeur de la fiche.
#[component]
pub(crate) fn Fait(terme: &'static str, valeur: String) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-0.5">
            <dt class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                {terme}
            </dt>
            <dd class="text-sm text-[var(--color-ink)]">{valeur}</dd>
        </div>
    }
}
