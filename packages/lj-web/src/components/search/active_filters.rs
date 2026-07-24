//! `ActiveFilterChips` — chips des filtres actifs sous la barre (gabarit
//! de référence) : une chip par valeur sélectionnée (✕ = retrait), les bornes de
//! dates fusionnées en une seule chip, « Tout effacer » à droite.
//!
//! `decision_chips` est pure (ParamsMap + facettes → chips) : les libellés se
//! résolvent dans les facettes courantes, une valeur absente (URL partagée)
//! retombe sur sa valeur brute.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use leptos_router::params::ParamsMap;
use lj_dtos::SearchFacets;

use super::compact_search::{query_state, DraftQuery};
use super::facet_widgets::{filter_keys, Nav};

/// Chip de filtre actif. `key`/`value` = mutation URL du ✕ (`key == "dates"` :
/// chip fusionnée `from`+`to`, retrait = vider les deux bornes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChip {
    pub key: &'static str,
    pub value: String,
    pub label: String,
}

/// Chips des filtres décisions actifs, dans l'ordre des clés de `filter_keys`.
pub fn decision_chips(map: &ParamsMap, facets: Option<&SearchFacets>) -> Vec<ActiveChip> {
    let all = |key: &str| map.get_all(key).unwrap_or_default();
    let mut chips = Vec::new();

    // Juridiction : racines = tokens `jurisdiction_type`, enfants (`jcode`)
    // à plat dans le même arbre.
    let jurisdiction = facets
        .map(|f| f.jurisdiction.as_slice())
        .unwrap_or_default();
    for v in all("jurisdictionType") {
        let label = jurisdiction
            .iter()
            .find(|c| c.value == v && c.parent.is_none())
            .map(|c| c.label.clone())
            .unwrap_or_else(|| v.clone());
        chips.push(ActiveChip {
            key: "jurisdictionType",
            value: v,
            label,
        });
    }
    // Office : facette plate à valeurs suffixe (`JEX`), ADR 0163.
    let office = facets.map(|f| f.office.as_slice()).unwrap_or_default();
    for v in all("office") {
        let label = office
            .iter()
            .find(|c| c.value == v)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| v.clone());
        chips.push(ActiveChip {
            key: "office",
            value: v,
            label,
        });
    }
    for v in all("jurisdictionCode") {
        let label = jurisdiction
            .iter()
            .find(|c| c.value == v)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| v.clone());
        chips.push(ActiveChip {
            key: "jurisdictionCode",
            value: v,
            label,
        });
    }

    for (key, choices) in [
        ("chamber", facets.map(|f| f.chamber.as_slice())),
        ("legalDomain", facets.map(|f| f.legal_domain.as_slice())),
        ("solution", facets.map(|f| f.solution.as_slice())),
        ("significance", facets.map(|f| f.significance.as_slice())),
        ("publication", facets.map(|f| f.publication.as_slice())),
    ] {
        let choices = choices.unwrap_or_default();
        for v in all(key) {
            let label = choices
                .iter()
                .find(|c| c.value == v)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| v.clone());
            chips.push(ActiveChip {
                key,
                value: v,
                label,
            });
        }
    }

    // Textes cités : instruments (`li`, libellé catalogue) et articles (`la`,
    // clés composites « instrument|article », ADR 0071) — libellé
    // « instrument · art. N ».
    let instruments = facets
        .map(|f| f.legal_instrument.as_slice())
        .unwrap_or_default();
    let instrument_label = |uid: &str| {
        instruments
            .iter()
            .find(|f| f.value == uid)
            .map(|f| f.label.clone())
            .unwrap_or_else(|| uid.to_string())
    };
    for v in all("legalInstrument") {
        let label = instrument_label(&v);
        chips.push(ActiveChip {
            key: "legalInstrument",
            value: v,
            label,
        });
    }
    for v in all("legalArticle") {
        let label = match v.split_once('|') {
            Some((uid, art)) => format!("{} · art. {art}", instrument_label(uid)),
            None => v.clone(),
        };
        chips.push(ActiveChip {
            key: "legalArticle",
            value: v,
            label,
        });
    }

    // Dates : une seule chip pour `from`+`to`.
    let from = map.get("dateFrom").unwrap_or_default();
    let to = map.get("dateTo").unwrap_or_default();
    let date_label = match (from.is_empty(), to.is_empty()) {
        (false, false) => Some(format!("Du {from} au {to}")),
        (false, true) => Some(format!("Depuis le {from}")),
        (true, false) => Some(format!("Jusqu'au {to}")),
        (true, true) => None,
    };
    if let Some(label) = date_label {
        chips.push(ActiveChip {
            key: "dates",
            value: String::new(),
            label,
        });
    }

    chips
}

#[component]
pub fn ActiveFilterChips(#[prop(into)] facets: Signal<Option<SearchFacets>>) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();

    // Map courante avec `q` réécrit par le texte (non soumis) de la barre —
    // même règle que les mutations de la barre de filtres (contexte DraftQuery).
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

    let chips = Memo::new(move |_| decision_chips(&query_map.get(), facets.get().as_ref()));

    let remove = move |chip: &ActiveChip| {
        let qs = if chip.key == "dates" {
            query_state::with_dates(&effective_map(), None, None)
        } else {
            query_state::toggle_multi(&effective_map(), chip.key, &chip.value)
        };
        nav.go(qs);
    };
    let clear_all = move |_| {
        nav.go(query_state::without_keys(&effective_map(), &filter_keys()));
    };

    view! {
        <Show when=move || !chips.with(|c| c.is_empty())>
            <div class="flex flex-wrap items-center gap-2">
                // Clé = key:value:LABEL : le libellé arrive avec les facettes
                // (après la chip, rendue d'abord sur la valeur brute) — l'inclure
                // dans la clé reconstruit la ligne à la résolution. Une poignée de
                // chips : reconstruction triviale, pas de signal par ligne.
                <For
                    each=move || chips.get()
                    key=|chip: &ActiveChip| format!("{}:{}:{}", chip.key, chip.value, chip.label)
                    children=move |chip: ActiveChip| {
                        let label = chip.label.clone();
                        let on_remove = move |_| remove(&chip);
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
                    on:click=clear_all
                    class="text-xs text-[var(--color-ink-subtle)] underline-offset-2 hover:text-[var(--color-accent)] hover:underline"
                >
                    "Tout effacer"
                </button>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{FacetChoice, LegalInstrumentFacet};

    fn map_of(pairs: &[(&str, &str)]) -> ParamsMap {
        let mut m = ParamsMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        m
    }

    fn facets() -> SearchFacets {
        SearchFacets {
            jurisdiction: vec![
                FacetChoice {
                    value: "TJ".into(),
                    label: "Tribunal judiciaire".into(),
                    count: 10,
                    parent: None,
                },
                FacetChoice {
                    value: "tj75".into(),
                    label: "TJ de Paris".into(),
                    count: 4,
                    parent: Some("TJ".into()),
                },
            ],
            chamber: Vec::new(),
            office: vec![FacetChoice {
                value: "JEX".into(),
                label: "Juge de l'exécution".into(),
                count: 2,
                parent: None,
            }],
            legal_domain: vec![FacetChoice {
                value: "civil".into(),
                label: "Civil".into(),
                count: 7,
                parent: None,
            }],
            solution: Vec::new(),
            significance: Vec::new(),
            publication: Vec::new(),
            date_lecture_year: Vec::new(),
            legal_instrument: vec![LegalInstrumentFacet {
                value: "code-civil".into(),
                label: "Code civil".into(),
                slug: None,
                count: 5,
                articles: Vec::new(),
            }],
        }
    }

    #[test]
    fn labels_resolved_from_facets() {
        let m = map_of(&[
            ("jurisdictionType", "TJ"),
            ("office", "JEX"),
            ("jurisdictionCode", "tj75"),
            ("legalDomain", "civil"),
        ]);
        let chips = decision_chips(&m, Some(&facets()));
        let labels: Vec<&str> = chips.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Tribunal judiciaire",
                "Juge de l'exécution",
                "TJ de Paris",
                "Civil"
            ]
        );
    }

    #[test]
    fn orphan_falls_back_to_raw_value() {
        let m = map_of(&[
            ("jurisdictionType", "XX"),
            ("legalInstrument", "code-inconnu"),
        ]);
        let chips = decision_chips(&m, Some(&facets()));
        assert_eq!(chips[0].label, "XX");
        assert_eq!(chips[1].label, "code-inconnu");
    }

    #[test]
    fn composite_article_labelled_with_instrument() {
        let m = map_of(&[("legalArticle", "code-civil|1240")]);
        let chips = decision_chips(&m, Some(&facets()));
        assert_eq!(chips[0].label, "Code civil · art. 1240");
        assert_eq!(chips[0].value, "code-civil|1240");
    }

    #[test]
    fn dates_merge_into_single_chip() {
        let m = map_of(&[
            ("dateFrom", "2020-01-01"),
            ("dateTo", "2021-06-30"),
            ("jurisdictionType", "TJ"),
        ]);
        let chips = decision_chips(&m, Some(&facets()));
        let dates: Vec<&ActiveChip> = chips.iter().filter(|c| c.key == "dates").collect();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0].label, "Du 2020-01-01 au 2021-06-30");
        // Borne seule.
        let m = map_of(&[("dateFrom", "2020-01-01")]);
        let chips = decision_chips(&m, None);
        assert_eq!(chips[0].label, "Depuis le 2020-01-01");
    }
}
