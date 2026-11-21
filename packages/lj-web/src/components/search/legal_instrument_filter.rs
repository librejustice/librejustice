//! « Textes cités » panel: expandable instruments with their articles
//! (composite « instrument|article » keys in `la`), and an input that queries
//! the whole catalogue, the facet only serving the head of it.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use lj_dtos::{FacetChoice, LegalInstrumentFacet, SearchFacets};

use crate::helpers::cn;

use super::compact_search::search_data::instruments_resource;
use super::compact_search::search_state::key_from_map;
use super::facet_widgets::{CheckboxOption, FilterSearchInput};

#[component]
pub fn LegalInstrumentPanel(
    #[prop(into)] facets: Signal<Option<SearchFacets>>,
    #[prop(into)] selected_instruments: Signal<Vec<String>>,
    #[prop(into)] selected_articles: Signal<Vec<String>>,
    #[prop(into)] on_toggle_instrument: Callback<String>,
    #[prop(into)] on_toggle_article: Callback<String>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let filter_text = RwSignal::new(String::new());
    let query_map = use_query_map();
    let scope = crate::jurisdiction::use_reader_scope();
    let lang = crate::i18n::use_reader_lang().get();

    // `FilterSearchInput` debounces, so the key turns once per pause and not
    // once per keystroke.
    let suggest = instruments_resource(Signal::derive(move || {
        (
            key_from_map(&query_map.get(), scope.code(), lang.code()).count_key(),
            filter_text.get().trim().to_string(),
        )
    }));
    let suggested = Signal::derive(move || {
        suggest
            .get()
            .map(|s| s.legal_instrument)
            .unwrap_or_default()
    });
    // The keys the catalogue answered for, instruments bare and articles under
    // their composite. They enter the list whatever the typed text looks like:
    // « L761-2 du code de la consommation » matches no label of its own row.
    let designated = Memo::new(move |_| {
        let mut out: HashSet<String> = HashSet::new();
        for f in suggested.get() {
            for a in &f.articles {
                out.insert(format!("{}|{}", f.value, a.value));
            }
            out.insert(f.value);
        }
        out
    });

    // Orphans, facets and catalogue in one list. `Memo`: read by the `facet`
    // signal of each row, looked up by value.
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
        for row in suggested.get() {
            match out.iter_mut().find(|f| f.value == row.value) {
                Some(known) => {
                    if known.label == known.value {
                        known.label = row.label;
                    }
                    known.slug = known.slug.take().or(row.slug);
                    known.count = known.count.max(row.count);
                    for article in row.articles {
                        if !known.articles.iter().any(|a| a.value == article.value) {
                            known.articles.push(article);
                        }
                    }
                }
                None => out.push(row),
            }
        }
        out
    });

    let needle = Signal::derive(move || filter_text.get().to_lowercase());

    let rank = Memo::new(move |_| {
        suggested
            .get()
            .iter()
            .enumerate()
            .map(|(i, f)| (f.value.clone(), i))
            .collect::<HashMap<String, usize>>()
    });

    let visible = Memo::new(move |_| {
        let needle = filter_text.get().to_lowercase();
        let facets = all_facets.get();
        if needle.is_empty() {
            return facets;
        }
        let mut out: Vec<LegalInstrumentFacet> = facets
            .into_iter()
            .filter(|f| {
                f.label.to_lowercase().contains(&needle)
                    || f.articles
                        .iter()
                        .any(|a| a.label.to_lowercase().contains(&needle))
                    || designated.with(|d| d.contains(&f.value))
            })
            .collect();
        out.sort_by_key(|f| rank.with(|r| r.get(&f.value).copied().unwrap_or(usize::MAX)));
        out
    });

    // Nothing found is a claim about the catalogue, which only its answer can
    // make: while that flies, the panel says it is looking.
    let waiting = Signal::derive(move || suggest.get().is_none());

    view! {
        <FilterSearchInput value=filter_text placeholder=crate::i18n::use_ui().search.filter_cited_texts_placeholder />
        <div
            class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto overscroll-contain"
        >
            <Show when=move || visible.get().is_empty()>
                <p class="px-2 py-1 text-xs text-[var(--color-ink-subtle)]">
                    {move || {
                        if waiting.get() { ui.catalogue_searching } else { ui.no_results }
                    }}
                </p>
            </Show>
            // `<For>` keyed by instrument: rows survive a change of question.
            // `LegalInstrumentRow` reads its counter and its articles off
            // `all_facets`, so ticking an article does not rebuild the list.
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
                            designated=designated
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
    /// compteur et ses articles, qui se rafraîchissent à l'arrivée des facettes sans
    /// reconstruire la ligne.
    #[prop(into)]
    instrument_facets: Signal<Vec<LegalInstrumentFacet>>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] selected_articles: Signal<Vec<String>>,
    #[prop(into)] needle: Signal<String>,
    /// Composite keys the catalogue answered for: they survive the needle
    /// filter, which reads a label they do not have to contain.
    #[prop(into)]
    designated: Signal<HashSet<String>>,
    #[prop(into)] on_toggle_instrument: Callback<()>,
    #[prop(into)] on_toggle_article: Callback<String>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let facet_value = value.clone();
    // Token de l'instrument (`ref_text_uid`), pour les clés d'article
    // composites « uid|numKey » dans `la` : un article est porté par SON
    // code, sinon « 1240 » s'allumerait dans tous les codes ayant cet article.
    // `StoredValue` (Copy) : librement re-capturable par les closures réactives.
    let value_key = StoredValue::new(value.clone());
    // `.with` et non `.get()` : ne cloner QUE sa propre facette, pas tout le
    // `Vec<LegalInstrumentFacet>` à chaque lecture et par ligne.
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
    // sont des clés composites « instrument|article » : on garde celles préfixées
    // par cet instrument et on en extrait le libellé d'article nu, sur lequel
    // restent l'affichage et le compteur.
    let my_prefix = format!("{}|", value_key.get_value());
    let my_selected = Memo::new(move |_| {
        selected_articles
            .get()
            .into_iter()
            .filter_map(|a| a.strip_prefix(&my_prefix).map(str::to_string))
            .collect::<Vec<_>>()
    });
    // Selected ∪ facet, deduplicated, selected first. A selected article
    // absent from the facet has no label of its own: its key stands in.
    let all_articles = Memo::new(move |_| {
        let known = facet_articles.get();
        let mut out: Vec<FacetChoice> = Vec::new();
        for value in my_selected.get() {
            let choice = known.iter().find(|c| c.value == value).cloned();
            out.push(choice.unwrap_or(FacetChoice {
                label: value.clone(),
                value,
                count: 0,
                parent: None,
                hint: None,
            }));
        }
        for choice in known {
            if !out.iter().any(|o| o.value == choice.value) {
                out.push(choice);
            }
        }
        out
    });
    // Under a needle, keep what matches it, what the catalogue named, and what
    // is already ticked.
    let visible_articles = Memo::new(move |_| {
        let needle = needle.get();
        let all = all_articles.get();
        if needle.is_empty() {
            return all;
        }
        let sel = my_selected.get();
        let prefix = format!("{}|", value_key.get_value());
        all.into_iter()
            .filter(|c| {
                c.label.to_lowercase().contains(&needle)
                    || sel.contains(&c.value)
                    || designated.with(|d| d.contains(&format!("{prefix}{}", c.value)))
            })
            .collect::<Vec<_>>()
    });
    let has_articles = Signal::derive(move || !visible_articles.get().is_empty());
    let needle_active = Signal::derive(move || !needle.get().is_empty());

    let has_selection = Memo::new(move |_| !my_selected.get().is_empty());
    let expanded = RwSignal::new(has_selection.get_untracked() || needle_active.get_untracked());
    Effect::new(move |_| {
        if has_selection.get() {
            expanded.set(true);
        }
    });
    Effect::new(move |was: Option<bool>| {
        let active = needle_active.get();
        if was != Some(active) {
            expanded.set(active || has_selection.get_untracked());
        }
        active
    });
    let has_opened = RwSignal::new(expanded.get_untracked());
    Effect::new(move |_| {
        if expanded.get() {
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
                    aria-label=move || if expanded.get() { ui.collapse } else { ui.expand }
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
                            if expanded.get() { "1fr" } else { "0fr" },
                        )
                    }
                >
                    <div class="min-h-0 overflow-hidden">
                        <div class="ml-6 flex flex-col gap-1 pt-0.5">
                            // Lazy-mount : les articles ne sont montés qu'au premier
                            // dépli de l'instrument (ou recherche active).
                            {move || {
                                has_opened
                                    .get()
                                    .then(|| {
                                        view! {
                                            <For
                                                each=move || visible_articles.get()
                                                key=|art: &FacetChoice| art.value.clone()
                                                children=move |art: FacetChoice| {
                                                    let art_count = art.value.clone();
                                                    let count = Signal::derive(move || {
                                                        facet_articles
                                                            .with(|a| {
                                                                a.iter()
                                                                    .find(|c| c.value == art_count)
                                                                    .map_or(0, |c| c.count)
                                                            })
                                                    });
                                                    let art_sel = art.value.clone();
                                                    let is_sel = Signal::derive(move || {
                                                        my_selected.with(|s| s.contains(&art_sel))
                                                    });
                                                    // Clé composite « instrument|article »
                                                    // émise dans `la`.
                                                    let art_key = format!("{}|{}", value_key.get_value(), art.value);
                                                    view! {
                                                        <CheckboxOption
                                                            label=art.label.clone()
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
