//! Page `/norm-index/{juris}/codes`: the codes and constitutions of one legal
//! order, each linking its table of contents `/norm/{juris}/{code}`. Pure SSR;
//! the other collections (statutes, decrees) are browsed by year under
//! `/norm-index/{juris}/{fond}`.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use lj_dtos::CodeCatalogueEntry;

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, set_cache_control, HubBreadcrumb, HubError, HubSkeleton,
};
use crate::pages::law_page::data::{sendable, PageError};

#[component]
pub fn CodeCataloguePage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    // Catalogue complet du fond, bloquant SSR (dans le document initial pour le SEO).
    let params = use_params_map();
    let entries = Resource::new_blocking(
        move || params.get().get("juris").unwrap_or_default(),
        |juris| sendable(fetch_catalogue(juris)),
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match entries.await {
                    Ok((juris, entries)) => {
                        set_cache_control(200);
                        Either::Left(view! { <CatalogueLoaded juris=juris entries=entries /> })
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

/// Charge le catalogue (codes + constitutions) d'un ordre juridique. Bloquant SSR.
async fn fetch_catalogue(juris: String) -> Result<(String, Vec<CodeCatalogueEntry>), PageError> {
    let mut entries = ApiClient::from_context()
        .fetch_codes_catalogue()
        .await
        .map_err(PageError::from)?
        .entries;
    entries.retain(|e| lj_vocab::url_segment(&e.jurisdiction) == juris);
    if entries.is_empty() {
        return Err(PageError {
            status: 404,
            message: "Juridiction inconnue".to_string(),
        });
    }
    Ok((juris, entries))
}

/// Rank of a legal order in the catalogue: France, the Union, international
/// law, then every other order, by name.
fn jurisdiction_rank(jurisdiction: &str) -> u8 {
    match jurisdiction {
        "FR" => 0,
        "EU" => 1,
        "INTL" => 2,
        _ => 3,
    }
}

/// Sous-catégorie d'affichage d'un texte à partir de sa `nature` brute
/// (taxonomie du corpus) : `(rang, libellé)`. Le fond ne porte que ces deux
/// familles (codes, constitutions).
fn nature_category(nature: &str) -> (u8, &'static str) {
    let n = nature.to_ascii_uppercase();
    if n == "CONSTITUTION" {
        (1, "Constitutions")
    } else {
        (0, "Codes")
    }
}

#[component]
fn CatalogueLoaded(juris: String, entries: Vec<CodeCatalogueEntry>) -> impl IntoView {
    let title = "Codes : textes consolidés - LibreJustice";
    let description =
        "Catalogue des codes et constitutions consolidés, un groupe par ordre juridique, de la \
         France à chaque État ou territoire. Versions à date, articles liés.";
    let index = format!("/norm-index/{juris}");
    let url = format!("https://librejustice.fr{index}/codes");
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", format!("https://librejustice.fr{index}")),
        ("Codes", url.clone()),
    ]);
    let count_line = format!("{} textes.", entries.len());

    let mut groups: Vec<(String, String, Vec<CodeCatalogueEntry>)> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|(j, _, _)| *j == entry.jurisdiction) {
            Some(group) => group.2.push(entry),
            None => groups.push((
                entry.jurisdiction.clone(),
                entry.jurisdiction_label.clone(),
                vec![entry],
            )),
        }
    }
    groups.sort_by_cached_key(|(j, label, _)| (jurisdiction_rank(j), lj_fold::fold(label)));

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
                    ("Codes".to_string(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Textes & codes"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Codes"</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">{count_line}</p>
            </header>

            {groups
                .into_iter()
                .map(|(_, label, codes)| {
                    view! { <CatalogueSection label=label codes=codes /> }
                })
                .collect_view()}
        </div>
    }
}

/// A legal order: its heading, then one sub-section per nature.
#[component]
fn CatalogueSection(label: String, codes: Vec<CodeCatalogueEntry>) -> impl IntoView {
    // Sous-groupes (catégorie de nature) ordonnés Codes → Constitutions ;
    // chaque sous-groupe garde l'ordre alphabétique d'arrivée.
    let mut categories: Vec<(u8, &'static str, Vec<CodeCatalogueEntry>)> = Vec::new();
    for entry in codes {
        let (rank, label) = nature_category(&entry.nature);
        if let Some(g) = categories.iter_mut().find(|(r, _, _)| *r == rank) {
            g.2.push(entry);
        } else {
            categories.push((rank, label, vec![entry]));
        }
    }
    categories.sort_by_key(|(r, _, _)| *r);

    view! {
        <section class="flex flex-col gap-4">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {label}
            </h2>
            {categories
                .into_iter()
                .map(|(_, label, codes)| view! { <CatalogueCategory label=label codes=codes /> })
                .collect_view()}
        </section>
    }
}

/// Une sous-section par nature (Codes / Constitutions) dans un ordre de
/// juridiction.
#[component]
fn CatalogueCategory(label: &'static str, codes: Vec<CodeCatalogueEntry>) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2">
            <h3 class="text-[0.7rem] uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]/70">
                {label}
            </h3>
            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {codes
                    .into_iter()
                    .map(|entry| view! { <CatalogueRow entry=entry /> })
                    .collect_view()}
            </ul>
        </div>
    }
}

#[component]
fn CatalogueRow(entry: CodeCatalogueEntry) -> impl IntoView {
    let href = lj_vocab::norm_path(&entry.jurisdiction, &entry.code);
    let count = format!(
        "{} article{}",
        entry.article_count,
        if entry.article_count > 1 { "s" } else { "" }
    );
    view! {
        <li class="py-3">
            <A href=href attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <span class="font-sans text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                    {entry.title}
                </span>
                <span class="tabular-nums text-sm text-[var(--color-ink-subtle)]">{count}</span>
            </A>
        </li>
    }
}
