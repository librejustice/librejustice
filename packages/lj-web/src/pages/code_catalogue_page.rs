//! Page `/codes` (CodeCataloguePage) — catalogue des codes et lois du corpus
//! (`legal_text`), groupé par ordre/origine (FR / UE / international / codes
//! étrangers), chaque code liant son sommaire `/texte/{code}`.
//!
//! Le SSR ne rend que les familles de TÊTE (codes, constitutions — ~200
//! entrées) : rendre les 6 700 entrées pesait 6,2 Mo de DOM + payload
//! d'hydratation. La longue traîne (lois, ordonnances, règlements UE) se
//! charge à la demande — dépliage explicite ou première frappe du filtre.
//! Les textes de la traîne restent crawlables par les sitemaps `/texte`.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use lj_dtos::CodeCatalogueEntry;

use crate::api::ApiClient;
use crate::pages::law_page::data::{sendable, PageError};

#[component]
pub fn CodeCataloguePage() -> impl IntoView {
    // Familles de tête, bloquant SSR (dans le document initial pour le SEO).
    let entries = Resource::new_blocking(|| (), |_| sendable(fetch_catalogue()));

    view! {
        <Suspense fallback=CatalogueSkeleton>
            {move || Suspend::new(async move {
                match entries.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <CatalogueLoaded entries=r.entries total=r.total /> })
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

/// Charge les familles de tête du catalogue. Bloquant SSR.
async fn fetch_catalogue() -> Result<lj_dtos::CodeCatalogueResponse, PageError> {
    ApiClient::from_context()
        .fetch_codes_catalogue(true)
        .await
        .map_err(PageError::from)
}

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

#[component]
fn CatalogueError(err: PageError) -> impl IntoView {
    let title = "Catalogue indisponible - LibreJustice";
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
fn CatalogueLoaded(entries: Vec<CodeCatalogueEntry>, total: u64) -> impl IntoView {
    let title = "Codes & lois - LibreJustice";
    let description =
        "Catalogue des codes et lois consolidés : droit français, droit de l'Union européenne, \
         conventions internationales et codes étrangers. Versions à date, articles liés.";

    // Filtre texte (client) : signal saisi par l'utilisateur, appliqué sur le
    // titre. En SSR le filtre est vide ⇒ les familles de tête sont rendues.
    let filter = RwSignal::new(String::new());
    let tail_count = total.saturating_sub(entries.len() as u64);
    let head = StoredValue::new(entries);

    // Longue traîne : chargée UNE fois, à la demande — dépliage explicite ou
    // première frappe du filtre (le filtre cherche sur tout le catalogue).
    let want_full = RwSignal::new(false);
    let full = RwSignal::new(None::<Vec<CodeCatalogueEntry>>);
    Effect::new(move |_| {
        if !want_full.get() || full.with(Option::is_some) {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(r) = ApiClient::from_context().fetch_codes_catalogue(false).await {
                full.set(Some(r.entries));
            }
        });
    });

    // Jeu actif : la traîne complète dès qu'elle est chargée, la tête sinon.
    let dataset = Memo::new(move |_| {
        full.get()
            .filter(|_| want_full.get())
            .unwrap_or_else(|| head.get_value())
    });
    // Groupes ordonnés (FR, UE, INTL, autres), recalculés au changement de jeu.
    let groups = Memo::new(move |_| {
        let mut groups: Vec<(String, Vec<CodeCatalogueEntry>)> = Vec::new();
        for entry in dataset.get() {
            if let Some(g) = groups.iter_mut().find(|(j, _)| *j == entry.jurisdiction) {
                g.1.push(entry);
            } else {
                groups.push((entry.jurisdiction.clone(), vec![entry]));
            }
        }
        groups.sort_by_key(|(j, _)| group_rank(j));
        groups
    });

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
                on:input=move |ev| {
                    let v = event_target_value(&ev);
                    if !v.trim().is_empty() {
                        want_full.set(true);
                    }
                    filter.set(v);
                }
                placeholder="Filtrer par titre (ex. « civil »)…"
                autocomplete="off"
                class="w-full rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
            />

            <Show when=move || want_full.get() && full.with(Option::is_none)>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    "Chargement du catalogue complet…"
                </p>
            </Show>

            <For
                each=move || groups.get()
                key=|(j, codes): &(String, Vec<CodeCatalogueEntry>)| (j.clone(), codes.len())
                children=move |(jurisdiction, codes): (String, Vec<CodeCatalogueEntry>)| {
                    view! { <CatalogueSection jurisdiction=jurisdiction codes=codes filter=filter /> }
                }
            />

            <Show when=move || { tail_count > 0 && !want_full.get() }>
                <button
                    type="button"
                    on:click=move |_| want_full.set(true)
                    class="self-start rounded-lg border border-[var(--color-rule)] px-4 py-2.5 text-sm text-[var(--color-ink-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
                >
                    {format!("Afficher les {tail_count} lois, ordonnances et règlements du corpus")}
                </button>
            </Show>
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
    let href = format!("/texte/{}", entry.code);
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
