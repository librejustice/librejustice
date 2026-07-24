//! `DecisionsFilterBar` — barre de filtres horizontale sous la barre de
//! recherche (gabarit de référence §9.1) : `Date ▾ · Juridiction ▾ · Portée ▾ ·
//! Domaine du droit ▾ · Textes cités ▾ · Plus de filtres`. « Plus de filtres »
//! étend la barre en place avec les dropdowns Office, Dispositif et Publication
//! (pas de modal) ; ils restent épinglés tant qu'un de leurs filtres est actif.
//!
//! Sync bidirectionnelle facettes ↔ query params, mutations via `query_state`
//! (replace=true, drop `page`) — mécanique héritée verbatim de l'ex-FilterRail.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use lj_dtos::{FacetChoice, SearchFacets};

use super::compact_search::{query_state, DraftQuery};
use super::date_range_picker::DateRangePicker;
use super::facet_widgets::{
    build_tree, scroll_lock_ref, CheckboxOption, FacetChecklist, FilterSearchInput, Nav, TreeRow,
};
use super::filter_dropdown::{FilterDropdown, OpenDropdown};
use super::legal_instrument_filter::LegalInstrumentPanel;

#[component]
pub fn DecisionsFilterBar(#[prop(into)] facets: Signal<Option<SearchFacets>>) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    // Un seul dropdown ouvert à la fois dans la barre.
    provide_context(OpenDropdown(RwSignal::new(None)));

    let get_all = move |key: &str| query_map.get().get_all(key).unwrap_or_default();

    // Map courante avec `q` réécrit par le texte (non soumis) de la barre : toute
    // mutation de filtre applique CE texte, pour ajuster requête + filtres d'un
    // seul geste. Absent le provider (composant isolé), on garde le `q` de l'URL.
    let draft = use_context::<DraftQuery>().map(|d| d.0);
    let effective_map = move || {
        let mut map = query_map.get_untracked();
        if let Some(draft) = draft {
            let q = draft.get_untracked().trim().to_string();
            if q.is_empty() {
                map.remove("q");
            } else {
                map.replace("q".to_string(), q);
            }
        }
        map
    };

    let jur_filter = RwSignal::new(String::new());

    // ── Callbacks de mutation (`nav` est `Copy`, capturé par copie) ──────────
    let toggle = move |key: String, value: String| {
        nav.go(query_state::toggle_multi(&effective_map(), &key, &value));
    };
    let on_dates = move |(f, t): (String, String)| {
        nav.go(query_state::with_dates(
            &effective_map(),
            Some(&f),
            Some(&t),
        ));
    };

    // ── Sélections courantes (URL) ────────────────────────────────────────────
    let jurs = Signal::derive(move || get_all("jurisdictionType"));
    let offices = Signal::derive(move || get_all("office"));
    let jcodes = Signal::derive(move || get_all("jurisdictionCode"));
    let chambres = Signal::derive(move || get_all("chamber"));
    let domaines = Signal::derive(move || get_all("legalDomain"));
    let solutions = Signal::derive(move || get_all("solution"));
    let portees = Signal::derive(move || get_all("significance"));
    let pubs = Signal::derive(move || get_all("publication"));
    let lis = Signal::derive(move || get_all("legalInstrument"));
    let las = Signal::derive(move || get_all("legalArticle"));
    let date_from = Signal::derive(move || query_map.get().get("dateFrom").unwrap_or_default());
    let date_to = Signal::derive(move || query_map.get().get("dateTo").unwrap_or_default());

    // ── Juridiction : arbre facette (racines `jurisdiction_type:*`, enfants = codes
    // `jurisdiction`). `Memo` : lu par les signaux par ligne (compteurs,
    // sélection) — mémoïsé = calculé une fois par changement de facettes/saisie.
    let jurisdiction_choices =
        Memo::new(move |_| facets.get().map(|f| f.jurisdiction).unwrap_or_default());
    let jurisdiction_tree = Memo::new(move |_| {
        let needle = jur_filter.get().to_lowercase();
        build_tree(&jurisdiction_choices.get())
            .into_iter()
            .filter(|(root, children)| {
                needle.is_empty()
                    || root.label.to_lowercase().contains(&needle)
                    || children
                        .iter()
                        .any(|c| c.label.to_lowercase().contains(&needle))
            })
            .collect::<Vec<_>>()
    });
    // Sélections absentes de la facette : lignes orphelines plates
    // `(clé d'URL, valeur)` en fin de liste.
    let juridiction_orphans = Memo::new(move |_| {
        let choices = jurisdiction_choices.get();
        let mut out: Vec<(&'static str, String)> = Vec::new();
        for s in jurs.get() {
            if !choices.iter().any(|c| c.value == s) {
                out.push(("jurisdictionType", s));
            }
        }
        for s in jcodes.get() {
            if !choices.iter().any(|c| c.value == s) {
                out.push(("jurisdictionCode", s));
            }
        }
        out
    });

    // ── Domaine : arbre facette (valeurs = suffixes `legal_domain:*`, racine et
    // feuille se filtrent par la même clé).
    let domaine_choices =
        Memo::new(move |_| facets.get().map(|f| f.legal_domain).unwrap_or_default());
    let domaine_tree = Memo::new(move |_| build_tree(&domaine_choices.get()));
    let domaine_orphans = Memo::new(move |_| {
        let choices = domaine_choices.get();
        domaines
            .get()
            .into_iter()
            .filter(|s| !choices.iter().any(|c| &c.value == s))
            .collect::<Vec<_>>()
    });

    let office_choices = Signal::derive(move || facets.get().map(|f| f.office).unwrap_or_default());
    let chamber_choices =
        Signal::derive(move || facets.get().map(|f| f.chamber).unwrap_or_default());
    let solution_choices =
        Signal::derive(move || facets.get().map(|f| f.solution).unwrap_or_default());
    let significance_choices =
        Signal::derive(move || facets.get().map(|f| f.significance).unwrap_or_default());
    let publication_choices =
        Signal::derive(move || facets.get().map(|f| f.publication).unwrap_or_default());

    // ── Compteurs actifs des boutons ─────────────────────────────────────────
    let jur_count = Signal::derive(move || jurs.get().len() + jcodes.get().len());
    let office_count = Signal::derive(move || offices.get().len());
    let chamber_count = Signal::derive(move || chambres.get().len());
    let domaine_count = Signal::derive(move || domaines.get().len());
    let dates_count = Signal::derive(move || {
        usize::from(!date_from.get().is_empty() || !date_to.get().is_empty())
    });
    let textes_count = Signal::derive(move || lis.get().len() + las.get().len());
    let solution_count = Signal::derive(move || solutions.get().len());
    let significance_count = Signal::derive(move || portees.get().len());
    let publication_count = Signal::derive(move || pubs.get().len());
    let more_count = Signal::derive(move || {
        offices.get().len() + chambres.get().len() + solutions.get().len() + pubs.get().len()
    });

    // « Plus de filtres » déplie Office + Dispositif + Publication EN PLACE
    // dans la barre ; un filtre actif dans ce groupe les épingle (on ne cache
    // jamais un filtre appliqué) et le bouton de bascule disparaît.
    let more_expanded = RwSignal::new(false);
    let show_more = Signal::derive(move || more_expanded.get() || more_count.get() > 0);

    view! {
        // Pas de grisage pendant un refetch : les dropdowns gardent les
        // facettes précédentes et restent pleinement interactifs (on peut
        // enchaîner les coches) — le grisage `opacity-60` donnait une
        // impression de menu figé/transparent à chaque clic de filtre.
        <div
            role="toolbar"
            aria-label="Filtres de recherche"
            class="flex flex-wrap items-center gap-2 pb-1"
        >
            <FilterDropdown id="date" label="Date" active_count=dates_count>
                <DateRangePicker
                    from=date_from
                    to=date_to
                    on_change=Callback::new(on_dates)
                />
            </FilterDropdown>

            <FilterDropdown id="juridiction" label="Juridiction" active_count=jur_count>
                <FilterSearchInput value=jur_filter placeholder="Filtrer (ex. Lyon…)" />
                <div
                    node_ref=scroll_lock_ref()
                    class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto"
                >
                    // `<For>` keyé par uid racine : `each` ne dépend que des facettes
                    // + de la saisie — JAMAIS de la sélection. Un clic de filtre ne
                    // reconstruit donc pas la liste : seuls les signaux par ligne
                    // (`selected`, compteurs) se mettent à jour en place.
                    <For
                        each=move || jurisdiction_tree.get()
                        key=|(root, _): &(FacetChoice, Vec<FacetChoice>)| root.value.clone()
                        children=move |(root, _): (FacetChoice, Vec<FacetChoice>)| {
                            let uid = root.value.clone();
                            let toggle_value = uid.clone();
                            // Ligne réactive : compteur/libellé/enfants relus dans le
                            // `Memo` d'arbre à chaque arrivée de facettes (la ligne
                            // keyée persiste, ses props se rafraîchissent en place).
                            let row_uid = uid.clone();
                            let row = Memo::new(move |_| {
                                jurisdiction_tree
                                    .get()
                                    .into_iter()
                                    .find(|(r, _)| r.value == row_uid)
                            });
                            let label_uid = uid.clone();
                            let label = Signal::derive(move || {
                                row.get().map(|(r, _)| r.label).unwrap_or(label_uid.clone())
                            });
                            let count = Signal::derive(move || {
                                row.get().map(|(r, _)| r.count).unwrap_or(0)
                            });
                            let child_choices = Signal::derive(move || {
                                row.get().map(|(_, c)| c).unwrap_or_default()
                            });
                            let sel_value = toggle_value.clone();
                            let selected =
                                Signal::derive(move || jurs.get().contains(&sel_value));
                            let toggle_root = toggle;
                            let toggle_child = toggle;
                            view! {
                                <TreeRow
                                    label=label
                                    count=count
                                    selected=selected
                                    child_choices=child_choices
                                    selected_children=jcodes
                                    on_toggle=Callback::new(move |_| toggle_root(
                                        "jurisdictionType".to_string(),
                                        toggle_value.clone(),
                                    ))
                                    on_toggle_child=Callback::new(move |code: String| toggle_child(
                                        "jurisdictionCode".to_string(),
                                        code,
                                    ))
                                />
                            }
                        }
                    />
                    <For
                        each=move || juridiction_orphans.get()
                        key=|(key, value): &(&'static str, String)| format!("{key}:{value}")
                        children=move |(key, value): (&'static str, String)| {
                            let sel_value = value.clone();
                            let is_sel = Signal::derive(move || {
                                let selection = match key {
                                    "jurisdictionCode" => jcodes.get(),
                                    _ => jurs.get(),
                                };
                                selection.contains(&sel_value)
                            });
                            let toggle_orphan = toggle;
                            let toggle_value = value.clone();
                            view! {
                                <CheckboxOption
                                    label=value.clone()
                                    count=Signal::derive(|| 0)
                                    selected=is_sel
                                    on_toggle=Callback::new(move |_| toggle_orphan(
                                        key.to_string(),
                                        toggle_value.clone(),
                                    ))
                                    indent=false
                                />
                            }
                        }
                    />
                </div>
            </FilterDropdown>

            // Portée jurisprudentielle (majeure/importante/limitée/indéterminée,
            // groupes de `publication_codes` — ADR 0167). Jumelle assumée de
            // Publication, en lecture normalisée inter-ordres (gabarit ).
            <FilterDropdown id="significance" label="Portée" active_count=significance_count>
                <FacetChecklist
                    choices=significance_choices
                    selected=portees
                    on_toggle=Callback::new(move |value: String| toggle(
                        "significance".to_string(),
                        value,
                    ))
                />
            </FilterDropdown>

            // Masqué quand la facette ne remonte rien (couverture domaine
            // partielle du corpus) — sauf sélection active, qu'on ne cache pas.
            <Show when=move || {
                !domaine_tree.get().is_empty() || domaine_count.get() > 0
            }>
            <FilterDropdown id="legalDomain" label="Domaine du droit" active_count=domaine_count>
                <div
                    node_ref=scroll_lock_ref()
                    class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto"
                >
                    <For
                        each=move || domaine_tree.get()
                        key=|(root, _): &(FacetChoice, Vec<FacetChoice>)| root.value.clone()
                        children=move |(root, _): (FacetChoice, Vec<FacetChoice>)| {
                            let value = root.value.clone();
                            let row_value = value.clone();
                            let row = Memo::new(move |_| {
                                domaine_tree
                                    .get()
                                    .into_iter()
                                    .find(|(r, _)| r.value == row_value)
                            });
                            let label_value = value.clone();
                            let label = Signal::derive(move || {
                                row.get().map(|(r, _)| r.label).unwrap_or(label_value.clone())
                            });
                            let count = Signal::derive(move || {
                                row.get().map(|(r, _)| r.count).unwrap_or(0)
                            });
                            let child_choices = Signal::derive(move || {
                                row.get().map(|(_, c)| c).unwrap_or_default()
                            });
                            let sel_value = value.clone();
                            let selected = Signal::derive(move || {
                                domaines.get().contains(&sel_value)
                            });
                            let toggle_root = toggle;
                            let toggle_child = toggle;
                            let toggle_value = value.clone();
                            view! {
                                <TreeRow
                                    label=label
                                    count=count
                                    selected=selected
                                    child_choices=child_choices
                                    selected_children=domaines
                                    on_toggle=Callback::new(move |_| toggle_root(
                                        "legalDomain".to_string(),
                                        toggle_value.clone(),
                                    ))
                                    on_toggle_child=Callback::new(move |leaf: String| toggle_child(
                                        "legalDomain".to_string(),
                                        leaf,
                                    ))
                                />
                            }
                        }
                    />
                    <For
                        each=move || domaine_orphans.get()
                        key=|value: &String| value.clone()
                        children=move |value: String| {
                            let sel_value = value.clone();
                            let is_sel = Signal::derive(move || {
                                domaines.get().contains(&sel_value)
                            });
                            let toggle_orphan = toggle;
                            let toggle_value = value.clone();
                            view! {
                                <CheckboxOption
                                    label=value.clone()
                                    count=Signal::derive(|| 0)
                                    selected=is_sel
                                    on_toggle=Callback::new(move |_| toggle_orphan(
                                        "legalDomain".to_string(),
                                        toggle_value.clone(),
                                    ))
                                    indent=false
                                />
                            }
                        }
                    />
                </div>
            </FilterDropdown>
            </Show>

            <FilterDropdown id="textes" label="Textes cités" active_count=textes_count>
                <LegalInstrumentPanel
                    facets=facets
                    selected_instruments=lis
                    selected_articles=las
                    on_toggle_instrument=Callback::new(move |value: String| toggle(
                        "legalInstrument".to_string(),
                        value,
                    ))
                    on_toggle_article=Callback::new(move |value: String| toggle(
                        "legalArticle".to_string(),
                        value,
                    ))
                />
            </FilterDropdown>

            <Show when=move || show_more.get()>
                // Office du juge (JEX, JAF, JCP, JLD, magistrat désigné…) : axe
                // séparé de la juridiction (ADR 0163), en miroir du filtre
                // `office_uid`. Masqué quand la facette est vide, sauf sélection.
                <Show when=move || {
                    !office_choices.get().is_empty() || office_count.get() > 0
                }>
                <FilterDropdown id="office" label="Office" active_count=office_count>
                    <FacetChecklist
                        choices=office_choices
                        selected=offices
                        on_toggle=Callback::new(move |value: String| toggle(
                            "office".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>
                </Show>

                // Chambre (catégorie contrôlée uniforme tous ordres, ADR 0172) :
                // sous-unité du siège, en miroir du filtre `chamber_uid`. Masquée
                // quand la facette est vide, sauf sélection active.
                <Show when=move || {
                    !chamber_choices.get().is_empty() || chamber_count.get() > 0
                }>
                <FilterDropdown id="chamber" label="Chambre" active_count=chamber_count>
                    <FacetChecklist
                        choices=chamber_choices
                        selected=chambres
                        on_toggle=Callback::new(move |value: String| toggle(
                            "chamber".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>
                </Show>

                <FilterDropdown id="dispositif" label="Dispositif" active_count=solution_count>
                    <FacetChecklist
                        choices=solution_choices
                        selected=solutions
                        on_toggle=Callback::new(move |value: String| toggle(
                            "solution".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>

                <FilterDropdown id="publication" label="Publication" active_count=publication_count>
                    <FacetChecklist
                        choices=publication_choices
                        selected=pubs
                        on_toggle=Callback::new(move |value: String| toggle(
                            "publication".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>
            </Show>

            <Show when=move || { more_count.get() == 0 }>
                <button
                    type="button"
                    on:click=move |_| more_expanded.update(|v| *v = !*v)
                    class="flex shrink-0 items-center gap-1.5 rounded border border-[var(--color-rule)] px-2.5 py-1 text-xs text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]"
                >
                    <span class="whitespace-nowrap">
                        {move || {
                            if more_expanded.get() { "Moins de filtres" } else { "Plus de filtres" }
                        }}
                    </span>
                </button>
            </Show>
        </div>
    }
}
