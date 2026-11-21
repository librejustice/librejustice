//! Vue « Annonces » de la page `/annonces` — le pot des événements juridiques en
//! cours, **même gabarit que `/decision-search` et `/norm-search`** : rail de synthèse à
//! gauche, barre de dropdowns + chips actives au centre, colonne d'appoint à
//! droite.
//!
//! Deux différences tiennent au pot, pas au gabarit :
//!
//! - **Pas d'invite de départ.** Les sœurs n'affichent rien sans requête ; ici
//!   l'état vide *est* le produit — « ce qui est ouvert en ce moment », trié par
//!   échéance. `q` n'est qu'un filtre de plus.
//! - **La barre de filtres est déclarée par le serveur.** Les axes viennent de la
//!   réponse (`axes`), parce qu'ils dépendent du fond interrogé : le CPV
//!   n'existe que pour les marchés, la famille d'avis n'a pas le même
//!   vocabulaire d'un fond à l'autre. Le front n'en connaît aucun par son nom,
//!   il rend ce que le backend sait exécuter.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_query_map;
use leptos_router::params::ParamsMap;
use lj_dtos::{
    FacetChoice, JournalFacetAxis, JournalFondGroup, JournalHit, JournalSearchRequest,
    JournalSearchResponse, JournalSort,
};

use crate::api::ApiClient;
use crate::components::layout::{CONTENT_COL, GRID_3COL, SIDE_RAIL};
use crate::helpers::{group_thousands, total_pages, SEARCH_PAGE_SIZE};
use crate::pages::decision_page::data::sendable;

use super::compact_search::query_state;
use super::facet_widgets::{stale_counts, FilterSearchInput, Nav, StaleCounts};
use super::filter_dropdown::{FilterDropdown, OpenDropdown};
use super::CompactSearch;

/// Rows per facet block of the rail.
const RAIL_ROWS: usize = 6;
/// Au-delà de ce nombre de valeurs, un axe ne se lit plus par le poids : sa
/// liste passe en ordre alphabétique et gagne un champ de filtre (départements,
/// divisions CPV).
const LISTE_LONGUE: usize = 12;

/// Clés d'URL portées par les filtres. `fond` en fait partie : c'est un axe
/// comme les autres, pas un mode.
const FILTER_KEYS: [&str; 7] = [
    "fond",
    "zone",
    "famille",
    "cpv",
    "metier",
    "echeanceJours",
    "closes",
];

/// Axes multi-valeurs : cocher une seconde valeur l'ajoute au lieu de remplacer
/// la première (un candidat suit deux départements).
const MULTI_KEYS: [&str; 5] = ["fond", "zone", "famille", "cpv", "metier"];

/// État de recherche lu depuis l'URL — la même mécanique que les pages sœurs :
/// l'URL est l'état, rien ne vit en mémoire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalsQuery {
    pub q: String,
    pub fond: Vec<String>,
    pub zone: Vec<String>,
    pub famille: Vec<String>,
    pub cpv: Vec<String>,
    pub metier: Vec<String>,
    pub echeance_jours: Option<u32>,
    pub closes: bool,
    pub sort: Option<String>,
    pub page: u32,
    /// The reader's jurisdiction, from the address rather than its query.
    pub home: Option<String>,
}

impl JournalsQuery {
    pub fn from_map(map: &ParamsMap) -> Self {
        let liste = |key: &str| -> Vec<String> {
            map.get_all(key)
                .map(|v| {
                    v.iter()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            q: map
                .get("q")
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            fond: liste("fond"),
            zone: liste("zone"),
            famille: liste("famille"),
            cpv: liste("cpv"),
            metier: liste("metier"),
            echeance_jours: map.get("echeanceJours").and_then(|j| j.parse().ok()),
            closes: map.get("closes").is_some_and(|v| v == "true" || v == "1"),
            sort: map.get("sort").filter(|s| !s.is_empty()),
            page: map
                .get("page")
                .and_then(|p| p.parse::<u32>().ok())
                .filter(|p| *p >= 1)
                .unwrap_or(1),
            home: None,
        }
    }

    /// Ce dont dépendent le total et les axes : la question et ses filtres, la
    /// page et le tri parcourant le même ensemble.
    fn count_key(&self) -> Self {
        Self {
            page: 1,
            sort: None,
            ..self.clone()
        }
    }

    /// Valeur active d'un axe, quelle que soit sa clé — le front ne connaît pas
    /// les axes par leur nom, il les lit comme le serveur les déclare.
    fn valeurs(&self, key: &str) -> Vec<String> {
        match key {
            "fond" => self.fond.clone(),
            "zone" => self.zone.clone(),
            "famille" => self.famille.clone(),
            "cpv" => self.cpv.clone(),
            "metier" => self.metier.clone(),
            "echeanceJours" => self
                .echeance_jours
                .map(|j| j.to_string())
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn to_request(&self) -> JournalSearchRequest {
        JournalSearchRequest {
            q: (!self.q.is_empty()).then(|| self.q.clone()),
            fond: self.fond.clone(),
            zone: self.zone.clone(),
            famille: self.famille.clone(),
            cpv: self.cpv.clone(),
            metier: self.metier.clone(),
            echeance_jours: self.echeance_jours,
            closes: self.closes,
            jurisdiction: None,
            home: self.home.clone(),
            sort: match self.sort.as_deref() {
                Some("publication") => JournalSort::Publication,
                Some("nouveautes") => JournalSort::Nouveautes,
                _ => JournalSort::Echeance,
            },
            limit: SEARCH_PAGE_SIZE,
            offset: self.page.max(1).saturating_sub(1) * SEARCH_PAGE_SIZE,
        }
    }
}

async fn fetch(state: JournalsQuery) -> Result<JournalSearchResponse, crate::api::Failure> {
    ApiClient::from_context()
        .search_journals(&state.to_request())
        .await
        .map_err(|e| e.failure)
}

#[component]
pub fn JournalsView() -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let ui = crate::i18n::use_ui();
    let query_map = use_query_map();
    let scope = crate::jurisdiction::use_reader_scope();
    let state = Signal::derive(move || JournalsQuery {
        home: Some(scope.code()),
        ..JournalsQuery::from_map(&query_map.get())
    });
    let results = Resource::new(move || state.get(), |s| sendable(fetch(s)));
    // Axes et volumétrie survivent au refetch : la barre reste cliquable pendant
    // qu'une page charge.
    let axes = RwSignal::new(Vec::<JournalFacetAxis>::new());
    let volume = RwSignal::new(None::<i64>);
    // État que comptent les nombres en main : filtre changé, ils comptent celui
    // d'avant le clic, donc ils se taisent le temps de la réponse.
    let counted = RwSignal::new(None::<JournalsQuery>);
    Effect::new(move |_| {
        if let Some(Ok(resp)) = results.get() {
            axes.set(resp.axes);
            volume.set(Some(resp.total));
            counted.set(Some(state.get_untracked().count_key()));
        }
    });
    let stale = Memo::new(move |_| counted.get() != Some(state.get().count_key()));
    provide_context(StaleCounts(stale.into()));
    let shown_volume = Signal::derive(move || volume.get().filter(|_| !stale.get()));

    view! {
        <div class=GRID_3COL>
            <aside aria-label=ui.search.results_summary class=SIDE_RAIL>
                <JournalsRail axes=axes state=state />
            </aside>
            <div class=CONTENT_COL>
                <div class="flex flex-col gap-2">
                    <CompactSearch />
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
                    </div>
                </div>
                <div class="flex min-w-0 flex-col gap-4">
                    <JournalsFilterBar axes=axes state=state />
                    <JournalsActiveChips axes=axes state=state />
                    <Suspense fallback=move || {
                        view! { <p class="text-sm text-[var(--color-ink-subtle)]">{ui.search.pending}</p> }
                    }>
                        {move || Suspend::new(async move {
                            let st = state.get_untracked();
                            match results.await {
                                Ok(resp) => view! { <JournalsResults resp=resp state=st /> }.into_any(),
                                Err(failure) => {
                                    let ui = crate::i18n::use_ui();
                                    view! {
                                        <p class="text-sm text-[var(--color-ink-subtle)]">
                                            {(ui.notices.unavailable)(&failure.text(&ui.shell))}
                                        </p>
                                    }
                                        .into_any()
                                }
                            }
                        })}
                    </Suspense>
                </div>
            </div>
            <aside aria-label=ui.search.see_also class=SIDE_RAIL>
                <JournalsSummary volume=shown_volume state=state />
            </aside>
        </div>
    }
}

/// Volumétrie + tri + flux, en tête de la colonne droite.
#[component]
fn JournalsSummary(
    #[prop(into)] volume: Signal<Option<i64>>,
    #[prop(into)] state: Signal<JournalsQuery>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let query_map = use_query_map();
    let nav = Nav::new();
    let tri_actif = move || state.get().sort.unwrap_or_else(|| "echeance".to_string());
    let ui = crate::i18n::use_ui();
    let tris = [
        ("echeance", ui.notices.sort_deadline),
        ("nouveautes", ui.notices.sort_seen),
        ("publication", ui.notices.sort_published),
    ];
    view! {
        <div class="flex flex-col gap-4">
            <div class="flex flex-col gap-1">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {move || {
                        volume
                            .get()
                            .map(numbers.results)
                            .unwrap_or_else(|| ui.search.searching.to_string())
                    }}
                </h2>
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {ui.notices.open_notices}
                </p>
            </div>
            <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
                <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.notices.sort_by}
                </p>
                {tris
                    .into_iter()
                    .map(|(value, label)| {
                        let actif = move || tri_actif() == value;
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    if actif() {
                                        "text-left text-[13px] font-medium text-[var(--color-bordeaux)]"
                                    } else {
                                        "text-left text-[13px] text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
                                    }
                                }
                                on:click=move |_| {
                                    nav.go(
                                        query_state::with_param(
                                            &query_map.get_untracked(),
                                            "sort",
                                            (value != "echeance").then_some(value),
                                            true,
                                        ),
                                    );
                                }
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <JournalsFlux state=state />
        </div>
    }
}

/// Les deux flux d'alerte de l'état de filtres courant : l'URL de la page, avec
/// une autre extension.
#[component]
fn JournalsFlux(#[prop(into)] state: Signal<JournalsQuery>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().notices;
    let query_map = use_query_map();
    let href = move |ext: &str| {
        let home = state.get().home.unwrap_or_default();
        let qs = query_state::without_keys(&query_map.get(), &["page", "sort"]);
        if qs.is_empty() {
            format!("/journal-search.{ext}?home={home}")
        } else {
            format!("/journal-search.{ext}?home={home}&{qs}")
        }
    };
    // L'agenda n'est offert que si un fond sélectionné porte des échéances : un
    // fond qui constate des faits déjà survenus produirait un calendrier vide.
    let agenda = move || {
        let fonds = state.get().fond;
        fonds.is_empty() || fonds.iter().any(|f| lj_dtos::journal_fond_a_echeance(f))
    };
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {ui.subscribe}
            </p>
            <a
                href=move || href("rss")
                class="text-[13px] text-[var(--color-ink-muted)] hover:text-[var(--color-accent)]"
            >
                {ui.rss}
            </a>
            <Show when=agenda>
                <a
                    href=move || href("ics")
                    class="text-[13px] text-[var(--color-ink-muted)] hover:text-[var(--color-accent)]"
                >
                    {ui.ics}
                </a>
            </Show>
            <p class="pt-1 text-[11px] leading-snug text-[var(--color-ink-subtle)]">
                {ui.no_account}
            </p>
        </div>
    }
}

/// Rail gauche : un bloc par axe déclaré, mêmes lignes `libellé … compte` que
/// le rail des pages sœurs.
#[component]
fn JournalsRail(
    #[prop(into)] axes: Signal<Vec<JournalFacetAxis>>,
    #[prop(into)] state: Signal<JournalsQuery>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-4 [&>div:first-child]:border-t-0 [&>div:first-child]:pt-0">
            <For
                each=move || axes.get()
                key=|axe: &JournalFacetAxis| axe.key.clone()
                children=move |axe: JournalFacetAxis| {
                    // Les clés d'axe ne bougent pas d'une réponse à l'autre,
                    // leurs comptes si : le bloc se relit dans le signal à
                    // chaque réponse.
                    let key = StoredValue::new(axe.key);
                    move || {
                        let (axe, actives) = key
                            .with_value(|k| {
                                (
                                    axes.get().into_iter().find(|a| &a.key == k),
                                    state.get().valeurs(k),
                                )
                            });
                        axe.map(|axe| rail_block(axe, &actives))
                    }
                }
            />
        </div>
    }
}

/// The rail shows the level below a root, keeping a root that has none. The
/// server interleaves each root with its leaves, hence the re-sort by weight.
fn rail_choices(mut choices: Vec<FacetChoice>) -> Vec<FacetChoice> {
    let parents: Vec<String> = choices.iter().filter_map(|c| c.parent.clone()).collect();
    if parents.is_empty() {
        return choices;
    }
    choices.retain(|c| !parents.contains(&c.value));
    choices.sort_by_key(|c| std::cmp::Reverse(c.count));
    choices
}

fn rail_block(axe: JournalFacetAxis, actives: &[String]) -> AnyView {
    let numbers = &crate::i18n::use_ui().numbers;
    if axe.choices.is_empty() {
        return ().into_any();
    }
    let query_map = use_query_map();
    let nav = Nav::new();
    let stale = stale_counts();
    let key = axe.key.clone();
    let rows = rail_choices(axe.choices)
        .into_iter()
        .take(RAIL_ROWS)
        .map(|c| {
            let is_active = actives.iter().any(|a| a == &c.value);
            let count = c.count;
            let label_class = if is_active {
                "truncate text-[13px] leading-snug font-medium text-[var(--color-bordeaux)]"
            } else {
                "truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors group-hover:text-[var(--color-ink)]"
            };
            let key = key.clone();
            let value = c.value.clone();
            view! {
                <button
                    type="button"
                    class="group flex w-full items-baseline justify-between gap-3 text-left"
                    title=c.label.clone()
                    on:click=move |_| {
                        nav.go(basculer(&query_map.get_untracked(), &key, &value, is_active));
                    }
                >
                    <span class=label_class>{c.label.clone()}</span>
                    <Show when=move || !stale.get()>
                        <span class="shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]">
                            {group_thousands(numbers, count)}
                        </span>
                    </Show>
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {axe.label.clone()}
            </p>
            {rows}
        </div>
    }
    .into_any()
}

/// Mutation d'URL d'un axe : multi-valeurs pour ceux qui l'acceptent, sinon
/// mono-sélection qui se retire au re-clic. Toute mutation revient page 1.
fn basculer(map: &ParamsMap, key: &str, value: &str, is_active: bool) -> String {
    if MULTI_KEYS.contains(&key) {
        return query_state::toggle_multi(map, key, value);
    }
    query_state::with_param(map, key, (!is_active).then_some(value), true)
}

/// Barre de filtres : un dropdown par axe déclaré, dans l'ordre du serveur.
#[component]
fn JournalsFilterBar(
    #[prop(into)] axes: Signal<Vec<JournalFacetAxis>>,
    #[prop(into)] state: Signal<JournalsQuery>,
) -> impl IntoView {
    provide_context(OpenDropdown(RwSignal::new(None)));
    view! {
        <div
            role="toolbar"
            aria-label=crate::i18n::use_ui().search.filters
            class="flex items-center gap-2 overflow-x-auto pb-1"
        >
            <For
                each=move || axes.get()
                key=|axe: &JournalFacetAxis| axe.key.clone()
                children=move |axe: JournalFacetAxis| {
                    let key = StoredValue::new(axe.key.clone());
                    // Les choix se relisent dans le signal, jamais figés à la
                    // première réponse : cocher un filtre remonte des comptes
                    // neufs sur tous les autres axes.
                    let choices = Signal::derive(move || {
                        key.with_value(|k| {
                            axes.get()
                                .into_iter()
                                .find(|a| &a.key == k)
                                .map(|a| a.choices)
                                .unwrap_or_default()
                        })
                    });
                    view! {
                        <FilterDropdown
                            id=axe.key.clone()
                            label=axe.label.clone()
                            active_count=Signal::derive(move || {
                                key.with_value(|k| state.get().valeurs(k).len())
                            })
                        >
                            <FacetList
                                url_key=key.get_value()
                                choices=choices
                                actives=Signal::derive(move || {
                                    key.with_value(|k| state.get().valeurs(k))
                                })
                            />
                        </FilterDropdown>
                    }
                }
            />
        </div>
    }
}

/// Liste d'un dropdown : lignes valeur/compte, la valeur active se retire au
/// re-clic. Une sélection orpheline (URL partagée, valeur absente de la
/// facette) reste affichée en tête, compteur 0 — sinon on ne peut plus la
/// retirer.
///
/// Au-delà de [`LISTE_LONGUE`] valeurs, l'axe se parcourt par le nom et non
/// plus par le poids : les 100 départements se lisent dans l'ordre
/// alphabétique, avec un champ pour filtrer. Le rail garde l'ordre du serveur
/// (les six plus gros) — c'est lui qui dit où est la masse.
#[component]
fn FacetList(
    url_key: String,
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] actives: Signal<Vec<String>>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let query_map = use_query_map();
    let nav = Nav::new();
    let stale = stale_counts();
    let saisie = RwSignal::new(String::new());
    let rows = Memo::new(move |_| {
        let choices = choices.get();
        let mut out: Vec<FacetChoice> = actives
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
        // A tree keeps the order the server posed, a root then its own leaves;
        // sorting it by name would scatter the leaves away from their root.
        let arbre = choices.iter().any(|c| c.parent.is_some());
        out.extend(choices);
        if !arbre && out.len() > LISTE_LONGUE {
            out.sort_by_key(|c| cle_alpha(&c.label));
        }
        out
    });
    let cherchable = Signal::derive(move || rows.with(|r| r.len()) > LISTE_LONGUE);
    let ui = &crate::i18n::use_ui().notices;
    let placeholder = match url_key.as_str() {
        "zone" => ui.filter_zone,
        "cpv" => ui.filter_cpv,
        "metier" => ui.filter_trade,
        _ => ui.filter,
    };
    // Les lignes montrées ; le compteur et le libellé de chacune se relisent
    // dans `rows`, qui suit les facettes de la dernière réponse.
    let visible = Memo::new(move |_| {
        let aiguille = cle_alpha(&saisie.get());
        let toutes = rows.get();
        let repond =
            |c: &FacetChoice| aiguille.is_empty() || cle_alpha(&c.label).contains(&aiguille);
        let racines: Vec<String> = toutes
            .iter()
            .filter(|c| c.parent.is_none() && repond(c))
            .map(|c| c.value.clone())
            .collect();
        toutes
            .into_iter()
            .filter(|c| {
                repond(c)
                    || c.parent
                        .as_deref()
                        .is_some_and(|p| racines.iter().any(|r| r == p))
            })
            .map(|c| (c.parent.is_some(), c))
            .collect::<Vec<_>>()
    });

    view! {
        <Show when=move || cherchable.get()>
            <FilterSearchInput value=saisie placeholder=placeholder />
        </Show>
        <div
            class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto overscroll-contain"
        >
            <Show when=move || visible.with(|r| r.is_empty())>
                <p class="px-2 py-1 text-xs text-[var(--color-ink-subtle)]">{crate::i18n::use_ui().search.no_value}</p>
            </Show>
            <For
                each=move || visible.get()
                key=|(_, c): &(bool, FacetChoice)| c.value.clone()
                children=move |(feuille, c): (bool, FacetChoice)| {
                    let key = url_key.clone();
                    let value = c.value.clone();
                    let pour_actif = c.value.clone();
                    let is_active = Signal::derive(move || {
                        actives.get().iter().any(|a| a == &pour_actif)
                    });
                    let lookup = c.value.clone();
                    let row = Memo::new(move |_| {
                        rows.with(|l| l.iter().find(|x| x.value == lookup).cloned())
                    });
                    let repli = c.value.clone();
                    let label = Signal::derive(move || {
                        row.get().map(|x| x.label).unwrap_or_else(|| repli.clone())
                    });
                    let count = Signal::derive(move || row.get().map(|x| x.count).unwrap_or(0));
                    let row_class = move || {
                        crate::helpers::cn([
                            "flex w-full items-center gap-2.5 rounded py-1 text-left text-sm transition-colors",
                            if feuille { "pl-5 pr-2" } else { "px-2 font-medium" },
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
                            aria-pressed=move || is_active.get().to_string()
                            class=row_class
                            on:click=move |_| {
                                nav.go(
                                    basculer(
                                        &query_map.get_untracked(),
                                        &key,
                                        &value,
                                        is_active.get_untracked(),
                                    ),
                                );
                            }
                        >
                            <span class="flex-1 truncate leading-tight">{label}</span>
                            <Show when=move || { count.get() > 0 && !stale.get() }>
                                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                                    {move || group_thousands(numbers, count.get())}
                                </span>
                            </Show>
                        </button>
                    }
                }
            />
        </div>
    }
}

/// Clé de tri et de recherche d'un libellé : minuscules, diacritiques rabattus.
/// « Ardèche » se range alors entre « Ardennes » et « Ariège », et se trouve en
/// tapant « ardeche ».
fn cle_alpha(label: &str) -> String {
    lj_fold::fold_chars(label)
}

/// Chips des filtres actifs + « Tout effacer ». Le libellé se résout dans
/// l'axe ; à défaut, la valeur brute.
#[component]
fn JournalsActiveChips(
    #[prop(into)] axes: Signal<Vec<JournalFacetAxis>>,
    #[prop(into)] state: Signal<JournalsQuery>,
) -> impl IntoView {
    let ui = crate::i18n::use_ui();
    let query_map = use_query_map();
    let nav = Nav::new();
    let chips = Memo::new(move |_| {
        let state = state.get();
        let axes = axes.get();
        let mut out: Vec<(String, String, String)> = Vec::new();
        for key in FILTER_KEYS {
            if key == "closes" {
                if state.closes {
                    out.push((
                        "closes".to_string(),
                        "true".to_string(),
                        ui.notices.closed_included.to_string(),
                    ));
                }
                continue;
            }
            let axe = axes.iter().find(|a| a.key == key);
            for value in state.valeurs(key) {
                let label = axe
                    .and_then(|a| a.choices.iter().find(|c| c.value == value))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| value.clone());
                out.push((key.to_string(), value, label));
            }
        }
        out
    });

    view! {
        <Show when=move || !chips.with(|c| c.is_empty())>
            <div class="flex flex-wrap items-center gap-2">
                <For
                    each=move || chips.get()
                    key=|(key, value, label): &(String, String, String)| {
                        format!("{key}:{value}:{label}")
                    }
                    children=move |(key, value, label): (String, String, String)| {
                        let on_remove = move |_| {
                            let map = query_map.get_untracked();
                            nav.go(if MULTI_KEYS.contains(&key.as_str()) {
                                query_state::toggle_multi(&map, &key, &value)
                            } else {
                                query_state::with_param(&map, &key, None, true)
                            });
                        };
                        view! {
                            <span class="inline-flex h-7 items-center gap-1.5 rounded-full border border-[var(--color-rule)] bg-[var(--color-bordeaux-soft)] pl-3 pr-1.5 text-xs text-[var(--color-accent)]">
                                <span class="max-w-56 truncate">{label}</span>
                                <button
                                    type="button"
                                    aria-label=ui.search.remove_filter
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
                        nav.go(query_state::without_keys(&query_map.get_untracked(), &FILTER_KEYS));
                    }
                    class="text-xs text-[var(--color-ink-subtle)] underline-offset-2 hover:text-[var(--color-accent)] hover:underline"
                >
                    {ui.search.clear_all}
                </button>
            </div>
        </Show>
    }
}

/// Les groupes de la réponse. Un seul fond interrogé ⇒ une liste simple ; les
/// deux ⇒ un intertitre par fond, jamais d'entrelacement.
#[component]
fn JournalsResults(resp: JournalSearchResponse, state: JournalsQuery) -> impl IntoView {
    if resp.groupes.iter().all(|g| g.hits.is_empty()) {
        return view! {
            <div class="border-t border-[var(--color-rule)] py-16">
                <p class="text-[var(--color-ink-muted)]">
                    {crate::i18n::use_ui().notices.none_open}
                </p>
            </div>
        }
        .into_any();
    }
    let seul = resp.groupes.len() == 1;
    let pages = if seul {
        total_pages(resp.groupes[0].total) as u32
    } else {
        0
    };
    let groupes = resp
        .groupes
        .into_iter()
        .map(|g| view! { <Groupe groupe=g seul=seul /> })
        .collect_view();
    view! {
        <section aria-label=crate::i18n::use_ui().search.results class="flex min-w-0 flex-col gap-6">
            {groupes}
            // La pagination n'a de sens que sur un fond : deux backends ont
            // deux curseurs, et l'utilisateur choisit son fond avant de
            // s'enfoncer dedans.
            <Show when=move || { seul && pages > 1 }>
                <JournalsPagination current_page=state.page total_pages=pages />
            </Show>
        </section>
    }
    .into_any()
}

#[component]
fn Groupe(groupe: JournalFondGroup, seul: bool) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    let fond = groupe.fond.clone();
    let entete = (!seul).then(|| {
        let label = groupe.label.clone();
        let total = groupe.total;
        view! {
            <div class="flex items-baseline justify-between gap-3 border-t border-[var(--color-rule)] pt-3">
                <h2 class="font-sans text-lg text-[var(--color-ink)]">{label}</h2>
                <button
                    type="button"
                    class="shrink-0 text-xs text-[var(--color-ink-subtle)] underline-offset-2 hover:text-[var(--color-accent)] hover:underline"
                    on:click=move |_| {
                        nav.go(query_state::toggle_multi(&query_map.get_untracked(), "fond", &fond));
                    }
                >
                    {(crate::i18n::use_ui().notices.see_all)(total)}
                </button>
            </div>
        }
    });
    let lignes = groupe
        .hits
        .into_iter()
        .map(|h| view! { <Ligne hit=h /> })
        .collect_view();
    view! {
        <div class="flex flex-col gap-1">
            {entete}
            <ul class="flex flex-col">{lignes}</ul>
        </div>
    }
}

/// Une annonce en liste : ce qu'elle est, qui elle concerne, quand elle ferme.
#[component]
fn Ligne(hit: JournalHit) -> impl IntoView {
    let href = lj_vocab::notice_path(&hit.jurisdiction, &hit.uid);
    let titre = hit.titre.clone();
    let acteur = Some(hit.acteur.trim().to_string()).filter(|a| !a.is_empty());
    let description = description(&hit);
    let meta = ligne_meta(&hit, &crate::i18n::use_ui().notices);
    let lien = view! {
        <A href=href attr:class="text-base text-[var(--color-ink)] hover:text-[var(--color-accent)]">
            {titre}
        </A>
    };
    view! {
        <li class="flex flex-col gap-1 border-t border-[var(--color-rule)] py-3">
            {lien} {acteur.map(|a| view! { <p class="text-sm text-[var(--color-ink-muted)]">{a}</p> })}
            {description
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

/// Collapsed to one line: a scraped notice keeps the newlines and tabs of the
/// HTML it came from, which a clamp would spend on whitespace.
pub fn description(hit: &JournalHit) -> Option<String> {
    let serre = hit
        .description
        .as_deref()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!serre.is_empty()).then_some(serre)
}

/// Ligne de métadonnées : échéance ou parution, lieu, nature, canaux.
fn ligne_meta(hit: &JournalHit, ui: &crate::i18n::NoticesUi) -> String {
    let dates = &crate::i18n::use_ui().dates;
    let mut bouts: Vec<String> = Vec::new();
    match hit.echeance.as_deref().and_then(|d| numeric_date(dates, d)) {
        Some(d) => bouts.push((ui.due_before)(&d)),
        None => {
            if let Some(d) = numeric_date(dates, &hit.date_publication) {
                bouts.push((ui.published_on)(&d));
            }
        }
    }
    if let Some(portee) = crate::helpers::portee_zones(ui, &hit.zones) {
        bouts.push(portee);
    }
    if let Some(f) = hit.famille.as_ref() {
        bouts.push(f.label.clone());
    }
    if let Some(journal) = hit.journal.as_ref() {
        bouts.push(journal.clone());
    }
    if hit.canaux.len() > 1 {
        bouts.push((ui.channels)(hit.canaux.len()));
    }
    bouts.join(" · ")
}

/// `2026-08-12` or RFC 3339, in figures; `None` for any other shape.
pub fn numeric_date(ui: &crate::i18n::DatesUi, value: &str) -> Option<String> {
    let date = value.get(..10)?;
    let mut parts = date.split('-');
    let (a, m, j) = (parts.next()?, parts.next()?, parts.next()?);
    (a.len() == 4 && m.len() == 2 && j.len() == 2).then(|| (ui.numeric_date)(a, m, j))
}

#[component]
fn JournalsPagination(current_page: u32, total_pages: u32) -> impl IntoView {
    let ui = crate::i18n::use_ui();
    let query_map = use_query_map();
    let nav = Nav::new();
    let lien = move |page: u32, label: &'static str, actif: bool| {
        let cible = query_state::with_param(
            &query_map.get_untracked(),
            "page",
            (page > 1).then(|| page.to_string()).as_deref(),
            false,
        );
        view! {
            <button
                type="button"
                disabled=!actif
                class="rounded border border-[var(--color-rule)] px-3 py-1.5 text-sm text-[var(--color-ink-muted)] disabled:opacity-40 enabled:hover:text-[var(--color-accent)]"
                on:click=move |_| nav.go(cible.clone())
            >
                {label}
            </button>
        }
    };
    view! {
        <nav aria-label=ui.search.pagination class="flex items-center justify-between gap-3">
            {lien(current_page.saturating_sub(1).max(1), ui.notices.previous, current_page > 1)}
            <span class="text-xs text-[var(--color-ink-subtle)]">
                {(ui.notices.page_of)(current_page, total_pages)}
            </span>
            {lien(current_page + 1, ui.notices.next, current_page < total_pages)}
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_drops_a_subdivided_root_and_keeps_one_that_is_not() {
        let choix = |value: &str, parent: Option<&str>, count: i64| FacetChoice {
            value: value.to_string(),
            label: value.to_string(),
            count,
            parent: parent.map(str::to_string),
            hint: None,
        };
        let rows = rail_choices(vec![
            choix("nature_marche:TRAVAUX", None, 5000),
            choix("45", Some("nature_marche:TRAVAUX"), 4000),
            choix("nature_marche:AUTRES", None, 50),
            choix("nature_marche:SERVICES", None, 9000),
            choix("71", Some("nature_marche:SERVICES"), 2000),
        ]);
        let vues: Vec<&str> = rows.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(vues, ["45", "71", "nature_marche:AUTRES"]);
    }

    #[test]
    fn la_date_iso_se_lit_en_francais() {
        let ui = &crate::i18n::Lang::Fr.ui().dates;
        assert_eq!(
            numeric_date(ui, "2026-08-12").as_deref(),
            Some("12/08/2026")
        );
        assert_eq!(
            numeric_date(ui, "2026-09-01T15:00:00+00:00").as_deref(),
            Some("01/09/2026")
        );
        assert_eq!(numeric_date(ui, "hier"), None);
    }

    #[test]
    fn le_fond_est_un_axe_multi_valeurs() {
        assert!(MULTI_KEYS.contains(&"fond"));
        assert!(!MULTI_KEYS.contains(&"echeanceJours"));
        assert!(FILTER_KEYS.contains(&"fond"));
    }

    /// Page et tri parcourent le même ensemble : leurs nombres ne se taisent pas.
    #[test]
    fn count_key_ignore_page_et_tri() {
        let base = JournalsQuery::default();
        let ailleurs = JournalsQuery {
            page: 4,
            sort: Some("publication".to_string()),
            ..base.clone()
        };
        assert_eq!(base.count_key(), ailleurs.count_key());

        let filtre = JournalsQuery {
            zone: vec!["75".to_string()],
            ..base.clone()
        };
        assert_ne!(base.count_key(), filtre.count_key());
    }

    #[test]
    fn l_etat_vide_demande_la_fenetre_ouverte() {
        let req = JournalsQuery::default().to_request();
        assert!(req.q.is_none() && req.fond.is_empty() && !req.closes);
        assert_eq!(req.sort, JournalSort::Echeance);
    }
}
