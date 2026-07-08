//! Vue « Lois et règlements » de la page `/textes` (ADR 0114 pour le corpus ;
//! gabarit de référence pour la page) : barre de filtres horizontale
//! `Juridiction ▾ · Nature ▾ · Source ▾` + chips actives + liste paginée de
//! hits avec total exact. État (requête, filtres, page) dans les query params —
//! la clé URL du filtre provenance est `origine` (`source` est un nom hérité) ;
//! le param API `/api/search-textes` reste `source`. Données via
//! `ApiClient::search_textes`.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::params::ParamsMap;
use lj_dtos::{ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, FacetChoice};

use crate::api::{ApiClient, PageParams, TextesFilters};
use crate::helpers::{format_article_num, format_results_count, group_thousands, total_pages};
use crate::pages::decision_page::data::sendable;

use super::compact_search::highlight::Highlighted;
use super::compact_search::query_state;
use super::facet_widgets::Nav;
use super::filter_dropdown::{FilterDropdown, OpenDropdown};
use super::CompactSearch;

/// Articles ramenés par page.
const TEXTES_LIMIT: u32 = 20;

/// Plafond de pages affichées (cohérent avec `helpers::total_pages` ; le service
/// borne déjà le total à 400).
const MAX_PAGES_DISPLAY: i64 = 40;

/// Clés de filtre portées par l'URL (corpus articles).
const FILTER_KEYS: [&str; 3] = ["jurisdiction", "nature", "origine"];

/// État de recherche lu depuis l'URL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TextesQuery {
    q: String,
    jurisdiction: Option<String>,
    nature: Option<String>,
    origine: Option<String>,
    page: u32,
}

impl TextesQuery {
    fn from_map(map: &ParamsMap) -> Self {
        let single = |key: &str| {
            map.get(key)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Self {
            q: map
                .get("q")
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            jurisdiction: single("jurisdiction"),
            nature: single("nature"),
            origine: single("origine"),
            page: map
                .get("page")
                .and_then(|p| p.parse::<u32>().ok())
                .filter(|p| *p >= 1)
                .unwrap_or(1),
        }
    }
}

/// Récupère articles + facettes pour l'état courant. Requête vide ⇒ pas d'appel.
/// Erreur repliée en message (jamais de reject).
async fn fetch_textes(state: TextesQuery) -> Result<Option<ArticleSearchResponse>, String> {
    if state.q.is_empty() {
        return Ok(None);
    }
    let filters = TextesFilters {
        code: None,
        jurisdiction: state.jurisdiction.as_deref(),
        nature: state.nature.as_deref(),
        source: state.origine.as_deref(),
    };
    let offset = (state.page - 1) * TEXTES_LIMIT;
    ApiClient::from_context()
        .search_textes(
            &state.q,
            filters,
            PageParams {
                limit: TEXTES_LIMIT,
                offset,
            },
        )
        .await
        .map(Some)
        .map_err(|e| e.message)
}

/// Corps « Lois et règlements », anatomie `/recherche` : rail de synthèse à
/// gauche (volumétrie + facettes cliquables), colonne contenu à droite (barre,
/// filtres, chips, liste). Les facettes et la volumétrie alimentent rail et
/// barre par `Effect` (mises à jour sur réponse seulement — pendant un refetch
/// ils gardent les précédentes, interactifs). Sans requête, ni filtres ni
/// résultats — juste l'invite.
#[component]
pub fn TextesView() -> impl IntoView {
    let query_map = use_query_map();
    let state = Signal::derive(move || TextesQuery::from_map(&query_map.get()));
    let query = Signal::derive(move || state.get().q);
    let results = Resource::new(move || state.get(), |s| sendable(fetch_textes(s)));
    let facets = RwSignal::new(None::<ArticleSearchFacets>);
    let volume = RwSignal::new(None::<i64>);
    Effect::new(move |_| {
        if let Some(Ok(Some(resp))) = results.get() {
            facets.set(Some(resp.facets));
            volume.set(Some(resp.total));
        }
    });

    view! {
        <div class="grid grid-cols-1 gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
            <aside
                aria-label="Synthèse des résultats"
                class="hidden min-w-0 lg:sticky lg:top-20 lg:block lg:max-h-[calc(100dvh-6rem)] lg:self-start lg:overflow-y-auto"
            >
                <TextesRail query=query volume=volume facets=facets state=state />
            </aside>
            <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
                <CompactSearch />
                <Show
                    when=move || !query.get().is_empty()
                    fallback=move || view! { <TextSearchPrompt /> }
                >
                    <div class="flex min-w-0 flex-col gap-4">
                        <TextesFilterBar facets=facets state=state />
                        <TextesActiveChips facets=facets state=state />
                        // Volumétrie compacte, mobile uniquement (sur desktop
                        // elle vit dans le rail).
                        {move || {
                            volume
                                .get()
                                .map(|total| {
                                    view! {
                                        <p class="text-sm text-[var(--color-ink-subtle)] lg:hidden">
                                            <span class="font-medium text-[var(--color-ink)]">
                                                {format_results_count(total)}
                                            </span>
                                        </p>
                                    }
                                })
                        }}
                        <Suspense fallback=move || {
                            view! {
                                <p class="text-sm text-[var(--color-ink-subtle)]">"Recherche…"</p>
                            }
                        }>
                            {move || Suspend::new(async move {
                                let st = state.get_untracked();
                                match results.await {
                                    Ok(Some(resp)) => {
                                        view! { <TextResultsLayout resp=resp state=st /> }
                                            .into_any()
                                    }
                                    // Requête vide arrivée jusqu'au resource (le `<Show>`
                                    // amont ne rend ce `<Suspense>` que requête non vide :
                                    // ce bras couvre la frame où l'URL vient de se vider).
                                    Ok(None) => view! { <TextSearchPrompt /> }.into_any(),
                                    Err(msg) => {
                                        view! {
                                            <p class="text-sm text-[var(--color-ink-subtle)]">
                                                {format!("Recherche indisponible ({msg}).")}
                                            </p>
                                        }
                                            .into_any()
                                    }
                                }
                            })}
                        </Suspense>
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Layout chargé : liste + pagination (la volumétrie vit dans le rail, et en
/// ligne compacte mobile dans la colonne).
#[component]
fn TextResultsLayout(resp: ArticleSearchResponse, state: TextesQuery) -> impl IntoView {
    let total = resp.total;
    let pages = total_pages(total, TEXTES_LIMIT as i64, MAX_PAGES_DISPLAY) as u32;
    let page = state.page;
    let query = state.q.clone();
    let hits = resp.hits;

    view! {
        <section aria-label="Résultats" class="flex min-w-0 flex-col gap-6">
            <TextResults hits=hits query=query page=page />
            <TextPagination current_page=page total_pages=pages />
        </section>
    }
}

/// Lignes par bloc de facettes du rail.
const RAIL_ROWS: usize = 5;

/// Rail gauche de `/textes` (même gabarit que le rail `/recherche`) :
/// volumétrie + blocs de facettes cliquables (Nature, Juridictions, Sources) +
/// lien vers le catalogue des codes. Chaque clic passe par la MÊME mutation
/// d'URL mono-sélection que les dropdowns de la barre (valeur active ⇒
/// re-clic la retire, retour page 1).
#[component]
fn TextesRail(
    #[prop(into)] query: Signal<String>,
    #[prop(into)] volume: Signal<Option<i64>>,
    #[prop(into)] facets: Signal<Option<ArticleSearchFacets>>,
    #[prop(into)] state: Signal<TextesQuery>,
) -> impl IntoView {
    let header = move || {
        let Some(total) = volume.get() else {
            return view! {
                <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">
                    "Recherche en cours pour "
                    <em class="not-italic">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                    "…"
                </h2>
            }
            .into_any();
        };
        view! {
            <div class="flex flex-col gap-1">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {format_results_count(total)}
                </h2>
                <p class="text-sm leading-snug text-[var(--color-ink-subtle)]">
                    "pour "
                    <em class="text-[var(--color-ink)]">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                </p>
                <p class="text-xs text-[var(--color-ink-subtle)]">"Articles en vigueur"</p>
            </div>
        }
        .into_any()
    };

    let natures = move || {
        rail_facet_block(
            "Nature",
            "nature",
            facets.get().map(|f| f.nature).unwrap_or_default(),
            state.get().nature,
        )
    };
    let jurisdictions = move || {
        rail_facet_block(
            "Juridictions",
            "jurisdiction",
            facets.get().map(|f| f.jurisdiction).unwrap_or_default(),
            state.get().jurisdiction,
        )
    };
    let sources = move || {
        rail_facet_block(
            "Sources",
            "origine",
            facets.get().map(|f| f.source).unwrap_or_default(),
            state.get().origine,
        )
    };

    view! {
        <Show when=move || !query.get().is_empty()>
            <div class="flex flex-col gap-4">
                {header}
                {natures}
                {jurisdictions}
                {sources}
                <div class="border-t border-[var(--color-rule)] pt-4">
                    <A
                        href="/codes"
                        attr:class="text-[13px] text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-accent)] hover:underline"
                    >
                        "Parcourir le catalogue des codes"
                    </A>
                </div>
            </div>
        </Show>
    }
}

/// Bloc liste du rail textes : titre + lignes `libellé … compte` cliquables,
/// mono-sélection (la ligne active se retire au re-clic). Vide ⇒ rien.
fn rail_facet_block(
    title: &'static str,
    url_key: &'static str,
    choices: Vec<FacetChoice>,
    active: Option<String>,
) -> AnyView {
    if choices.is_empty() {
        return ().into_any();
    }
    let query_map = use_query_map();
    let nav = Nav::new();
    let rows = choices
        .into_iter()
        .take(RAIL_ROWS)
        .map(|c| {
            let is_active = active.as_deref() == Some(c.value.as_str());
            let label_class = if is_active {
                "truncate text-[13px] leading-snug font-medium text-[var(--color-bordeaux)]"
            } else {
                "truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors group-hover:text-[var(--color-ink)]"
            };
            let value = c.value;
            view! {
                <button
                    type="button"
                    class="group flex w-full items-baseline justify-between gap-3 text-left"
                    title=c.label.clone()
                    on:click=move |_| {
                        let next = if is_active {
                            query_state::with_param(&query_map.get_untracked(), url_key, None, true)
                        } else {
                            query_state::with_param(
                                &query_map.get_untracked(),
                                url_key,
                                Some(&value),
                                true,
                            )
                        };
                        nav.go(next);
                    }
                >
                    <span class=label_class>{c.label.clone()}</span>
                    <span class="shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]">
                        {group_thousands(c.count)}
                    </span>
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {rows}
        </div>
    }
    .into_any()
}

/// Barre de filtres textes (gabarit de référence « Lois et règlements ») :
/// `Juridiction ▾ · Nature ▾ · Source ▾`. Chaque facette est mono-sélection :
/// cliquer une valeur l'active (`?<clé>=<valeur>`), recliquer la retire. Toute
/// mutation supprime `page` (retour page 1).
#[component]
fn TextesFilterBar(
    #[prop(into)] facets: Signal<Option<ArticleSearchFacets>>,
    #[prop(into)] state: Signal<TextesQuery>,
) -> impl IntoView {
    // Un seul dropdown ouvert à la fois dans la barre.
    provide_context(OpenDropdown(RwSignal::new(None)));

    view! {
        <div
            role="toolbar"
            aria-label="Filtres de recherche"
            class="flex items-center gap-2 overflow-x-auto pb-1"
        >
            <FilterDropdown
                id="jurisdiction"
                label="Juridiction"
                active_count=Signal::derive(move || {
                    usize::from(state.get().jurisdiction.is_some())
                })
            >
                <MonoFacetList
                    url_key="jurisdiction"
                    choices=Signal::derive(move || {
                        facets.get().map(|f| f.jurisdiction).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().jurisdiction)
                />
            </FilterDropdown>
            <FilterDropdown
                id="nature"
                label="Nature"
                active_count=Signal::derive(move || usize::from(state.get().nature.is_some()))
            >
                <MonoFacetList
                    url_key="nature"
                    choices=Signal::derive(move || {
                        facets.get().map(|f| f.nature).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().nature)
                />
            </FilterDropdown>
            <FilterDropdown
                id="origine"
                label="Source"
                active_count=Signal::derive(move || usize::from(state.get().origine.is_some()))
            >
                <MonoFacetList
                    url_key="origine"
                    choices=Signal::derive(move || {
                        facets.get().map(|f| f.source).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().origine)
                />
            </FilterDropdown>
        </div>
    }
}

/// Liste mono-sélection d'un dropdown textes : lignes valeur/compte, la valeur
/// active se retire au re-clic. Sélection orpheline (URL partagée, absente de la
/// facette) affichée en tête, compteur 0.
#[component]
fn MonoFacetList(
    url_key: &'static str,
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] active: Signal<Option<String>>,
) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    let select = move |value: String, is_active: bool| {
        let next = if is_active {
            query_state::with_param(&query_map.get_untracked(), url_key, None, true)
        } else {
            query_state::with_param(&query_map.get_untracked(), url_key, Some(&value), true)
        };
        nav.go(next);
    };

    // Sélection orpheline + facettes. `Memo` : lu par les lignes du `<For>`.
    let rows = Memo::new(move |_| {
        let choices = choices.get();
        let mut out: Vec<FacetChoice> = active
            .get()
            .filter(|a| !choices.iter().any(|c| &c.value == a))
            .map(|a| FacetChoice {
                label: a.clone(),
                value: a,
                count: 0,
                parent: None,
            })
            .into_iter()
            .collect();
        out.extend(choices);
        out
    });

    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto">
            <Show when=move || rows.with(|r| r.is_empty())>
                <p class="px-2 py-1 text-xs text-[var(--color-ink-subtle)]">"Aucune valeur"</p>
            </Show>
            <For
                each=move || rows.get()
                key=|c: &FacetChoice| c.value.clone()
                children=move |c: FacetChoice| {
                    let value = c.value.clone();
                    let lookup = c.value.clone();
                    let row = Memo::new(move |_| {
                        rows.with(|l| l.iter().find(|x| x.value == lookup).cloned())
                    });
                    let fallback = c.value.clone();
                    let row_label = Signal::derive(move || {
                        row.get().map(|x| x.label).unwrap_or(fallback.clone())
                    });
                    let row_count =
                        Signal::derive(move || row.get().map(|x| x.count).unwrap_or(0));
                    let sel_value = c.value.clone();
                    let is_active =
                        Signal::derive(move || active.get().as_deref() == Some(sel_value.as_str()));
                    let row_class = move || {
                        crate::helpers::cn([
                            "flex w-full items-center gap-2.5 rounded px-2 py-1 text-left text-sm transition-colors",
                            if is_active.get() {
                                "bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
                            } else {
                                "text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
                            },
                        ])
                    };
                    view! {
                        <button
                            type="button"
                            on:click=move |_| select(value.clone(), is_active.get_untracked())
                            aria-pressed=move || is_active.get().to_string()
                            class=row_class
                        >
                            <span class="flex-1 truncate leading-tight">{row_label}</span>
                            <Show when=move || { row_count.get() > 0 }>
                                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                                    {move || row_count.get()}
                                </span>
                            </Show>
                        </button>
                    }
                }
            />
        </div>
    }
}

/// Chips des filtres textes actifs (une par clé mono-valeur) + « Tout effacer ».
#[component]
fn TextesActiveChips(
    #[prop(into)] facets: Signal<Option<ArticleSearchFacets>>,
    #[prop(into)] state: Signal<TextesQuery>,
) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();

    // (clé URL, valeur, libellé résolu dans la facette — repli valeur brute).
    let chips = Memo::new(move |_| {
        let state = state.get();
        let facets = facets.get();
        let resolve = |choices: Option<&[FacetChoice]>, v: &str| {
            choices
                .and_then(|c| c.iter().find(|x| x.value == v))
                .map(|x| x.label.clone())
                .unwrap_or_else(|| v.to_string())
        };
        let mut out: Vec<(&'static str, String, String)> = Vec::new();
        if let Some(v) = state.jurisdiction {
            let label = resolve(facets.as_ref().map(|f| f.jurisdiction.as_slice()), &v);
            out.push(("jurisdiction", v, label));
        }
        if let Some(v) = state.nature {
            let label = resolve(facets.as_ref().map(|f| f.nature.as_slice()), &v);
            out.push(("nature", v, label));
        }
        if let Some(v) = state.origine {
            let label = resolve(facets.as_ref().map(|f| f.source.as_slice()), &v);
            out.push(("origine", v, label));
        }
        out
    });

    view! {
        <Show when=move || !chips.with(|c| c.is_empty())>
            <div class="flex flex-wrap items-center gap-2">
                // Libellé dans la clé : il arrive avec les facettes, après la chip
                // (même règle que les chips décisions).
                <For
                    each=move || chips.get()
                    key=|(key, value, label): &(&'static str, String, String)| {
                        format!("{key}:{value}:{label}")
                    }
                    children=move |(key, _value, label): (&'static str, String, String)| {
                        let on_remove = move |_| {
                            nav.go(query_state::with_param(
                                &query_map.get_untracked(),
                                key,
                                None,
                                true,
                            ));
                        };
                        view! {
                            <span class="inline-flex h-7 items-center gap-1.5 rounded-full border border-[var(--color-rule)] bg-[var(--color-bordeaux-soft)] pl-3 pr-1.5 text-xs text-[var(--color-accent)]">
                                <span class="max-w-56 truncate">{label}</span>
                                <button
                                    type="button"
                                    aria-label="Retirer ce filtre"
                                    on:click=on_remove
                                    class="flex h-4 w-4 items-center justify-center rounded-full transition-colors hover:bg-[var(--color-accent)] hover:text-[var(--color-accent-foreground)]"
                                >
                                    <svg viewBox="0 0 12 12" class="h-2.5 w-2.5" aria-hidden="true">
                                        <path
                                            d="M3 3l6 6M9 3L3 9"
                                            fill="none"
                                            stroke="currentColor"
                                            stroke-width="1.5"
                                            stroke-linecap="round"
                                        />
                                    </svg>
                                </button>
                            </span>
                        }
                    }
                />
                <button
                    type="button"
                    on:click=move |_| {
                        nav.go(query_state::without_keys(
                            &query_map.get_untracked(),
                            &FILTER_KEYS,
                        ));
                    }
                    class="text-xs text-[var(--color-ink-subtle)] underline-offset-2 hover:text-[var(--color-accent)] hover:underline"
                >
                    "Tout effacer"
                </button>
            </div>
        </Show>
    }
}

/// Liste des articles trouvés (ou état vide pour la page courante). Même
/// gabarit que la liste décisions : gouttière à numéros + filets `border-t`.
#[component]
fn TextResults(hits: Vec<ArticleSearchHit>, query: String, page: u32) -> impl IntoView {
    if hits.is_empty() {
        return view! {
            <div class="border-t border-[var(--color-rule)] py-16">
                <p class="text-[var(--color-ink-muted)]">
                    {format!("Aucun article pour « {query} ».")}
                </p>
            </div>
        }
        .into_any();
    }
    let rows = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| view! { <TextHit hit=h index=i page=page /> })
        .collect_view();
    view! { <ul class="flex flex-col">{rows}</ul> }.into_any()
}

#[component]
fn TextHit(hit: ArticleSearchHit, index: usize, page: u32) -> impl IntoView {
    // Lien sur la clé canonique (`numKey`) — le serve résout en lookup exact, plus
    // de normalisation au runtime (ADR 0123 §2). `num` reste l'affichage.
    let href = format!("/loi/{}/{}", hit.code, hit.num_key);
    let position = (page as usize - 1) * TEXTES_LIMIT as usize + index + 1;
    let numeral = format!("{position:02}");
    // Fil d'ariane LEGI compacté aux deux derniers échelons : ils situent
    // l'article, le chemin complet (Partie > Livre > Titre > …) noie le hit.
    let crumb = hit.titre_path.filter(|t| !t.trim().is_empty()).map(|t| {
        let segs: Vec<&str> = t
            .split('>')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        segs[segs.len().saturating_sub(2)..].join(" › ")
    });
    view! {
        <li class="group grid grid-cols-[auto_1fr] gap-x-6 border-t border-[var(--color-rule)] py-7">
            <span aria-hidden="true" class="hit-numeral pt-0.5">
                {numeral}
            </span>
            <A href=href attr:class="flex min-w-0 flex-col gap-2 no-underline">
                <h3 class="font-sans text-lg leading-snug tracking-tight text-[var(--color-ink)] transition-colors group-hover:text-[var(--color-accent)]">
                    {format!("Article {}", format_article_num(&hit.num))}
                    <span class="text-[var(--color-ink-subtle)]">
                        {format!("\u{00A0}· {}", hit.code_title)}
                    </span>
                </h3>
                {crumb
                    .map(|c| {
                        view! {
                            <p class="truncate text-xs text-[var(--color-ink-subtle)]">{c}</p>
                        }
                    })}
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]">
                    <Highlighted text=hit.snippet />
                </p>
            </A>
        </li>
    }
}

/// Pagination du corpus articles : fenêtre `[1] … [current±2] … [total]`, chaque
/// page écrite dans `?page=` (drop si page 1), scroll top.
#[component]
fn TextPagination(current_page: u32, total_pages: u32) -> impl IntoView {
    if total_pages <= 1 {
        return ().into_any();
    }
    let query_map = use_query_map();
    let navigate = StoredValue::new(use_navigate());
    let go_to = move |page: u32| {
        let value = page.to_string();
        let qs = if page == 1 {
            query_state::with_param(&query_map.get_untracked(), "page", None, false)
        } else {
            query_state::with_param(&query_map.get_untracked(), "page", Some(&value), false)
        };
        navigate.with_value(|n| n(&query_state::search_href(&qs), Default::default()));
        scroll_to_top();
    };

    let items = build_page_range(current_page, total_pages)
        .into_iter()
        .enumerate()
        .map(|(i, item)| match item {
            PageItem::Ellipsis => view! {
                <span
                    class="px-2 text-sm text-[var(--color-ink-subtle)]"
                    data-key=format!("e{i}")
                >
                    "…"
                </span>
            }
            .into_any(),
            PageItem::Page(p) => view! {
                <PageButton
                    label=p.to_string()
                    aria_label=format!("Page {p}")
                    disabled=false
                    active=p == current_page
                    on_click=Callback::new(move |_| go_to(p))
                />
            }
            .into_any(),
        })
        .collect_view();

    view! {
        <nav aria-label="Pagination" class="flex items-center justify-center gap-1 pt-4">
            <PageButton
                label="←"
                aria_label="Page précédente"
                disabled=current_page == 1
                active=false
                on_click=Callback::new(move |_| go_to(current_page - 1))
            />
            {items}
            <PageButton
                label="→"
                aria_label="Page suivante"
                disabled=current_page == total_pages
                active=false
                on_click=Callback::new(move |_| go_to(current_page + 1))
            />
        </nav>
    }
    .into_any()
}

/// Élément de la fenêtre de pagination : numéro de page ou ellipse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageItem {
    Page(u32),
    Ellipsis,
}

/// Fenêtre `[1] … [current±2] … [total]`.
fn build_page_range(current: u32, total: u32) -> Vec<PageItem> {
    if total <= 7 {
        return (1..=total).map(PageItem::Page).collect();
    }
    let mut pages: Vec<PageItem> = Vec::new();
    let push_page = |pages: &mut Vec<PageItem>, p: u32| {
        if !pages.contains(&PageItem::Page(p)) {
            pages.push(PageItem::Page(p));
        }
    };
    push_page(&mut pages, 1);
    let start = (current.saturating_sub(2)).max(2);
    let end = (current + 2).min(total - 1);
    if start > 2 {
        pages.push(PageItem::Ellipsis);
    }
    for p in start..=end {
        push_page(&mut pages, p);
    }
    if end < total - 1 {
        pages.push(PageItem::Ellipsis);
    }
    push_page(&mut pages, total);
    pages
}

#[cfg(feature = "hydrate")]
fn scroll_to_top() {
    if let Some(window) = web_sys::window() {
        window.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

#[cfg(feature = "ssr")]
fn scroll_to_top() {}

#[component]
fn PageButton(
    #[prop(into)] label: String,
    #[prop(into)] aria_label: String,
    disabled: bool,
    active: bool,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    let class = crate::helpers::cn([
        "flex h-8 min-w-8 items-center justify-center rounded px-2 text-sm transition-colors",
        if active {
            "bg-[var(--color-ink)] text-[var(--color-parchment)]"
        } else {
            "text-[var(--color-ink-muted)] hover:bg-[var(--color-vellum)] hover:text-[var(--color-ink)]"
        },
        if disabled {
            "pointer-events-none opacity-30"
        } else {
            ""
        },
    ]);
    view! {
        <button
            type="button"
            on:click=move |_| on_click.run(())
            prop:disabled=disabled
            aria-label=aria_label
            aria-current=active.then_some("page")
            class=class
        >
            {label}
        </button>
    }
}

/// Invite initiale (requête vide).
#[component]
fn TextSearchPrompt() -> impl IntoView {
    view! {
        <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Textes & codes"
            </p>
            <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                "Cherchez dans le texte des codes et lois."
            </h2>
            <p class="max-w-prose text-[var(--color-ink-muted)]">
                "Saisissez une notion (« responsabilité du fait des choses ») ou des "
                "mots du texte ; les articles en vigueur les plus pertinents "
                "s'affichent, avec un lien vers leur page versionnée. "
            </p>
            <A
                href="/codes"
                attr:class="text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                "Parcourir le catalogue des codes"
            </A>
        </div>
    }
}
