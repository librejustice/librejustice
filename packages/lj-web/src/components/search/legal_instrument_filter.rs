//! Panneau « Textes cités » (ex-`LegalInstrumentFilter` du rail, transplanté
//! dans le dropdown dédié de la barre de filtres) : recherche/saisie libre
//! (« + Ajouter »), instruments dépliables avec leurs articles (clés composites
//! « instrument|article » dans `la`, ADR 0071).

use leptos::prelude::*;
use lj_dtos::{LegalInstrumentFacet, SearchFacets};

use crate::helpers::cn;

use super::facet_widgets::{scroll_lock_ref, CheckboxOption, FilterSearchInput};

#[component]
pub fn LegalInstrumentPanel(
    #[prop(into)] facets: Signal<Option<SearchFacets>>,
    #[prop(into)] selected_instruments: Signal<Vec<String>>,
    #[prop(into)] selected_articles: Signal<Vec<String>>,
    #[prop(into)] on_toggle_instrument: Callback<String>,
    #[prop(into)] on_toggle_article: Callback<String>,
) -> impl IntoView {
    let filter_text = RwSignal::new(String::new());

    // Liste fusionnée orphelins + facettes, filtrée par needle. `Memo` : lue par le
    // signal `facet` de chaque ligne (lookup par valeur) → O(1) par lecture.
    let all_facets = Memo::new(move |_| {
        let facet_list: Vec<LegalInstrumentFacet> = facets
            .get()
            .map(|f| f.legal_instrument.clone())
            .unwrap_or_default();
        let orphans: Vec<LegalInstrumentFacet> = selected_instruments
            .get()
            .into_iter()
            .filter(|s| !facet_list.iter().any(|f| &f.value == s))
            .map(|s| LegalInstrumentFacet {
                label: s.clone(),
                value: s,
                slug: None,
                count: 0,
                articles: Vec::new(),
            })
            .collect();
        let mut out = orphans;
        out.extend(facet_list);
        out
    });

    let needle = Signal::derive(move || filter_text.get().to_lowercase());

    let visible = Memo::new(move |_| {
        let needle = filter_text.get().to_lowercase();
        let facets = all_facets.get();
        if needle.is_empty() {
            return facets;
        }
        facets
            .into_iter()
            .filter(|f| {
                f.label.to_lowercase().contains(&needle)
                    || f.articles
                        .iter()
                        .any(|a| a.value.to_lowercase().contains(&needle))
            })
            .collect()
    });

    let trimmed = Signal::derive(move || filter_text.get().trim().to_string());
    let can_add = Signal::derive(move || {
        let t = trimmed.get();
        let needle = t.to_lowercase();
        !t.is_empty()
            && !selected_instruments.get().contains(&t)
            && !selected_articles.get().contains(&t)
            && !all_facets
                .get()
                .iter()
                .any(|f| f.value.to_lowercase() == needle)
    });

    let handle_add = move || {
        on_toggle_article.run(trimmed.get_untracked());
        filter_text.set(String::new());
    };
    let handle_add_enter = handle_add;

    view! {
        <FilterSearchInput
            value=filter_text
            placeholder="Filtrer ou saisir un article…"
            on_enter=Callback::new(move |_| {
                if can_add.get_untracked() {
                    handle_add_enter();
                }
            })
        />
        <div
            node_ref=scroll_lock_ref()
            class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto"
        >
            <Show when=move || can_add.get()>
                <button
                    type="button"
                    on:click=move |_| handle_add()
                    class="flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-xs text-[var(--color-accent)] transition-colors hover:bg-[var(--color-vellum)]"
                >
                    <span class="font-medium">"+"</span>
                    " Ajouter «"{move || trimmed.get()}"»"
                </button>
            </Show>
            <Show when=move || visible.get().is_empty() && !can_add.get()>
                <p class="px-2 py-1 text-xs text-[var(--color-ink-subtle)]">
                    "Aucun résultat"
                </p>
            </Show>
            // `<For>` keyé par instrument : les lignes persistent à travers les
            // changements de requête. `LegalInstrumentRow` lit son compteur / ses
            // articles dans `all_facets` (réactif) — un toggle d'article ne
            // reconstruit pas la liste, et `each` ne dépend pas de `la`.
            <For
                each=move || visible.get()
                key=|f: &LegalInstrumentFacet| f.value.clone()
                children=move |f: LegalInstrumentFacet| {
                    let value = f.value.clone();
                    let selected = Signal::derive(move || {
                        selected_instruments.get().contains(&value)
                    });
                    let value_cb = f.value.clone();
                    view! {
                        <LegalInstrumentRow
                            value=f.value.clone()
                            instrument_facets=all_facets
                            selected=selected
                            selected_articles=selected_articles
                            needle=needle
                            on_toggle_instrument=Callback::new(move |_| on_toggle_instrument
                                .run(value_cb.clone()))
                            on_toggle_article=on_toggle_article
                        />
                    }
                }
            />
        </div>
    }
}

// ── LegalInstrumentRow ────────────────────────────────────────────────────────

#[component]
fn LegalInstrumentRow(
    value: String,
    /// Toutes les facettes d'instruments (orphelins + recherche). La ligne y lit son
    /// compteur et ses articles, donc ils se rafraîchissent à l'arrivée des facettes
    /// sans reconstruire la ligne (keyée par valeur côté `<For>`).
    #[prop(into)]
    instrument_facets: Signal<Vec<LegalInstrumentFacet>>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] selected_articles: Signal<Vec<String>>,
    #[prop(into)] needle: Signal<String>,
    #[prop(into)] on_toggle_instrument: Callback<()>,
    #[prop(into)] on_toggle_article: Callback<String>,
) -> impl IntoView {
    let facet_value = value.clone();
    // Token de l'instrument (`ref_text_uid`, ADR 0145 M4), pour les clés d'article
    // composites « uid|numKey » dans `la` (ADR 0071) : un article est porté par SON
    // code, sinon « 1240 » s'allumerait dans tous les codes ayant cet article.
    // `StoredValue` (Copy) : librement re-capturable par les closures réactives /
    // `<For>` imbriquées.
    let value_key = StoredValue::new(value.clone());
    // `Memo` : lus par les signaux `count`/`selected` de chaque ligne-article.
    // `.with` (et non `.get()`) : ne cloner QUE sa propre facette, pas tout le
    // `Vec<LegalInstrumentFacet>` (30 instruments × articles) à chaque lecture, par
    // ligne — sinon O(lignes × facettes) de deep-clones à l'arrivée des facettes.
    let facet = Memo::new(move |_| {
        instrument_facets.with(|l| l.iter().find(|f| f.value == value).cloned())
    });
    let facet_count = Signal::derive(move || facet.with(|f| f.as_ref().map_or(0, |x| x.count)));
    // Libellé réactif (titre catalogue) : arrive avec les facettes ; tant qu'il
    // n'est pas là (orphelin d'URL), le token sert de repli.
    let facet_label = Signal::derive(move || {
        facet.with(|f| {
            f.as_ref()
                .map(|x| x.label.clone())
                .unwrap_or_else(|| facet_value.clone())
        })
    });
    let facet_articles = Memo::new(move |_| {
        facet.with(|f| f.as_ref().map(|x| x.articles.clone()).unwrap_or_default())
    });

    // Articles sélectionnés (`la`) appartenant à CET instrument. Les valeurs `la`
    // sont des clés composites « instrument|article » (ADR 0071) : on garde celles
    // préfixées par cet instrument et on en extrait le libellé d'article nu (pour
    // l'affichage / le compteur, qui restent sur le libellé seul).
    let my_prefix = format!("{}|", value_key.get_value());
    let my_selected = Memo::new(move |_| {
        selected_articles
            .get()
            .into_iter()
            .filter_map(|a| a.strip_prefix(&my_prefix).map(str::to_string))
            .collect::<Vec<_>>()
    });
    // all_articles = sélectionnés ∪ facette (dédup, sélectionnés d'abord).
    let all_articles = Memo::new(move |_| {
        let mut names = my_selected.get();
        for a in facet_articles.get() {
            if !names.contains(&a.value) {
                names.push(a.value);
            }
        }
        names
    });
    // Sous needle, ne garder que les articles matchant ou déjà sélectionnés.
    let visible_articles = Memo::new(move |_| {
        let needle = needle.get();
        let all = all_articles.get();
        if needle.is_empty() {
            return all;
        }
        let sel = my_selected.get();
        all.into_iter()
            .filter(|a| a.to_lowercase().contains(&needle) || sel.contains(a))
            .collect::<Vec<_>>()
    });
    let has_articles = Signal::derive(move || !visible_articles.get().is_empty());
    let needle_active = Signal::derive(move || !needle.get().is_empty());

    // `Memo` : ne notifie qu'au changement effectif du bool (auto-dépli stable).
    let has_selection = Memo::new(move |_| !my_selected.get().is_empty());
    let expanded = RwSignal::new(has_selection.get_untracked());
    // Auto-dépli sens unique : déplie quand une sélection apparaît, jamais l'inverse.
    // Décocher le dernier article laisse l'instrument déplié (repli au chevron seul).
    Effect::new(move |_| {
        if has_selection.get() {
            expanded.set(true);
        }
    });
    // Lazy-mount des articles : on ne monte les ≤20 lignes-articles d'un instrument
    // qu'au premier dépli (clic, sélection présente, ou recherche active). Sinon les
    // ≤600 lignes-articles des 30 instruments restent montées (le repli n'est que du
    // CSS, `<Show>` les rend dès qu'il y a des articles) → leurs compteurs recalculent
    // à chaque arrivée de facettes ET le navigateur re-layoute les 293 nœuds à chaque
    // pli/dépli du groupe. Monté une fois au dépli, gardé (parité `TreeRow`).
    let has_opened = RwSignal::new(has_selection.get_untracked());
    Effect::new(move |_| {
        if expanded.get() || needle_active.get() {
            has_opened.set(true);
        }
    });

    let chevron_class = move || {
        cn([
            "flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]",
            if !has_articles.get() { "pointer-events-none opacity-0" } else { "" },
        ])
    };

    view! {
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-1">
                <button
                    type="button"
                    aria-label=move || if expanded.get() { "Réduire" } else { "Développer" }
                    on:click=move |_| expanded.update(|v| *v = !*v)
                    class=chevron_class
                >
                    <svg
                        viewBox="0 0 12 12"
                        class=move || {
                            cn([
                                "h-3 w-3 transition-transform",
                                if expanded.get() { "rotate-90" } else { "" },
                            ])
                        }
                        fill="none"
                        aria-hidden="true"
                    >
                        <path
                            d="M4 2l4 4-4 4"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        />
                    </svg>
                </button>
                <CheckboxOption
                    label=facet_label
                    count=facet_count
                    selected=selected
                    on_toggle=on_toggle_instrument
                    indent=false
                />
            </div>
            <Show when=move || has_articles.get()>
                <div
                    class="grid"
                    style=move || {
                        format!(
                            "grid-template-rows: {}; transition: grid-template-rows 200ms ease-out",
                            if expanded.get() || needle_active.get() { "1fr" } else { "0fr" },
                        )
                    }
                >
                    <div class="min-h-0 overflow-hidden">
                        <div class="ml-6 flex flex-col gap-1 pt-0.5">
                            // Lazy-mount : les articles ne sont montés qu'au premier
                            // dépli de l'instrument (ou recherche active). Tant que
                            // l'instrument n'a pas été ouvert, aucune ligne-article
                            // vivante → pas de cascade, pas de layout à chaque pli.
                            {move || {
                                has_opened
                                    .get()
                                    .then(|| {
                                        view! {
                                            <For
                                                each=move || visible_articles.get()
                                                key=|art: &String| art.clone()
                                                children=move |art: String| {
                                                    let art_count = art.clone();
                                                    let count = Signal::derive(move || {
                                                        facet_articles
                                                            .with(|a| {
                                                                a.iter()
                                                                    .find(|c| c.value == art_count)
                                                                    .map_or(0, |c| c.count)
                                                            })
                                                    });
                                                    let art_sel = art.clone();
                                                    let is_sel = Signal::derive(move || {
                                                        my_selected.with(|s| s.contains(&art_sel))
                                                    });
                                                    // Clé composite « instrument|article »
                                                    // émise dans `la` (ADR 0071).
                                                    let art_key = format!("{}|{}", value_key.get_value(), art);
                                                    view! {
                                                        <CheckboxOption
                                                            label=art.clone()
                                                            count=count
                                                            selected=is_sel
                                                            on_toggle=Callback::new(move |_| on_toggle_article
                                                                .run(art_key.clone()))
                                                            indent=false
                                                        />
                                                    }
                                                }
                                            />
                                        }
                                    })
                            }}
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}
