//! Page `/codes` (CodeCataloguePage) — catalogue des codes et lois du corpus
//! (`legal_text`). Rendue SSR (liste crawlable, route indexable), groupée par
//! ordre/origine (FR / UE / international / codes étrangers), chaque code liant
//! son sommaire `/loi/{code}`. Une boîte de filtre client affine la liste par
//! titre sans rappel réseau.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use lj_dtos::CodeCatalogueEntry;

use crate::api::ApiClient;
use crate::pages::law_page::data::{sendable, PageError};

#[component]
pub fn CodeCataloguePage() -> impl IntoView {
    // Catalogue bloquant SSR (liste dans le document initial pour le SEO).
    let entries = Resource::new_blocking(|| (), |_| sendable(fetch_catalogue()));

    view! {
        <Suspense fallback=CatalogueSkeleton>
            {move || Suspend::new(async move {
                match entries.await {
                    Ok(entries) => {
                        set_cache_control(200);
                        Either::Left(view! { <CatalogueLoaded entries=entries /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <CatalogueError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// Charge le catalogue des codes. Bloquant SSR.
async fn fetch_catalogue() -> Result<Vec<CodeCatalogueEntry>, PageError> {
    ApiClient::from_context()
        .fetch_codes_catalogue()
        .await
        .map(|r| r.entries)
        .map_err(PageError::from)
}

#[cfg(feature = "ssr")]
fn set_cache_control(status: u16) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue};
    let value = match status {
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 | 422 => "public, max-age=0, s-maxage=300",
        _ => "no-store",
    };
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if let Ok(hv) = HeaderValue::from_str(value) {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn set_cache_control(_status: u16) {}

#[component]
fn CatalogueError(err: PageError) -> impl IntoView {
    let title = "Catalogue indisponible — LibreJustice";
    view! {
        <Title text=title />
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
fn CatalogueSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-9 w-1/2" />
            <Skeleton class="h-10 w-full" />
            <Skeleton class="h-4 w-2/3" />
            <Skeleton class="h-4 w-1/2" />
        </div>
    }
}

/// Libellé d'un groupe à partir de la valeur `jurisdiction` du corpus.
fn group_label(jurisdiction: &str) -> &'static str {
    match jurisdiction {
        "FR" => "France",
        "UE" => "Union européenne",
        "INTL" => "International",
        _ => "Autres juridictions",
    }
}

/// Ordre d'affichage des groupes : FR, UE, INTL, puis le reste (codes étrangers).
fn group_rank(jurisdiction: &str) -> u8 {
    match jurisdiction {
        "FR" => 0,
        "UE" => 1,
        "INTL" => 2,
        _ => 3,
    }
}

/// Sous-catégorie d'affichage d'un texte à partir de sa `nature` brute (taxonomie
/// du corpus) : `(rang, libellé)`. Sous-groupe un ordre de juridiction en Codes /
/// Constitutions / Lois. Le scope du catalogue (côté store) ne laisse passer que
/// ces familles (ADR 0133).
fn nature_category(nature: &str) -> (u8, &'static str) {
    let n = nature.to_ascii_uppercase();
    if n.starts_with("CODE") || n == "ETAT_CIVIL" {
        (0, "Codes")
    } else if n == "CONSTITUTION" || n == "LOI_CONSTIT" {
        (1, "Constitutions")
    } else {
        (2, "Lois et ordonnances")
    }
}

#[component]
fn CatalogueLoaded(entries: Vec<CodeCatalogueEntry>) -> impl IntoView {
    let title = "Codes & lois — LibreJustice";
    let description =
        "Catalogue des codes et lois consolidés : droit français, droit de l'Union européenne, \
         conventions internationales et codes étrangers. Versions à date, articles liés.";

    // Filtre texte (client) : signal saisi par l'utilisateur, appliqué sur le
    // titre. En SSR le filtre est vide ⇒ tout le catalogue est rendu (crawlable).
    let filter = RwSignal::new(String::new());

    // Groupes ordonnés (FR, UE, INTL, autres) à partir des entrées du corpus. Calculé
    // une fois (données non réactives) ; chaque groupe garde son ordre d'arrivée.
    let mut groups: Vec<(String, Vec<CodeCatalogueEntry>)> = Vec::new();
    for entry in entries {
        if let Some(g) = groups.iter_mut().find(|(j, _)| *j == entry.jurisdiction) {
            g.1.push(entry);
        } else {
            groups.push((entry.jurisdiction.clone(), vec![entry]));
        }
    }
    groups.sort_by_key(|(j, _)| group_rank(j));

    let sections = groups
        .into_iter()
        .map(|(jurisdiction, codes)| {
            view! { <CatalogueSection jurisdiction=jurisdiction codes=codes filter=filter /> }
        })
        .collect_view();

    view! {
        <Title text=title />
        <Meta name="description" content=description />

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Référentiel"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Codes & lois"</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    "Parcourez les codes et lois consolidés du corpus. Chaque texte ouvre "
                    "son sommaire et ses articles versionnés."
                </p>
            </header>

            <input
                type="search"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))
                placeholder="Filtrer par titre (ex. « civil »)…"
                autocomplete="off"
                class="w-full rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
            />

            {sections}
        </div>
    }
}

/// Une section d'ordre/origine : en-tête + sous-sections par nature (Codes /
/// Constitutions / Lois). Masquée si le filtre ne laisse aucun code.
#[component]
fn CatalogueSection(
    jurisdiction: String,
    codes: Vec<CodeCatalogueEntry>,
    filter: RwSignal<String>,
) -> impl IntoView {
    let label = group_label(&jurisdiction);
    let codes = StoredValue::new(codes);

    // Sous-groupes (catégorie de nature) du filtre courant, ordonnés Codes →
    // Constitutions → Lois ; chaque sous-groupe garde l'ordre alphabétique d'arrivée.
    let categories = Memo::new(move |_| {
        let needle = filter.get().trim().to_lowercase();
        let mut groups: Vec<(u8, &'static str, Vec<CodeCatalogueEntry>)> = Vec::new();
        codes.with_value(|c| {
            for entry in c {
                if !needle.is_empty() && !entry.title.to_lowercase().contains(&needle) {
                    continue;
                }
                let (rank, label) = nature_category(&entry.nature);
                if let Some(g) = groups.iter_mut().find(|(r, _, _)| *r == rank) {
                    g.2.push(entry.clone());
                } else {
                    groups.push((rank, label, vec![entry.clone()]));
                }
            }
        });
        groups.sort_by_key(|(r, _, _)| *r);
        groups
    });

    view! {
        <Show when=move || !categories.get().is_empty()>
            <section class="flex flex-col gap-4">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {label}
                </h2>
                <For
                    each=move || categories.get()
                    key=|(rank, _, codes): &(u8, &'static str, Vec<CodeCatalogueEntry>)| {
                        (*rank, codes.len())
                    }
                    children=|(_, label, codes): (u8, &'static str, Vec<CodeCatalogueEntry>)| {
                        view! { <CatalogueCategory label=label codes=codes /> }
                    }
                />
            </section>
        </Show>
    }
}

/// Une sous-section par nature (Codes / Constitutions / Lois) dans un ordre de
/// juridiction.
#[component]
fn CatalogueCategory(label: &'static str, codes: Vec<CodeCatalogueEntry>) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2">
            <h3 class="text-[0.7rem] uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]/70">
                {label}
            </h3>
            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                <For
                    each=move || codes.clone()
                    key=|e: &CodeCatalogueEntry| e.code.clone()
                    children=|entry: CodeCatalogueEntry| view! { <CatalogueRow entry=entry /> }
                />
            </ul>
        </div>
    }
}

#[component]
fn CatalogueRow(entry: CodeCatalogueEntry) -> impl IntoView {
    let href = format!("/loi/{}", entry.code);
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
