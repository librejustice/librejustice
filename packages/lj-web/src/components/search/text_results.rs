//! The « Lois et règlements » view of `/norm-search`: a filter bar
//! `Droit ▾ · Nature ▾ · Source ▾`, the active chips and a paged list of hits
//! with an exact total. The state (query, filters, page) lives in the query
//! params, the filters named as those of `/internal/search-textes`; data
//! through `ApiClient::search_textes`.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::params::ParamsMap;
use lj_dtos::{ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, FacetChoice};

use crate::api::{ApiClient, PageParams, TextesFilters};
use crate::components::layout::{CONTENT_COL, GRID_3COL, SIDE_RAIL};
use crate::helpers::{format_article_num, group_thousands, total_pages, SEARCH_PAGE_SIZE};
use crate::jurisdiction::{home_place, use_reader_scope};
use crate::pages::decision_page::data::sendable;

use super::compact_search::highlight::Highlighted;
use super::compact_search::query_state;
use super::corpus_intro::{corpus_stats, DecisionsCorpusTeaser};
use super::cross_teaser::DecisionsTeaser;
use super::facet_widgets::{stale_counts, Nav, StaleCounts};
use super::filter_dropdown::{FilterDropdown, OpenDropdown};
use super::freshness::{answered, is_stale};
use super::journal_results::numeric_date;
use super::mode_select::{texts_modes, ModeSelect};
use super::results_rail::{skeleton_block, skeleton_rows, RAIL_COLUMN};
use super::syntax_hint::{HelpCorpus, SyntaxHint};
use super::CompactSearch;

/// Clés de filtre portées par l'URL (corpus articles). `code` borne le moteur à
/// un texte (posé par le formulaire « Rechercher dans ce code » de `/norm/{juris}/{code}`).
/// `place` is not one of them: the places read from are a setting, not a
/// filter, so no chip takes them off.
const FILTER_KEYS: [&str; 6] = [
    "jurisdiction",
    "source",
    "scope",
    "code",
    "valid_from",
    "as_of",
];

/// État de recherche lu depuis l'URL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TextesQuery {
    q: String,
    code: Option<String>,
    places: Vec<String>,
    jurisdictions: Vec<String>,
    source: Option<String>,
    scope: Option<String>,
    valid_from: Option<String>,
    as_of: Option<String>,
    mode: lj_dtos::SearchMode,
    page: u32,
}

impl TextesQuery {
    /// A URL naming no place reads from the place of the jurisdiction `home`.
    fn from_map(map: &ParamsMap, home: &str) -> Self {
        let single = |key: &str| {
            map.get(key)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let repeated = |key: &str| -> Vec<String> {
            map.get_all(key)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };
        Self {
            q: map
                .get("q")
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            code: single("code"),
            places: match repeated("place") {
                none if none.is_empty() => vec![home_place(home)],
                places => places,
            },
            jurisdictions: repeated("jurisdiction"),
            source: single("source"),
            scope: single("scope"),
            valid_from: single("valid_from"),
            as_of: single("as_of"),
            mode: match map.get("mode").as_deref() {
                Some("lexical") => lj_dtos::SearchMode::Lexical,
                Some("reference") => lj_dtos::SearchMode::Reference,
                _ => lj_dtos::SearchMode::Auto,
            },
            page: map
                .get("page")
                .and_then(|p| p.parse::<u32>().ok())
                .filter(|p| *p >= 1)
                .unwrap_or(1),
        }
    }

    /// Ce dont dépendent le total et les facettes : la question et ses filtres,
    /// la page parcourant le même ensemble.
    fn count_key(&self) -> Self {
        Self {
            page: 1,
            ..self.clone()
        }
    }

    fn filters(&self) -> TextesFilters<'_> {
        TextesFilters {
            code: self.code.as_deref(),
            places: &self.places,
            jurisdictions: &self.jurisdictions,
            source: self.source.as_deref(),
            scope: self.scope.as_deref(),
            valid_from: self.valid_from.as_deref(),
            as_of: self.as_of.as_deref(),
        }
    }
}

/// Récupère articles + facettes pour l'état courant. Requête vide ⇒ parcours du
/// corpus, servi par le même appel. Erreur repliée en message montrable (jamais
/// de reject).
async fn fetch_textes(
    state: TextesQuery,
) -> Result<ArticleSearchResponse, crate::api::client::SearchFailure> {
    let offset = (state.page - 1) * SEARCH_PAGE_SIZE;
    ApiClient::from_context()
        .search_textes(
            &state.q,
            state.filters(),
            PageParams {
                limit: SEARCH_PAGE_SIZE,
                offset,
            },
            state.mode,
            lj_dtos::SearchContext::User,
        )
        .await
        .map_err(|e| {
            #[cfg(feature = "hydrate")]
            crate::incident::signaler_recherche_refusee(e.status);
            crate::api::client::SearchFailure::from_error(&e, &state.q)
        })
}

/// The total and the facets, asked beside the hits: the server counts the
/// answer that served them, once per question and filters.
async fn fetch_textes_count(
    state: TextesQuery,
) -> Result<lj_dtos::ArticleSearchCountResponse, crate::api::client::SearchFailure> {
    ApiClient::from_context()
        .search_textes_count(
            &state.q,
            state.filters(),
            state.mode,
            lj_dtos::SearchContext::User,
        )
        .await
        .map_err(|e| crate::api::client::SearchFailure::from_error(&e, &state.q))
}

/// Corps « Lois et règlements » : grille `240px | minmax(0,1fr) | 280px` dès
/// `lg` — rail de synthèse à gauche (volumétrie + facettes cliquables), colonne
/// contenu au centre (barre, filtres, chips, liste), colonne droite (pont croisé
/// « Dans les décisions » + aide à la recherche).
///
/// Les facettes et la volumétrie alimentent rail et barre par `Effect`, mis à
/// jour sur réponse seulement : pendant un refetch ils gardent les précédentes
/// et restent interactifs. Sans requête la page est la même, servie par le mode
/// parcours : mêmes filtres, mêmes facettes, même liste.
#[component]
pub fn TextesView() -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let ui = &crate::i18n::use_ui().search;
    let query_map = use_query_map();
    let scope = use_reader_scope();
    let state = Signal::derive(move || TextesQuery::from_map(&query_map.get(), &scope.code()));
    let query = Signal::derive(move || state.get().q);
    let results = Resource::new(move || state.get(), |s| sendable(fetch_textes(s)));
    let facets = RwSignal::new(None::<ArticleSearchFacets>);
    let lang = crate::i18n::use_reader_lang().get();
    let volume = RwSignal::new(None::<i64>);
    // État auquel les facettes en main répondent : question changée, le rail
    // passe en attente plutôt que de résumer les résultats précédents ; filtre
    // changé, il reste interactif mais ses nombres comptent l'ensemble d'avant.
    let counted = RwSignal::new(None::<TextesQuery>);
    // The list's own answer, the resolved mode and the orders searched, arrives
    // before the count.
    let listed = RwSignal::new(None::<TextesQuery>);
    let served_mode = RwSignal::new(None::<lj_dtos::QueryMode>);
    let served_jurisdictions = RwSignal::new(None::<Vec<String>>);
    Effect::new(move |_| {
        if let Some(Ok(resp)) = results.get() {
            served_mode.set(Some(resp.query_mode));
            served_jurisdictions.set(Some(resp.jurisdictions));
            listed.set(Some(state.get_untracked().count_key()));
            // A listing and a reference answer carry their count.
            if let (Some(total), Some(f)) = (resp.total, resp.facets) {
                volume.set(Some(total));
                facets.set(Some(crate::i18n::relabel_article_facets(lang, f)));
                counted.set(Some(state.get_untracked().count_key()));
            }
        }
    });
    let count = Resource::new(
        move || state.get().count_key(),
        |s| sendable(fetch_textes_count(s)),
    );
    Effect::new(move |_| {
        if let Some(Ok(c)) = count.get() {
            volume.set(Some(c.total));
            facets.set(Some(crate::i18n::relabel_article_facets(lang, c.facets)));
            counted.set(Some(state.get_untracked().count_key()));
        }
    });
    let rail_loading = Signal::derive(move || {
        counted.with(|c| c.as_ref().map(|k| k.q.clone())) != Some(query.get())
    });
    let count_key = Signal::derive(move || state.get().count_key());
    let stale = Memo::new(move |_| is_stale(counted.into(), count_key).get());
    provide_context(StaleCounts(stale.into()));
    let shown_volume = answered(volume.into(), counted.into(), count_key);
    // The resolved mode asserts, so it goes silent while an answer is in flight.
    let shown_mode = answered(served_mode.into(), listed.into(), count_key);
    let shown_jurisdictions = answered(served_jurisdictions.into(), listed.into(), count_key);
    let has_query = Signal::derive(move || !query.get().is_empty());
    let stats = corpus_stats();

    view! {
        <div class=GRID_3COL>
            <aside aria-label=ui.results_summary class=SIDE_RAIL>
                <TextesRail
                    facets=facets
                    state=state
                    loading=rail_loading
                    searched=shown_jurisdictions
                />
            </aside>
            <div class=CONTENT_COL>
                <div class="flex flex-col gap-2">
                    <CompactSearch />
                    // Volumétrie mobile sous la barre, l'aide calée à droite (sur
                    // desktop les deux vivent dans la colonne droite).
                    <div class="flex items-baseline gap-3 lg:hidden">
                        {move || {
                            shown_volume
                                .get()
                                .map(|total| {
                                    view! {
                                        <p class="text-sm text-[var(--color-ink-subtle)]">
                                            <span class="font-medium text-[var(--color-ink)]">
                                                {(numbers.results)(total)}
                                            </span>
                                        </p>
                                    }
                                })
                        }}
                        <SyntaxHint corpus=HelpCorpus::Textes class="ml-auto shrink-0" />
                    </div>
                    <div class="lg:hidden">
                        <ModeSelect resolved=shown_mode modes=texts_modes(&crate::i18n::use_ui().search) class="w-full" />
                    </div>
                </div>
                <div class="flex min-w-0 flex-col gap-4">
                    <TextesFilterBar facets=facets state=state />
                    <TextesActiveChips facets=facets state=state />
                    <Suspense fallback=move || {
                        view! {
                            <p class="text-sm text-[var(--color-ink-subtle)]">{ui.pending}</p>
                        }
                    }>
                        {move || Suspend::new(async move {
                            let st = state.get_untracked();
                            match results.await {
                                Ok(resp) => {
                                    view! { <TextResultsLayout resp=resp state=st /> }.into_any()
                                }
                                Err(failure) => {
                                    let (title, hint) = failure.words(crate::i18n::use_ui());
                                    view! {
                                        <div role="alert" class="text-sm">
                                            <p class="text-[var(--color-accent)]">{title}</p>
                                            <p class="mt-1 text-[var(--color-ink-subtle)]">
                                                {hint}
                                            </p>
                                        </div>
                                    }
                                        .into_any()
                                }
                            }
                        })}
                    </Suspense>
                </div>
            </div>
            // Colonne droite : volumétrie, aide à la recherche, puis pont croisé vers
            // le moteur décisions. Sans question à poser, le pont montre le fonds
            // jurisprudentiel. Sous lg la colonne disparaît.
            <aside aria-label=ui.see_also class=SIDE_RAIL>
                <div class="flex flex-col gap-4">
                    <TextesSummary query=query volume=shown_volume />
                    <ModeSelect resolved=shown_mode modes=texts_modes(&crate::i18n::use_ui().search) class="w-full" />
                    <div class="flex self-start">
                        <SyntaxHint corpus=HelpCorpus::Textes />
                    </div>
                    <Show
                        when=move || has_query.get()
                        fallback=move || view! { <DecisionsCorpusTeaser stats=stats /> }
                    >
                        <DecisionsTeaser query=query />
                    </Show>
                </div>
            </aside>
        </div>
    }
}

/// Volumétrie desktop, en tête de la colonne droite : « N résultats / pour
/// « q » », ou l'état d'attente tant que la réponse n'est pas arrivée. En
/// parcours il n'y a pas de question, donc pas de ligne « pour ».
#[component]
fn TextesSummary(
    #[prop(into)] query: Signal<String>,
    #[prop(into)] volume: Signal<Option<i64>>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let ui = &crate::i18n::use_ui().search;
    move || {
        let q = query.get();
        let Some(total) = volume.get() else {
            if q.is_empty() {
                return view! {
                    <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">
                        {ui.loading_corpus}
                    </h2>
                }
                .into_any();
            }
            return view! {
                <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">
                    {ui.searching}
                </h2>
            }
            .into_any();
        };
        if q.is_empty() {
            return view! {
                <div class="flex flex-col gap-1">
                    <h2 class="font-sans text-xl text-[var(--color-ink)]">
                        {(numbers.results)(total)}
                    </h2>
                    <p class="text-xs text-[var(--color-ink-subtle)]">
                        {ui.texts_by_last_change}
                    </p>
                </div>
            }
            .into_any();
        }
        view! {
            <div class="flex flex-col gap-1">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {(numbers.results)(total)}
                </h2>
                <p class="text-xs text-[var(--color-ink-subtle)]">{ui.in_force}</p>
            </div>
        }
        .into_any()
    }
}

/// Layout chargé : liste + pagination (la volumétrie vit dans la colonne
/// droite, et en ligne compacte mobile dans la colonne de contenu).
#[component]
fn TextResultsLayout(resp: ArticleSearchResponse, state: TextesQuery) -> impl IntoView {
    let pages = total_pages(resp.served) as u32;
    let page = state.page;
    let query = state.q.clone();
    let lang = crate::i18n::use_reader_lang().get();
    let mut hits = resp.hits;
    let mut pinned = resp.pinned;
    let titles = hits.iter_mut().map(|h| &mut h.code_title);
    for title in titles.chain(pinned.iter_mut().map(|p| &mut p.code_title)) {
        crate::i18n::relabel_title(lang, title);
    }
    let more = resp.pinned_more;
    // A result opens where it was found: the Swiss civil code reached from a
    // Swiss answer is read from Switzerland, not from the metropole.
    let place = resp
        .reader_place
        .as_ref()
        .map(|p| format!("?place={}", p.uid))
        .unwrap_or_default();

    view! {
        <section aria-label=crate::i18n::use_ui().search.results class="flex min-w-0 flex-col gap-6">
            <PinnedArticlesCard pinned=pinned more=more place=place.clone() />
            <TextResults hits=hits query=query page=page place=place />
            <TextPagination current_page=page total_pages=pages />
        </section>
    }
}

#[component]
fn PinnedArticlesCard(
    pinned: Vec<lj_dtos::PinnedArticle>,
    more: bool,
    place: String,
) -> impl IntoView {
    if pinned.is_empty() {
        return ().into_any();
    }
    let label = crate::i18n::use_ui().search.maybe_looking_for;
    view! {
        <section
            aria-label=label
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 px-4 py-3"
        >
            <p class="font-sans text-sm text-[var(--color-ink-subtle)]">{label}</p>
            <ul class="mt-2 flex flex-col gap-2">
                {pinned
                    .into_iter()
                    .map(|a| view! { <PinnedArticleRow article=a place=place.clone() /> })
                    .collect::<Vec<_>>()}
            </ul>
            <Show when=move || more>
                <PinnedArticlesMore />
            </Show>
        </section>
    }
    .into_any()
}

#[component]
fn PinnedArticleRow(article: lj_dtos::PinnedArticle, place: String) -> impl IntoView {
    let text = lj_vocab::norm_path(&article.jurisdiction, &article.code);
    let (href, titre) = match article.num_key.as_str() {
        "" => (format!("{text}{place}"), article.code_title.clone()),
        key => (
            format!("{text}/{key}{place}"),
            (crate::i18n::use_ui().search.article_of)(&article.code_title, &article.num),
        ),
    };
    let chemin = article
        .titre_path
        .as_deref()
        .and_then(|p| p.rsplit('>').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    view! {
        <li class="flex flex-col gap-0.5">
            <A
                href=href
                attr:class="font-sans text-base leading-snug text-[var(--color-ink)] no-underline transition-colors hover:text-[var(--color-accent)]"
            >
                {titre}
            </A>
            {chemin
                .map(|c| {
                    view! {
                        <p class="truncate font-sans text-xs text-[var(--color-ink-subtle)]">{c}</p>
                    }
                })}
        </li>
    }
}

#[component]
fn PinnedArticlesMore() -> impl IntoView {
    let query_map = use_query_map();
    let href = move || {
        let qs = query_state::with_param(&query_map.get(), "mode", Some("reference"), true);
        query_state::search_href(&qs)
    };
    view! {
        <A
            href=href
            attr:class="mt-3 inline-block text-xs text-[var(--color-ink-subtle)] underline decoration-dotted underline-offset-4 transition-colors hover:text-[var(--color-accent)]"
        >
            {crate::i18n::use_ui().search.other_articles_match}
        </A>
    }
}

/// Lignes par bloc de facettes du rail.
const RAIL_ROWS: usize = 5;

/// Left rail of `/norm-search`: clickable facet blocks (Portée, Droit,
/// Sources). A click goes through the same single-choice URL change as the
/// bar's dropdowns (clicking the active value again removes it, back to page
/// 1). `loading`: the facets at hand answer another query, so the rows stay
/// mute rather than summarise wrongly.
#[component]
fn TextesRail(
    #[prop(into)] facets: Signal<Option<ArticleSearchFacets>>,
    #[prop(into)] state: Signal<TextesQuery>,
    #[prop(into)] loading: Signal<bool>,
    #[prop(into)] searched: Signal<Option<Vec<String>>>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    // Held here rather than in the block, which every answer redraws: the list
    // stays open, and fetched once, while places are ticked one after another.
    let lang = crate::i18n::use_reader_lang().get();
    let vocabulary = Resource::new(
        || (),
        move |_| sendable(crate::pages::law_page::data::fetch_places(lang)),
    );
    let picker_open = RwSignal::new(false);
    let scopes = move || {
        rail_facet_block(
            ui.rail_scope,
            "scope",
            facets.get().map(|f| f.scope).unwrap_or_default(),
            state.get().scope,
        )
    };
    let jurisdictions = move || {
        let active = state.get().jurisdictions;
        let tree = facets
            .get()
            .as_ref()
            .map(jurisdiction_tree)
            .unwrap_or_default();
        jurisdiction_block(
            rail_tree(tree, &active),
            active,
            searched.get().unwrap_or_default(),
            vocabulary,
            state.get().places,
            picker_open,
        )
    };
    let sources = move || {
        rail_facet_block(
            ui.rail_sources,
            "source",
            facets.get().map(|f| f.source).unwrap_or_default(),
            state.get().source,
        )
    };

    move || {
        if loading.get() {
            return view! {
                <div class=RAIL_COLUMN>
                    {skeleton_block(ui.rail_scope, skeleton_rows(RAIL_ROWS))}
                    {skeleton_block(ui.rail_law, skeleton_rows(RAIL_ROWS))}
                    {skeleton_block(ui.rail_sources, skeleton_rows(RAIL_ROWS))}
                </div>
            }
            .into_any();
        }
        view! {
            <div class=RAIL_COLUMN>
                {scopes}
                {jurisdictions}
                {sources}
            </div>
        }
        .into_any()
    }
}

const RAIL_ROW: &str = "group flex w-full items-baseline justify-between gap-3 text-left";
const RAIL_LABEL_ACTIVE: &str =
    "truncate text-[13px] leading-snug font-medium text-[var(--color-bordeaux)]";
const RAIL_LABEL: &str = "truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors group-hover:text-[var(--color-ink)]";
const RAIL_LABEL_ECHO: &str = "truncate rounded bg-[var(--color-rule)]/60 px-1 text-[13px] leading-snug text-[var(--color-ink-subtle)] transition-colors group-hover:text-[var(--color-ink)]";
const RAIL_COUNT: &str =
    "shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]";

/// The places results are read from, set from the rail: what applies in any of
/// them is served. A heading ticks or clears its places at once; the current URL
/// is kept, only `place` rewritten, and the jurisdiction's place alone, or no
/// place at all, is written as no `place`.
#[component]
fn ReaderPlacePicker(
    vocabulary: Resource<Vec<lj_dtos::PlaceNode>>,
    current: Vec<String>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let query_map = use_query_map();
    let nav = Nav::new();
    let scope = use_reader_scope();
    let home = StoredValue::new(home_place(&untrack(|| scope.code())));
    let go = move |next: Vec<String>| {
        let next = match next.as_slice() {
            [] => Vec::new(),
            [only] if home.with_value(|h| only == h) => Vec::new(),
            _ => next,
        };
        nav.go(query_state::with_values(
            &query_map.get_untracked(),
            "place",
            &next,
        ));
    };
    view! {
        <Suspense fallback=|| ()>
            {move || {
                let current = current.clone();
                Suspend::new(async move {
                    let places = vocabulary.await;
                    if places.is_empty() {
                        return ().into_any();
                    }
                    let groups = crate::pages::law_page::place_groups(&places);
                    let picked: Vec<String> = groups
                        .iter()
                        .flat_map(|(_, members)| members)
                        .filter(|p| current.contains(&p.uid))
                        .map(|p| p.label.clone())
                        .collect();
                    let summary = match picked.as_slice() {
                        [] => String::new(),
                        [one] => one.clone(),
                        [a, b] => format!("{a}, {b}"),
                        many => (ui.n_places)(many.len()),
                    };
                    let is_default = home.with_value(|h| current == [h.as_str()]);
                    let reset_label = home.with_value(|h| match h.as_str() {
                        lj_dtos::DEFAULT_READER_PLACE => {
                            ui.back_to_metropolitan_france.to_string()
                        }
                        _ => {
                            let label = places.iter().find(|p| &p.uid == h).map(|p| p.label.as_str());
                            (ui.back_to_default_place)(label.unwrap_or(h))
                        }
                    });
                    let full = current.len() >= lj_dtos::MAX_READER_PLACES;
                    let sections = groups
                        .into_iter()
                        .map(|(label, members)| {
                            let uids: Vec<String> = members.iter().map(|p| p.uid.clone()).collect();
                            let all_on = uids.iter().all(|u| current.contains(u));
                            let fits = current.len()
                                + uids.iter().filter(|u| !current.contains(u)).count()
                                <= lj_dtos::MAX_READER_PLACES;
                            let heading = {
                                let current = current.clone();
                                view! {
                                    <label class="flex items-center gap-2 pt-2 text-[11px] uppercase tracking-[0.12em] text-[var(--color-ink-subtle)]">
                                        <input
                                            type="checkbox"
                                            prop:checked=all_on
                                            prop:disabled=!all_on && !fits
                                            on:change=move |_| {
                                                let mut next = current.clone();
                                                if all_on {
                                                    next.retain(|u| !uids.contains(u));
                                                } else {
                                                    for u in &uids {
                                                        if !next.contains(u) {
                                                            next.push(u.clone());
                                                        }
                                                    }
                                                }
                                                go(next);
                                            }
                                        />
                                        {label}
                                    </label>
                                }
                            };
                            let rows = members
                                .into_iter()
                                .map(|p| {
                                    let on = current.contains(&p.uid);
                                    let current = current.clone();
                                    let uid = p.uid.clone();
                                    view! {
                                        <label class="flex items-center gap-2 pl-4 text-[13px] text-[var(--color-ink-muted)]">
                                            <input
                                                type="checkbox"
                                                prop:checked=on
                                                prop:disabled=!on && full
                                                on:change=move |_| {
                                                    let mut next = current.clone();
                                                    if on {
                                                        next.retain(|u| *u != uid);
                                                    } else {
                                                        next.push(uid.clone());
                                                    }
                                                    go(next);
                                                }
                                            />
                                            {p.label}
                                        </label>
                                    }
                                })
                                .collect_view();
                            view! { <div class="flex flex-col gap-1">{heading}{rows}</div> }
                        })
                        .collect_view();
                    view! {
                        <div class="flex flex-col gap-1 pt-1">
                            <button
                                type="button"
                                class=RAIL_ROW
                                aria-expanded=move || open.get().to_string()
                                on:click=move |_| open.update(|o| *o = !*o)
                            >
                                <span class="text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                                    {crate::i18n::use_ui().norm.read_from}
                                </span>
                                <span class="truncate text-[13px] text-[var(--color-ink)]">{summary}</span>
                            </button>
                            <div
                                class="flex max-h-80 flex-col gap-1 overflow-y-auto rounded-md border border-[var(--color-rule)] px-2 pb-2"
                                class:hidden=move || !open.get()
                            >
                                {sections}
                                {(!is_default)
                                    .then(|| {
                                        view! {
                                            <button
                                                type="button"
                                                class="pt-2 text-left text-[12px] text-[var(--color-bordeaux)] hover:underline"
                                                on:click=move |_| go(Vec::new())
                                            >
                                                {reset_label}
                                            </button>
                                        }
                                    })}
                            </div>
                        </div>
                    }
                        .into_any()
                })
            }}
        </Suspense>
    }
}

/// The jurisdiction block opens on the automatic position, active while no
/// filter is stated; the corpora the engine chose are echoed in grey. The
/// places read from close it, since they are what the automatic position
/// answers to.
fn jurisdiction_block(
    choices: Vec<FacetChoice>,
    active: Vec<String>,
    searched: Vec<String>,
    vocabulary: Resource<Vec<lj_dtos::PlaceNode>>,
    picked: Vec<String>,
    picker_open: RwSignal<bool>,
) -> AnyView {
    let numbers = &crate::i18n::use_ui().numbers;
    if choices.is_empty() {
        return ().into_any();
    }
    let query_map = use_query_map();
    let nav = Nav::new();
    let stale = stale_counts();
    let auto = active.is_empty();
    let auto_row = view! {
        <button
            type="button"
            class=RAIL_ROW
            aria-pressed=auto.to_string()
            on:click=move |_| {
                nav.go(query_state::with_param(&query_map.get_untracked(), "jurisdiction", None, true));
            }
        >
            <span class=if auto { RAIL_LABEL_ACTIVE } else { RAIL_LABEL }>{crate::i18n::use_ui().search.automatic}</span>
        </button>
    };
    let rows = choices
        .into_iter()
        .map(|c| {
            let is_active = active.contains(&c.value);
            let echoed = auto && searched.contains(&c.value);
            let row_class = match tree_depth(&c) {
                0 => RAIL_ROW.to_string(),
                1 => format!("{RAIL_ROW} pl-4"),
                _ => format!("{RAIL_ROW} pl-8"),
            };
            let count = c.count;
            let counted = count > 0;
            let label_class = match (is_active, echoed) {
                (true, _) => RAIL_LABEL_ACTIVE,
                (false, true) => RAIL_LABEL_ECHO,
                (false, false) => RAIL_LABEL,
            };
            let value = c.value;
            view! {
                <button
                    type="button"
                    class=row_class
                    title=c.label.clone()
                    on:click=move |_| {
                        let next = if is_active {
                            query_state::toggle_multi(&query_map.get_untracked(), "jurisdiction", &value)
                        } else {
                            query_state::with_param(
                                &query_map.get_untracked(),
                                "jurisdiction",
                                Some(&value),
                                true,
                            )
                        };
                        nav.go(next);
                    }
                >
                    <span class=label_class>{c.label.clone()}</span>
                    <Show when=move || counted && !stale.get()>
                        <span class=RAIL_COUNT>{group_thousands(numbers, count)}</span>
                    </Show>
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {crate::i18n::use_ui().search.rail_law}
            </p>
            {auto_row}
            {rows}
            <ReaderPlacePicker vocabulary=vocabulary current=picked open=picker_open />
        </div>
    }
    .into_any()
}

/// Bloc liste du rail textes : titre + lignes `libellé … compte` cliquables,
/// mono-sélection (la ligne active se retire au re-clic). Vide ⇒ rien.
fn rail_facet_block(
    title: &'static str,
    url_key: &'static str,
    choices: Vec<FacetChoice>,
    active: Option<String>,
) -> AnyView {
    let numbers = &crate::i18n::use_ui().numbers;
    if choices.is_empty() {
        return ().into_any();
    }
    let query_map = use_query_map();
    let nav = Nav::new();
    let stale = stale_counts();
    let rows = choices
        .into_iter()
        .take(RAIL_ROWS)
        .map(|c| {
            let is_active = active.as_deref() == Some(c.value.as_str());
            let count = c.count;
            let label_class = if is_active {
                RAIL_LABEL_ACTIVE
            } else {
                RAIL_LABEL
            };
            let value = c.value;
            view! {
                <button
                    type="button"
                    class=RAIL_ROW
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
                    <Show when=move || !stale.get()>
                        <span class=RAIL_COUNT>{group_thousands(numbers, count)}</span>
                    </Show>
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

/// The orders, each followed by the types the hits sit on under it, depth first.
fn jurisdiction_tree(facets: &ArticleSearchFacets) -> Vec<FacetChoice> {
    fn under(out: &mut Vec<FacetChoice>, paths: &[FacetChoice], parent: &str) {
        for child in paths.iter().filter(|c| c.parent.as_deref() == Some(parent)) {
            out.push(child.clone());
            under(out, paths, &child.value);
        }
    }
    let mut out = Vec::new();
    for order in &facets.jurisdiction {
        out.push(order.clone());
        under(&mut out, &facets.path, &order.value);
    }
    out
}

/// The rail's share of the tree: every order and its first types, the active
/// row kept wherever it sits. The bar's dropdown holds the whole tree.
fn rail_tree(tree: Vec<FacetChoice>, active: &[String]) -> Vec<FacetChoice> {
    let mut types_shown = 0;
    tree.into_iter()
        .filter(|c| {
            let is_active = active.contains(&c.value);
            match tree_depth(c) {
                0 => {
                    types_shown = 0;
                    true
                }
                1 => {
                    types_shown += 1;
                    types_shown <= RAIL_ROWS || is_active
                }
                _ => is_active,
            }
        })
        .collect()
}

/// A row's depth under its order, `FR.LOI.ORGANIQUE` 2; a row with no parent
/// sits at 0, whatever dots its value carries.
fn tree_depth(choice: &FacetChoice) -> usize {
    match choice.parent {
        Some(_) => choice.value.matches('.').count(),
        None => 0,
    }
}

/// Filter bar of the text search (the « Lois et règlements » template):
/// `Droit ▾ · Portée ▾ · Source ▾`, `Droit` an order or a type under it. Each
/// facet takes one value: clicking a value sets it (`?<key>=<value>`), clicking
/// it again removes it, and any change drops `page` (back to page 1).
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
            aria-label=crate::i18n::use_ui().search.filters
            class="flex items-center gap-2 overflow-x-auto pb-1"
        >
            <FilterDropdown
                id="jurisdiction"
                label=crate::i18n::use_ui().search.rail_law
                active_count=Signal::derive(move || state.get().jurisdictions.len())
            >
                <MonoFacetList
                    url_key="jurisdiction"
                    choices=Signal::derive(move || {
                        facets.get().as_ref().map(jurisdiction_tree).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().jurisdictions)
                />
            </FilterDropdown>
            <FilterDropdown
                id="scope"
                label=crate::i18n::use_ui().search.rail_scope
                active_count=Signal::derive(move || usize::from(state.get().scope.is_some()))
            >
                <MonoFacetList
                    url_key="scope"
                    choices=Signal::derive(move || {
                        facets.get().map(|f| f.scope).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().scope.into_iter().collect::<Vec<_>>())
                />
            </FilterDropdown>
            <FilterDropdown
                id="source"
                label=crate::i18n::use_ui().search.filter_source
                active_count=Signal::derive(move || usize::from(state.get().source.is_some()))
            >
                <MonoFacetList
                    url_key="source"
                    choices=Signal::derive(move || {
                        facets.get().map(|f| f.source).unwrap_or_default()
                    })
                    active=Signal::derive(move || state.get().source.into_iter().collect::<Vec<_>>())
                />
            </FilterDropdown>
        </div>
    }
}

/// Liste d'un dropdown textes : lignes valeur/compte, un clic pose la valeur
/// seule, la valeur active se retire au re-clic. Sélection orpheline (URL
/// partagée, absente de la facette) affichée en tête, compteur 0.
#[component]
fn MonoFacetList(
    url_key: &'static str,
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] active: Signal<Vec<String>>,
) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    let stale = stale_counts();
    let select = move |value: String, is_active: bool| {
        let next = if is_active {
            query_state::toggle_multi(&query_map.get_untracked(), url_key, &value)
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
            .into_iter()
            .filter(|a| !choices.iter().any(|c| &c.value == a))
            .map(|a| FacetChoice {
                label: a.clone(),
                value: a,
                count: 0,
                parent: None,
                hint: None,
            })
            .collect();
        out.extend(choices);
        out
    });

    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto">
            <Show when=move || rows.with(|r| r.is_empty())>
                <p class="px-2 py-1 text-xs text-[var(--color-ink-subtle)]">{crate::i18n::use_ui().search.no_value}</p>
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
                    let is_active = Signal::derive(move || active.get().contains(&sel_value));
                    let indent = match tree_depth(&c) {
                        0 => "pl-2",
                        1 => "pl-6",
                        _ => "pl-10",
                    };
                    let row_class = move || {
                        crate::helpers::cn([
                            "flex w-full items-center gap-2.5 rounded pr-2 py-1 text-left text-sm transition-colors",
                            indent,
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
                            <Show when=move || { row_count.get() > 0 && !stale.get() }>
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

/// The chips of the active norms filters, one per key with the date bounds together, and the clear-all button.
#[component]
fn TextesActiveChips(
    #[prop(into)] facets: Signal<Option<ArticleSearchFacets>>,
    #[prop(into)] state: Signal<TextesQuery>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let dates = &crate::i18n::use_ui().dates;
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
        if let Some(v) = state.code {
            // Le slug est lisible une fois humanisé (« code-de-justice-… » → « Code de
            // justice… ») — pas de facette pour porter le titre.
            let label = (ui.chip_in_text)(&humanize_slug(&v));
            out.push(("code", v, label));
        }
        if !state.jurisdictions.is_empty() {
            let tree = facets.as_ref().map(jurisdiction_tree);
            let labels: Vec<String> = state
                .jurisdictions
                .iter()
                .map(|v| resolve(tree.as_deref(), v))
                .collect();
            out.push((
                "jurisdiction",
                state.jurisdictions.join(","),
                labels.join(", "),
            ));
        }
        if let Some(v) = state.source {
            let label = resolve(facets.as_ref().map(|f| f.source.as_slice()), &v);
            out.push(("source", v, label));
        }
        if let Some(v) = state.scope {
            let label = resolve(facets.as_ref().map(|f| f.scope.as_slice()), &v);
            out.push(("scope", v, label));
        }
        let day = |iso: &str| numeric_date(dates, iso).unwrap_or_else(|| iso.to_string());
        let dates = match (state.valid_from.as_deref(), state.as_of.as_deref()) {
            (Some(from), Some(at)) => Some((ui.chip_dates_between)(&day(from), &day(at))),
            (Some(from), None) => Some((ui.chip_dates_from)(&day(from))),
            (None, Some(at)) => Some((ui.chip_in_force_at)(&day(at))),
            (None, None) => None,
        };
        if let Some(label) = dates {
            out.push(("dates", String::new(), label));
        }
        out
    });

    view! {
        <Show when=move || !chips.with(|c| c.is_empty())>
            <div class="flex flex-wrap items-center gap-2">
                // Libellé dans la clé : il arrive avec les facettes, après la chip.
                <For
                    each=move || chips.get()
                    key=|(key, value, label): &(&'static str, String, String)| {
                        format!("{key}:{value}:{label}")
                    }
                    children=move |(key, _value, label): (&'static str, String, String)| {
                        let keys: Vec<&'static str> = match key {
                            "dates" => vec!["valid_from", "as_of", "page"],
                            _ => vec![key, "page"],
                        };
                        let on_remove = move |_| {
                            nav.go(query_state::without_keys(&query_map.get_untracked(), &keys));
                        };
                        view! {
                            <span class="inline-flex h-7 items-center gap-1.5 rounded-full border border-[var(--color-rule)] bg-[var(--color-bordeaux-soft)] pl-3 pr-1.5 text-xs text-[var(--color-accent)]">
                                <span class="max-w-56 truncate">{label}</span>
                                <button
                                    type="button"
                                    aria-label=ui.remove_filter
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
                    {ui.clear_all}
                </button>
            </div>
        </Show>
    }
}

/// Liste des articles trouvés (ou état vide pour la page courante). Même
/// gabarit que la liste décisions : gouttière à numéros + filets `border-t`.
#[component]
fn TextResults(
    hits: Vec<ArticleSearchHit>,
    query: String,
    page: u32,
    place: String,
) -> impl IntoView {
    if hits.is_empty() {
        return view! {
            <div class="border-t border-[var(--color-rule)] py-16">
                <p class="text-[var(--color-ink-muted)]">
                    {match query.is_empty() {
                        true => crate::i18n::use_ui().search.no_text_for_filters.to_string(),
                        false => (crate::i18n::use_ui().search.no_article_for)(&query),
                    }}
                </p>
            </div>
        }
        .into_any();
    }
    let rows = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| view! { <TextHit hit=h index=i page=page place=place.clone() /> })
        .collect_view();
    view! { <ul class="flex flex-col">{rows}</ul> }.into_any()
}

#[component]
fn TextHit(hit: ArticleSearchHit, index: usize, page: u32, place: String) -> impl IntoView {
    // Lien sur la clé canonique (`numKey`) — le serve résout en lookup exact.
    // `num` reste l'affichage ; `num` vide = hit « texte entier » (texte à corps)
    // → page du texte.
    let is_whole_text = hit.num_key.is_empty();
    let text = lj_vocab::norm_path(&hit.jurisdiction, &hit.code);
    let href = if is_whole_text {
        format!("{text}{place}")
    } else {
        format!("{text}/{}{place}", hit.num_key)
    };
    let position = (page as usize - 1) * SEARCH_PAGE_SIZE as usize + index + 1;
    let numeral = format!("{position:02}");
    let meta_line = article_meta_line(&hit);
    view! {
        <li class="group grid grid-cols-[auto_1fr] gap-x-6 border-t border-[var(--color-rule)] py-7">
            <span aria-hidden="true" class="hit-numeral pt-0.5">
                {numeral}
            </span>
            <A href=href attr:class="flex min-w-0 flex-col gap-2 no-underline">
                <h3 class="font-sans text-lg leading-snug tracking-tight text-[var(--color-ink)] transition-colors group-hover:text-[var(--color-accent)]">
                    {if is_whole_text {
                        view! { {hit.code_title.clone()} }.into_any()
                    } else {
                        view! {
                            {(crate::i18n::use_ui().norm.article_n)(&format_article_num(&hit.num))}
                            <span class="text-[var(--color-ink-subtle)]">
                                {format!("\u{00A0}· {}", hit.code_title)}
                            </span>
                        }
                        .into_any()
                    }}
                </h3>
                {meta_line}
                <p class="text-[0.95rem] leading-relaxed text-[var(--color-ink-muted)]">
                    <Highlighted text=hit.snippet />
                </p>
            </A>
        </li>
    }
}

pub(super) fn article_meta_line(hit: &ArticleSearchHit) -> Option<impl IntoView> {
    // Fil d'ariane LEGI compacté aux deux derniers échelons : ils situent
    // l'article, le chemin complet (Partie > Livre > Titre > …) noie le hit.
    let crumb = hit
        .titre_path
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(|t| {
            let segs: Vec<&str> = t
                .split('>')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            segs[segs.len().saturating_sub(2)..].join(" › ")
        });
    // Badge d'état : « JO du <date> » pour une publication (`source = "jorf"`,
    // jamais d'état DILA), libellé d'état pour une version quand il porte un
    // signal — `VIGUEUR` est l'état par défaut du droit servi dans la fenêtre.
    let badge = if hit.source == "jorf" {
        hit.date_debut
            .as_deref()
            .and_then(|d| numeric_date(&crate::i18n::use_ui().dates, d))
            .map(|d| (crate::i18n::use_ui().search.journal_of)(&d))
    } else {
        hit.status
            .as_deref()
            .filter(|s| *s != "VIGUEUR")
            .and_then(crate::i18n::use_ui().norm.article_status)
            .map(str::to_string)
    };
    (badge.is_some() || crumb.is_some()).then(|| {
        view! {
            <p class="flex min-w-0 items-baseline gap-2 text-xs text-[var(--color-ink-subtle)]">
                {badge
                    .map(|b| {
                        view! {
                            <span class="shrink-0 rounded border border-[var(--color-rule)] px-1.5 py-0.5">
                                {b}
                            </span>
                        }
                    })}
                {crumb.map(|c| view! { <span class="truncate">{c}</span> })}
            </p>
        }
    })
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
                    aria_label=(crate::i18n::use_ui().search.page_n)(p)
                    disabled=false
                    active=p == current_page
                    on_click=Callback::new(move |_| go_to(p))
                />
            }
            .into_any(),
        })
        .collect_view();

    view! {
        <nav aria-label=crate::i18n::use_ui().search.pagination class="flex items-center justify-center gap-1 pt-4">
            <PageButton
                label="←"
                aria_label=crate::i18n::use_ui().search.previous_page
                disabled=current_page == 1
                active=false
                on_click=Callback::new(move |_| go_to(current_page - 1))
            />
            {items}
            <PageButton
                label="→"
                aria_label=crate::i18n::use_ui().search.next_page
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

/// Libellé approximatif d'un slug de code (tirets → espaces, initiale en
/// capitale) : « code-de-justice-administrative » → « Code de justice
/// administrative » (accents perdus, assumé — chip de portée, pas un titre).
fn humanize_slug(slug: &str) -> String {
    let mut s = slug.replace('-', " ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La page parcourt le même ensemble : ses nombres ne se taisent pas.
    #[test]
    fn count_key_ignore_la_page() {
        let base = TextesQuery::default();
        let page4 = TextesQuery {
            page: 4,
            ..base.clone()
        };
        assert_eq!(base.count_key(), page4.count_key());

        let filtre = TextesQuery {
            jurisdictions: vec!["FR.CODE".to_string()],
            ..base.clone()
        };
        assert_ne!(base.count_key(), filtre.count_key());
    }
}
