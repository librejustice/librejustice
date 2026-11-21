//! Page de recherche de décisions `/decision-search` (gabarit du secteur) :
//! CompactSearch + barre de filtres horizontale + résultats. La recherche de
//! textes vit sur sa propre page `/norm-search` (moteurs et filtres distincts,
//! aucune recherche transverse). État dans les query params ; données via
//! `leptos-fetch` (SWR) ; DTO `lj-dtos`.
//!
//! Anatomie alignée sur la page décision (`DecisionLayout`) : même conteneur
//! `max-w-[92rem]` et même grille `240px | minmax(0,1fr) | 280px` dès `lg` —
//! colonne gauche = rail de facettes (`SearchRail`), colonne DROITE (280px) =
//! volumétrie + réglages d'affichage + aide + pont croisé « Dans les textes ».
//! La colonne de contenu (barre, filtres, liste) prend le reste, borné à
//! `max-w-3xl` ; sous `lg` les réglages d'affichage y retombent.
//!
//! `<Title>` dynamique reflétant la requête `q` de l'URL ; hérite de la
//! description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use leptos_router::hooks::use_query_map;
use lj_dtos::{PinnedDecision, QueryMode, SearchResponse};

use crate::components::client_only::ClientOnly;
use crate::components::decision_bar::{carry_seed, search_origin, ResultNavSeed};
use crate::components::layout::{CONTENT_COL, GRID_3COL, SHELL, SIDE_RAIL};
use crate::components::search::compact_search::search_data::{
    cached_search, count_resource, search_is_loading, search_resource, SearchResult, SeenSearchKeys,
};
use crate::components::search::compact_search::search_state::{key_from_map, SearchKey};
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::view_prefs::CardView;
use crate::components::search::{
    answered, corpus_stats, decision_modes, effective_card_view, is_stale, ActiveFilterChips,
    CompactSearch, DecisionsFilterBar, ModeSelect, Pagination, ResultEmpty, ResultError,
    ResultList, ResultSkeleton, SearchRail, SortSelect, StaleCounts, SyntaxHint,
    TextesCorpusTeaser, TextesTeaser, ViewControls,
};
use crate::helpers::{total_pages, SEARCH_PAGE_SIZE};

/// `/decision-search` est **rendue côté client** : route non indexée, la plus coûteuse
/// en SSR (elle rend CompactSearch + cartes + facettes côté serveur) et la seule
/// porteuse des mismatches d'hydratation (date-picker, nav). `ClientOnly` rend
/// un squelette statique au SSR et au 1er rendu, puis
/// monte le corps réactif après hydratation. Les routes SEO (décision, landing,
/// légal, mcp-guide) restent SSR.
#[component]
pub fn SearchPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let ui = &crate::i18n::use_ui().search;
    let query_map = use_query_map();
    // `leptos_meta` réévalue cette clôture après le démontage de la route, quand
    // `query_map` est déjà libéré : `try_get` au lieu de `get`.
    let title = move || {
        let q = query_map
            .try_get()
            .and_then(|params| params.get("q"))
            .unwrap_or_default();
        let q = q.trim();
        if q.is_empty() {
            ui.decisions_title.to_string()
        } else {
            format!("{q} - LibreJustice")
        }
    };
    view! {
        <Title text=title />
        <ClientOnly fallback=|| {
            view! { <SearchPageSkeleton /> }
        }>
            <SearchPageBody />
        </ClientOnly>
    }
}

/// Squelette statique du fallback SSR/1er rendu : même grille que le corps
/// (rail vide + colonne contenu), barre inerte + skeleton de résultats. Aucun
/// signal dépendant de `window`/`localStorage` → hydratation sans divergence.
#[component]
fn SearchPageSkeleton() -> impl IntoView {
    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().search.page_title}</h1>
            <div class=GRID_3COL>
                <div class="hidden lg:block"></div>
                <div class=CONTENT_COL>
                    <div class="h-12 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/40"></div>
                    <ResultSkeleton />
                </div>
            </div>
        </div>
    }
}

#[component]
fn SearchPageBody() -> impl IntoView {
    let query_map = use_query_map();
    // Texte de la barre partagé avec les filtres : cocher un filtre l'applique
    // comme `q` sans passer par « Rechercher ». Initialisé sur le `q` de l'URL.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));

    view! {
        <div class=SHELL>
            <h1 class="sr-only">{crate::i18n::use_ui().search.page_title}</h1>
            <DecisionsView />
        </div>
    }
}

/// Corps décisions : rail de synthèse à gauche (colonne alignée sur le
/// sommaire de la page décision), colonne contenu à droite — barre de
/// recherche, filtres horizontaux, chips, tri et résultats (gabarit du secteur).
/// Sans requête la page est la même, servie par le mode parcours : mêmes
/// filtres, mêmes facettes, même liste. Seul le pont croisé, qui porte une
/// question, cède la place à son pendant corpus.
#[component]
fn DecisionsView() -> impl IntoView {
    let query_map = use_query_map();
    let query = Signal::derive(move || {
        query_map
            .get()
            .get("q")
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    });
    let scope = crate::jurisdiction::use_reader_scope();
    let lang = crate::i18n::use_reader_lang().get();
    let key = Signal::derive(move || key_from_map(&query_map.get(), scope.code(), lang.code()));
    let resource = search_resource(key);
    // Facettes et volumétrie remontées de la réponse par `Effect` : ne jamais lire
    // la `Resource` dans le rendu du rail ou de la barre. Mises à jour sur réponse
    // Ok SEULEMENT — pendant un refetch, rail et barre gardent les valeurs
    // précédentes, interactives.
    let facets = RwSignal::new(None::<lj_dtos::SearchFacets>);
    let lang = crate::i18n::use_reader_lang().get();
    let volume = RwSignal::new(None::<Volume>);
    // Décompte et facettes arrivent par leur propre appel, sauf en parcours où la
    // réponse de liste les porte : deux sources, un seul jeu de signaux.
    let total = RwSignal::new(None::<i64>);
    // Clé de décompte à laquelle les facettes en main répondent : question et
    // filtres. Question changée, le rail passe en attente ; filtre changé, il
    // reste cliquable mais ses nombres comptent l'ensemble d'avant le clic.
    let counted = RwSignal::new(None::<SearchKey>);
    // Full key, not `count_key`: the resolved mode and the rerank verdict depend
    // on the sort and the AI mode, which `count_key` drops.
    let answered_key = RwSignal::new(None::<SearchKey>);
    Effect::new(move |_| {
        if let Some(Ok(resp)) = resource.get() {
            volume.set(Some(Volume {
                mode: resp.query_mode,
                reranked: resp.reranked,
            }));
            answered_key.set(Some(key.get_untracked()));
            if let Some(compte) = resp.total {
                total.set(Some(compte));
            }
            if let Some(f) = resp.facets {
                facets.set(Some(crate::i18n::relabel_facets(lang, f)));
                counted.set(Some(key.get_untracked().count_key()));
            }
        }
    });
    let compte = count_resource(key);
    Effect::new(move |_| {
        if let Some(Ok(compte)) = compte.get() {
            total.set(Some(compte.total));
            facets.set(Some(crate::i18n::relabel_facets(lang, compte.facets)));
            counted.set(Some(key.get_untracked().count_key()));
        }
    });
    // `Memo` : lu par le compteur de chaque ligne de facette, plusieurs centaines
    // sous une branche juridiction dépliée.
    let count_key = Signal::derive(move || key.get().count_key());
    let stale = Memo::new(move |_| is_stale(counted.into(), count_key).get());
    provide_context(StaleCounts(stale.into()));
    let shown_total = answered(total.into(), counted.into(), count_key);
    // Skeleton : clé en premier chargement (`is_loading` leptos-fetch) ET rien en
    // cache pour elle. La lecture SYNCHRONE du cache donne les deux comportements :
    // pas de flash au retour depuis `/decision` (cache présent → rendu direct), et
    // skeleton bien affiché quand une mutation de filtre retombe sur une clé vue
    // mais évincée du cache.
    let is_loading = search_is_loading(key);
    let show_skeleton =
        Signal::derive(move || is_loading.get() && cached_search(&key.get()).is_none());
    // Rail en attente : les facettes en main résument une AUTRE question. Un
    // changement de filtre, de tri ou de page garde la même, donc garde le rail
    // interactif ; la même lecture de cache que le skeleton évite un clignotement
    // au retour depuis une décision.
    let rail_loading = Signal::derive(move || {
        counted.with(|c| c.as_ref().map(|k| k.query.clone())) != Some(query.get())
            && cached_search(&key.get()).is_none()
    });
    let has_query = Signal::derive(move || !query.get().is_empty());
    let stats = corpus_stats();
    // `servi` asserts, so it goes silent in flight; the card view holds the
    // page's shape and keeps the last known mode, a toggle flipping on every
    // refetch being worse than a default one round-trip behind.
    let servi: Signal<Option<Volume>> = answered(volume.into(), answered_key.into(), key);
    let mode = Signal::derive(move || servi.get().map(|v| v.mode));
    let reranked = Signal::derive(move || servi.get().map(|v| v.reranked));
    let dernier_mode = Signal::derive(move || volume.get().map(|v| v.mode));
    let default_view = effective_card_view(query, dernier_mode);
    let (ai_mode, set_ai_mode) = crate::components::search::compact_search::ai_mode::use_ai_mode();

    view! {
        <div class=GRID_3COL>
            <aside aria-label=crate::i18n::use_ui().search.results_summary class=SIDE_RAIL>
                <SearchRail facets=facets loading=rail_loading />
            </aside>
            <div class=CONTENT_COL>
                <div class="flex flex-col gap-3">
                    <CompactSearch />
                    // Volumétrie mobile juste sous la barre, l'aide calée à droite.
                    // Sur desktop la volumétrie vit dans la colonne droite
                    // (`lg:hidden` ici).
                    <div class="flex items-baseline gap-3 lg:hidden">
                        <MobileSummary volume=volume total=shown_total loading=show_skeleton />
                        <SyntaxHint class="ml-auto shrink-0" />
                    </div>
                </div>
                <div class="flex min-w-0 flex-col gap-4">
                    // Tri en haut à droite de la barre de filtres dès `lg` (gabarit
                    // secteur `… · Plus de filtres + Trier par ▾`) ; sous `lg` il
                    // rejoint les commandes de la liste, et la barre prend la largeur.
                    <div class="flex items-start justify-between gap-4">
                        <div class="min-w-0 flex-1">
                            <DecisionsFilterBar facets=facets />
                        </div>
                        <div class="hidden shrink-0 lg:block">
                            <SortSelect />
                        </div>
                    </div>
                    <ActiveFilterChips facets=facets />
                    <div class="lg:hidden">
                        <ViewControls
                            query=query
                            mode=dernier_mode
                            served=mode
                            rerank=ai_mode
                            on_rerank=set_ai_mode
                            reranked=reranked
                        >
                            <ModeSelect resolved=mode modes=decision_modes(&crate::i18n::use_ui().search) />
                            <div class="ml-auto">
                                <SortSelect />
                            </div>
                        </ViewControls>
                    </div>
                    <SearchResults
                        query=query
                        resource=resource
                        key=key
                        show_skeleton=show_skeleton
                        default_view=default_view
                    />
                </div>
            </div>
            // Colonne droite : volumétrie, aide à la recherche, puis pont croisé
            // vers le moteur textes (la même requête posée à l'autre moteur). Sans
            // question à poser, le pont montre le fonds normatif. Sous lg la
            // colonne disparaît.
            <aside aria-label=crate::i18n::use_ui().search.see_also class=SIDE_RAIL>
                <div class="flex flex-col gap-4">
                    <SearchSummary
                        query=query
                        volume=volume
                        total=shown_total
                        loading=show_skeleton
                    />
                    <ModeSelect
                        resolved=mode
                        modes=decision_modes(&crate::i18n::use_ui().search)
                        class="w-full"
                    />
                    <ViewControls
                        query=query
                        mode=dernier_mode
                        served=mode
                        rerank=ai_mode
                        on_rerank=set_ai_mode
                        reranked=reranked
                        spread=true
                    />
                    <div class="flex self-start">
                        <SyntaxHint />
                    </div>
                    <Show
                        when=move || has_query.get()
                        fallback=move || view! { <TextesCorpusTeaser stats=stats /> }
                    >
                        <TextesTeaser query=query />
                    </Show>
                </div>
            </aside>
        </div>
    }
}

/// Desktop volumetry, heading the right column: the count, or the waiting state
/// on a cold cache. The question is in the bar and the mode is on the pills, so
/// only browse adds a line, having neither.
#[component]
fn SearchSummary(
    #[prop(into)] query: Signal<String>,
    #[prop(into)] volume: Signal<Option<Volume>>,
    #[prop(into)] total: Signal<Option<i64>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    move || {
        if loading.get() {
            return waiting_summary(query.get()).into_any();
        }
        if volume.get().is_none() {
            return ().into_any();
        }
        let compte = move || match total.get() {
            Some(total) => view! {
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {(numbers.results)(total)}
                </h2>
            }
            .into_any(),
            None => view! { <CountPlaceholder /> }.into_any(),
        };
        compte().into_any()
    }
}

/// Le décompte arrive après la liste : la ligne garde sa hauteur plutôt que de
/// pousser le rail quand le nombre atterrit.
#[component]
fn CountPlaceholder() -> impl IntoView {
    view! {
        <div
            class="h-7 w-32 animate-pulse rounded bg-[var(--color-rule)]/40"
            aria-label=crate::i18n::use_ui().search.counting
        ></div>
    }
}

/// `reranked` is the server's answer, not the caller's request: it gives up
/// silently on an exhausted budget or under three results, and nothing else on
/// the page knows.
#[derive(Clone, Copy, PartialEq)]
struct Volume {
    mode: QueryMode,
    reranked: bool,
}

fn waiting_summary(query: String) -> AnyView {
    let ui = &crate::i18n::use_ui().search;
    let label = if query.is_empty() {
        ui.loading_corpus
    } else {
        ui.searching
    };
    view! { <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">{label}</h2> }.into_any()
}

/// Volumétrie compacte, rendue dans la ligne mobile qui porte aussi l'aide à la
/// recherche (sur desktop la volumétrie vit dans la colonne droite).
#[component]
fn MobileSummary(
    #[prop(into)] volume: Signal<Option<Volume>>,
    #[prop(into)] total: Signal<Option<i64>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    move || {
        if loading.get() {
            return view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">{crate::i18n::use_ui().search.searching}</p>
            }
            .into_any();
        }
        if volume.get().is_none() {
            return ().into_any();
        }
        view! {
            <p class="text-sm text-[var(--color-ink-subtle)]">
                {move || {
                    total
                        .get()
                        .map(|total| {
                            view! {
                                <span class="font-medium text-[var(--color-ink)]">
                                    {(numbers.results)(total)}
                                </span>
                            }
                        })
                }}
            </p>
        }
        .into_any()
    }
}

/// Corps : skeleton (chargement à cache froid) / erreur / résultats, lus depuis
/// la `Resource`. Le skeleton est piloté par `show_skeleton` (calculé dans
/// `DecisionsView`) ; un `<Transition>` garde les résultats précédents pendant
/// qu'une clé déjà en cache résout.
#[component]
fn SearchResults(
    #[prop(into)] query: Signal<String>,
    resource: Resource<SearchResult>,
    #[prop(into)] key: Signal<SearchKey>,
    #[prop(into)] show_skeleton: Signal<bool>,
    #[prop(into)] default_view: Signal<CardView>,
) -> impl IntoView {
    // Anime la liste à chaque NOUVELLE recherche — clé jamais vue cette session,
    // c'est-à-dire changement de requête, filtre, tri, page ou mode IA. Set de clés
    // vues PARTAGÉ via contexte (fourni par `App`) : il survit au remontage de
    // `SearchPage` au retour depuis `/decision`, donc une recherche déjà vue ne
    // rejoue pas l'animation. Fallback local hors provider.
    let seen_keys = use_context::<SeenSearchKeys>()
        .map(|s| s.0)
        .unwrap_or_else(|| StoredValue::new(std::collections::HashSet::<SearchKey>::new()));

    view! {
        <section aria-label=crate::i18n::use_ui().search.search_results class="min-w-0">
            <Show
                when=move || show_skeleton.get()
                fallback=move || {
                    // Au retour depuis une décision la donnée est déjà en cache : on la
                    // lit de façon SYNCHRONE et on rend la liste dans la frame de
                    // montage. Cache froid (premier chargement) → <Transition> + Suspend
                    // (streaming SSR ; le skeleton est piloté en amont par le <Show>
                    // `show_skeleton`).
                    move || {
                        let current_key = key.get();
                        match cached_search(&current_key) {
                            Some(Ok(result)) => {
                                seen_keys.update_value(|seen| {
                                    seen.insert(current_key);
                                });
                                view! {
                                    <ResultsBody
                                        result=result
                                        query=query
                                        key=key
                                        animate=false
                                        default_view=default_view
                                    />
                                }
                                    .into_any()
                            }
                            Some(Err(failure)) => view! { <ResultError failure=failure /> }
                                .into_any(),
                            None => {
                                view! {
                                    <Transition fallback=|| ()>
                                        {move || Suspend::new(async move {
                                            let resolved = resource.await;
                                            // 1ʳᵉ fois qu'on voit cette clé cette session →
                                            // anime ; revisite (cache) → pas d'animation.
                                            let current_key = key.get_untracked();
                                            let animate = !seen_keys
                                                .with_value(|seen| seen.contains(&current_key));
                                            seen_keys.update_value(move |seen| {
                                                seen.insert(current_key);
                                            });
                                            match resolved {
                                                Err(failure) => {
                                                    view! { <ResultError failure=failure /> }
                                                        .into_any()
                                                }
                                                Ok(result) => {
                                                    view! {
                                                        <ResultsBody
                                                            result=result
                                                            query=query
                                                            key=key
                                                            animate=animate
                                                            default_view=default_view
                                                        />
                                                    }
                                                        .into_any()
                                                }
                                            }
                                        })}
                                    </Transition>
                                }
                                    .into_any()
                            }
                        }
                    }
                }
            >
                <ResultSkeleton />
            </Show>
        </section>
    }
}

/// Corps des résultats : vide / liste + pagination.
#[component]
fn ResultsBody(
    result: SearchResponse,
    #[prop(into)] query: Signal<String>,
    #[prop(into)] key: Signal<SearchKey>,
    animate: bool,
    #[prop(into)] default_view: Signal<CardView>,
) -> impl IntoView {
    let pinned = result.pinned.clone();
    let pinned_more = result.pinned_more;
    if result.hits.is_empty() {
        return view! {
            <div class="flex min-w-0 flex-col gap-6">
                <PinnedCard pinned=pinned more=pinned_more />
                <ResultEmpty query=query />
            </div>
        }
        .into_any();
    }
    // Restaure le scroll de la liste au retour depuis une décision : posé par le
    // bouton retour de la barre décision, consommé ICI au montage. One-shot.
    #[cfg(feature = "hydrate")]
    {
        let restore = crate::components::decision_bar::use_restore_scroll();
        Effect::new(move |_| {
            let Some(y) = restore.get_untracked() else {
                return;
            };
            restore.set(None);
            let scroll = move || {
                if let Some(w) = web_sys::window() {
                    w.scroll_to_with_x_and_y(0.0, y);
                }
            };
            // La liste se monte sous le <Suspend> dans la tâche de cet effet ; ses
            // nœuds ne sont attachés au DOM vivant qu'en fin de tâche. On applique le
            // scroll à trois échéances croissantes, toutes idempotentes, pour viser
            // le tout premier paint sans saut :
            //   1. synchrone — atteint la cible si le DOM est déjà à pleine hauteur ;
            //   2. micro-tâche (`spawn_local`) — s'exécute après l'attache des nœuds
            //      mais AVANT le paint (checkpoint micro-tâches < étape de rendu),
            //      donc la frame de montage peint déjà à la bonne position ;
            //   3. rAF — filet de sécurité si la liste se monte une frame plus tard.
            scroll();
            leptos::task::spawn_local(async move { scroll() });
            request_animation_frame(scroll);
        });
    }
    // La pagination est bornée par la liste servie, pas par le décompte, qui
    // arrive par son propre appel. En parcours la liste n'est qu'une fenêtre sur
    // le fonds, et le total la borne.
    let servis = result.all_hit_paths.len() as i64;
    let total = match result.query_mode {
        QueryMode::Browse => result.total.unwrap_or(servis),
        _ => servis,
    };
    let pages = total_pages(total) as u32;
    let hits = result.hits;
    let all_hit_paths = result.all_hit_paths;
    let SearchKey { page, query, .. } = key.get_untracked();
    let highlight = matches!(result.query_mode, QueryMode::Lexical | QueryMode::Hybrid)
        .then_some(query)
        .filter(|q| !q.trim().is_empty());
    view! {
        <div class="flex min-w-0 flex-col gap-6">
            <PinnedCard pinned=pinned more=pinned_more />
            <ResultList
                hits=hits
                all_hit_paths=all_hit_paths
                highlight=highlight
                page=page
                page_size=SEARCH_PAGE_SIZE
                default_view=default_view
                animate=animate
            />
            // `current_page`/`total_pages` STATIQUES (pas d'abonnement réactif) :
            // `ResultsBody` est reconstruit en entier à chaque nouvelle recherche, donc
            // la page courante est fixe pour une instance. Dériver de `key` (signal de
            // page) ferait ré-évaluer la pagination depuis un scope que le `<Suspense>`
            // dispose au changement de clé → panic « reactive value already disposed ».
            <Pagination
                current_page=Signal::derive(move || page)
                total_pages=Signal::derive(move || pages)
            />
        </div>
    }
    .into_any()
}

/// The top of the reference retrieval, served apart from the content list.
#[component]
fn PinnedCard(pinned: Vec<PinnedDecision>, more: bool) -> impl IntoView {
    if pinned.is_empty() {
        return ().into_any();
    }
    let ui = &crate::i18n::use_ui().search;
    let label = match pinned.len() {
        1 => ui.looking_for_this_one,
        _ => ui.looking_for_these,
    };
    view! {
        <section
            aria-label=label
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 px-4 py-3"
        >
            <p class="font-sans text-sm text-[var(--color-ink-subtle)]">{label}</p>
            <ul class="mt-2 flex flex-col gap-2">
                {pinned
                    .into_iter()
                    .map(|d| view! { <PinnedRow decision=d /> })
                    .collect::<Vec<_>>()}
            </ul>
            <Show when=move || more>
                <PinnedMore />
            </Show>
        </section>
    }
    .into_any()
}

#[component]
fn PinnedRow(decision: PinnedDecision) -> impl IntoView {
    use crate::components::ui::{Badge, BadgeTone};

    let seat = crate::i18n::use_ui()
        .referential
        .named_seat(decision.seat, decision.seat_parts.as_ref())
        .filter(|s| !s.trim().is_empty());
    let solution = decision
        .solution
        .filter(|t| t.key != "AUTRE")
        .map(|t| crate::i18n::ReferentialUi::label(crate::i18n::use_ui().referential.solution, &t));
    let on_navigate = |ev: leptos::ev::MouseEvent| {
        carry_seed(
            &ev,
            &ResultNavSeed {
                nav: None,
                from_search: Some(search_origin(None)),
            },
        );
    };
    view! {
        <li class="flex flex-col gap-0.5">
            <A
                href=lj_dtos::decision_path(decision.jurisdiction_type, decision.jurisdiction_code.as_deref(), &decision.id)
                on:click=on_navigate
                attr:class="font-sans text-base leading-snug text-[var(--color-ink)] no-underline transition-colors hover:text-[var(--color-accent)]"
            >
                {decision.title}
            </A>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                {seat
                    .map(|s| {
                        view! { <span class="text-xs text-[var(--color-ink-subtle)]">{s}</span> }
                    })}
                <div class="ml-auto flex flex-wrap justify-end gap-1.5">
                    {solution.map(|label| view! { <Badge tone=BadgeTone::Outline>{label}</Badge> })}
                </div>
            </div>
        </li>
    }
}

/// The card is a sample: `reference` on the same query is the whole list of the
/// decisions carrying the designation.
#[component]
fn PinnedMore() -> impl IntoView {
    use crate::components::search::compact_search::query_state;

    let query_map = leptos_router::hooks::use_query_map();
    let href = move || {
        let qs = query_state::with_param(&query_map.get(), "mode", Some("reference"), true);
        query_state::search_href(&qs)
    };
    view! {
        <A
            href=href
            attr:class="mt-3 inline-block text-xs text-[var(--color-ink-subtle)] underline decoration-dotted underline-offset-4 transition-colors hover:text-[var(--color-accent)]"
        >
            {crate::i18n::use_ui().search.other_decisions_match}
        </A>
    }
}
