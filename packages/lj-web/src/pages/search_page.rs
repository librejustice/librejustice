//! Page de recherche de décisions `/recherche` (gabarit de référence) :
//! CompactSearch + barre de filtres horizontale + résultats. La recherche de
//! textes vit sur sa propre page `/textes` (moteurs et filtres distincts,
//! aucune recherche transverse). État dans les query params ; données via
//! `leptos-fetch` (SWR) ; DTO `lj-dtos`.
//!
//! Anatomie alignée sur la page décision : même conteneur `max-w-[92rem]` et
//! même colonne gauche (240px, gouttière 48) que le sommaire d'une décision —
//! elle porte le rail de synthèse (`SearchRail`). La colonne de contenu
//! (barre, filtres, liste) prend le reste et se borne à `max-w-3xl` ; pas de
//! colonne droite fantôme qui compresserait le contenu pour rien.
//!
//! Hérite du title/description racine (le React n'a pas de `<Title>` propre).

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use lj_dtos::{QueryMode, SearchResponse};

use crate::components::client_only::ClientOnly;
use crate::components::search::compact_search::search_data::{
    cached_search, search_is_loading, search_resource, SeenSearchKeys,
};
use crate::components::search::compact_search::search_state::{
    key_from_map, SearchKey, SEARCH_LIMIT,
};
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::{
    ActiveFilterChips, CompactSearch, DecisionsFilterBar, Pagination, ResultEmpty, ResultError,
    ResultList, ResultSkeleton, SearchRail, SortSelect, SyntaxHint,
};
use crate::helpers::{format_results_count, total_pages};

/// `/recherche` est **rendue côté client** (ADR 0063) : route non indexée, et la
/// plus coûteuse en SSR (~20 ms p50 / 48 ms p95 vs ~2 ms pour la landing, car elle
/// rend CompactSearch + FilterRail + cartes + facettes côté serveur) **et** seule
/// porteuse des mismatches d'hydratation (toggle IA `localStorage`, date-picker,
/// nav). `ClientOnly` rend un squelette statique au SSR + 1er rendu, puis monte le
/// corps réactif après hydratation : ~0 rendu serveur du contenu lourd, classe de
/// mismatch supprimée. Les routes SEO (décision, landing, légal, mcp-guide) restent
/// SSR.
#[component]
pub fn SearchPage() -> impl IntoView {
    view! {
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
        <div class="mx-auto w-full max-w-[92rem] px-4 py-8 sm:px-6 lg:px-8">
            <h1 class="sr-only">"Recherche"</h1>
            <div class="grid grid-cols-1 gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                <div class="hidden lg:block"></div>
                <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
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
        <div class="mx-auto w-full max-w-[92rem] px-4 py-8 sm:px-6 lg:px-8">
            <h1 class="sr-only">"Recherche"</h1>
            <DecisionsView />
        </div>
    }
}

/// Corps décisions : rail de synthèse à gauche (colonne alignée sur le
/// sommaire de la page décision), colonne contenu à droite — barre de
/// recherche, filtres horizontaux, chips, tri et résultats (gabarit de référence).
/// Sans requête, ni filtres ni résultats — juste l'invite.
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
    let key = Signal::derive(move || key_from_map(&query_map.get()));
    let resource = search_resource(key);
    // Facettes et volumétrie remontées de la réponse par `Effect` (jamais lire
    // la `Resource` dans le rendu du rail/de la barre — mismatch hydratation +
    // panic « already disposed » documentés). Mises à jour sur réponse Ok
    // SEULEMENT : pendant un refetch, rail et barre gardent les valeurs
    // précédentes, interactives.
    let facets = RwSignal::new(None::<lj_dtos::SearchFacets>);
    let volume = RwSignal::new(None::<(i64, QueryMode)>);
    Effect::new(move |_| {
        if let Some(Ok(resp)) = resource.get().flatten() {
            volume.set(Some((resp.total, resp.query_mode)));
            facets.set(resp.facets);
        }
    });
    // Skeleton : clé en premier chargement (`is_loading` leptos-fetch) ET rien
    // en cache pour elle — la lecture SYNCHRONE du cache évite les deux écueils :
    // pas de flash au retour depuis `/decision` (cache présent → rendu direct),
    // skeleton bien affiché quand une mutation de filtre retombe sur une clé
    // vue mais évincée du cache (l'ancien garde « clé déjà vue » le masquait :
    // la zone semblait morte pendant la recherche).
    let is_loading = search_is_loading(key);
    let show_skeleton =
        Signal::derive(move || is_loading.get() && cached_search(&key.get()).is_none());

    view! {
        <div class="grid grid-cols-1 gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
            <aside
                aria-label="Synthèse des résultats"
                class="hidden min-w-0 lg:sticky lg:top-20 lg:block lg:max-h-[calc(100dvh-6rem)] lg:self-start lg:overflow-y-auto"
            >
                <SearchRail query=query volume=volume facets=facets loading=show_skeleton />
            </aside>
            <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
                <div class="flex flex-col gap-3">
                    <CompactSearch />
                    // Sous la barre sur mobile toujours, et sur desktop tant que la
                    // requête est vide (le rail — qui la porte alors — est masqué).
                    <div
                        class="flex self-start"
                        class=("lg:hidden", move || !query.get().is_empty())
                    >
                        <SyntaxHint />
                    </div>
                </div>
                <Show when=move || !query.get().is_empty() fallback=move || view! { <PromptEmpty /> }>
                    <div class="flex min-w-0 flex-col gap-4">
                        // Tri sur la ligne des filtres, calé à droite (gabarit
                        //  `… · Plus de filtres + Trier par ▾`) ; la barre
                        // wrappe dans sa colonne, le tri reste en haut à droite.
                        <div class="flex items-start justify-between gap-4">
                            <div class="min-w-0 flex-1">
                                <DecisionsFilterBar facets=facets />
                            </div>
                            <SortSelect />
                        </div>
                        <ActiveFilterChips facets=facets />
                        <MobileSummary volume=volume loading=show_skeleton />
                        <SearchResults query=query resource=resource key=key show_skeleton=show_skeleton />
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Volumétrie compacte de la rangée tri, mobile uniquement (sur desktop elle
/// vit dans le rail).
#[component]
fn MobileSummary(
    #[prop(into)] volume: Signal<Option<(i64, QueryMode)>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    move || {
        if loading.get() {
            return view! {
                <p class="text-sm text-[var(--color-ink-subtle)] lg:hidden">"Recherche en cours…"</p>
            }
            .into_any();
        }
        let Some((total, mode)) = volume.get() else {
            return ().into_any();
        };
        let mode_label = match mode {
            QueryMode::Lexical => "lexicale",
            QueryMode::Hybrid => "sémantique",
        };
        view! {
            <p class="text-sm text-[var(--color-ink-subtle)] lg:hidden">
                <span class="font-medium text-[var(--color-ink)]">{format_results_count(total)}</span>
                {format!(" · Recherche {mode_label}")}
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
    resource: Resource<Option<Result<SearchResponse, String>>>,
    #[prop(into)] key: Signal<crate::components::search::compact_search::search_state::SearchKey>,
    #[prop(into)] show_skeleton: Signal<bool>,
) -> impl IntoView {
    // Anime la liste à chaque NOUVELLE recherche (clé jamais vue cette session) ;
    // pas de rejeu au retour vers une recherche déjà en cache (rendu instantané →
    // rejouer l'effet mimerait un reload). Port de la logique `animate` du loader
    // Node : `true` sur cache-miss, `false` quand `getQueryData(key)` existe déjà.
    // Une nouvelle recherche = changement de requête, filtre, tri, page OU mode IA
    // → ses cartes rejouent l'animation « rise » (dont l'activation du mode IA).
    // Set de clés vues PARTAGÉ via contexte (fourni par `App`) : il survit au
    // remontage de `SearchPage` au retour depuis `/decision`, donc une recherche
    // déjà vue ne rejoue pas l'animation. Fallback local hors provider.
    let seen_keys = use_context::<SeenSearchKeys>()
        .map(|s| s.0)
        .unwrap_or_else(|| StoredValue::new(std::collections::HashSet::<SearchKey>::new()));

    view! {
        <section aria-label="Résultats de recherche" class="min-w-0">
            <Show
                when=move || show_skeleton.get()
                fallback=move || {
                    // Au retour depuis une décision la donnée est déjà en cache : on la
                    // lit de façon SYNCHRONE et on rend la liste dans la frame de
                    // montage. Le poll asynchrone du <Suspense> insérait sinon une frame
                    // où la zone résultats est vide (shell peint en haut de page) avant
                    // l'apparition de la liste — le « flash » résiduel. Cache froid
                    // (premier chargement) → <Transition> + Suspend (streaming SSR ;
                    // le skeleton est piloté en amont par le <Show> `show_skeleton`).
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
                                    />
                                }
                                    .into_any()
                            }
                            Some(Err(message)) => view! { <ResultError message=message /> }
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
                                                None => ().into_any(),
                                                Some(Err(message)) => {
                                                    view! { <ResultError message=message /> }
                                                        .into_any()
                                                }
                                                Some(Ok(result)) => {
                                                    view! {
                                                        <ResultsBody
                                                            result=result
                                                            query=query
                                                            key=key
                                                            animate=animate
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
    #[prop(into)] key: Signal<crate::components::search::compact_search::search_state::SearchKey>,
    animate: bool,
) -> impl IntoView {
    if result.hits.is_empty() {
        return view! { <ResultEmpty query=query /> }.into_any();
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
            // Sans ce relais, scroll_to clampait (page courte → peinte en haut puis
            // saut), l'ancien défaut.
            scroll();
            leptos::task::spawn_local(async move { scroll() });
            request_animation_frame(scroll);
        });
    }
    let total = result.total;
    let pages = total_pages(total, SEARCH_LIMIT as i64, MAX_PAGES_DISPLAY) as u32;
    let hits = result.hits;
    let all_hit_ids = result.all_hit_ids;
    let page = key.get_untracked().page;
    let ai_mode = key.get_untracked().ai_mode;

    view! {
        <div class="flex min-w-0 flex-col gap-6">
            <ResultList
                hits=hits
                all_hit_ids=all_hit_ids
                page=page
                total=total
                page_size=SEARCH_LIMIT
                auto_load_summary=ai_mode
                animate=animate
            />
            // `current_page`/`total_pages` STATIQUES (pas d'abonnement réactif) :
            // `ResultsBody` est reconstruit en entier à chaque nouvelle recherche, donc
            // la page courante est fixe pour une instance. Surtout, dériver de `key`
            // (signal de page) ferait ré-évaluer la pagination depuis un scope que le
            // `<Suspense>` dispose au changement de clé → panic « reactive value already
            // disposed » → runtime WASM mort (freeze). `page`/`pages` sont déjà calculés
            // ci-dessus.
            <Pagination
                current_page=Signal::derive(move || page)
                total_pages=Signal::derive(move || pages)
            />
        </div>
    }
    .into_any()
}

/// Plafond de pages affichées (parité `totalPages(total, 10, 40)` côté React :
/// `RESULTS_CAP=400` borne déjà à 40 pages, le cap URL `MAX_PAGES=10` borne la
/// navigation — on garde 40 ici pour la parité du calcul `total_pages`).
const MAX_PAGES_DISPLAY: i64 = 40;

/// Invite initiale (requête vide).
#[component]
fn PromptEmpty() -> impl IntoView {
    view! {
        <div class="flex flex-col items-start gap-3 border-t border-[var(--color-rule)] py-16">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Recherche"
            </p>
            <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                "Saisissez une requête pour commencer."
            </h2>
            <p class="max-w-prose text-[var(--color-ink-muted)]">
                "La recherche fonctionne en langage naturel ou avec des opérateurs booléens. La detection du mode est automatique."
            </p>
        </div>
    }
}
