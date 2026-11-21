//! Mutation of the search URL state. Every mutation starts from the current
//! `ParamsMap` (read through `use_query_map`), applies one change, and returns the
//! *query string without the leading `?`* ready for [`search_href`].
//!
//! Invariants:
//! - every filter/sort mutation removes `page` (back to page 1);
//! - `relevance` ⇒ `sort` removed; `aiMode=false` ⇒ `aiMode` removed;
//! - the other keys are kept as is (multi-values included).
//!
//! Private module of the slice, attached through `#[path]` from a component (the
//! frozen `mod.rs` of `search/` only exposes the components).

use leptos_router::params::ParamsMap;

use crate::i18n::SearchUi;

/// The decision search, under a jurisdiction.
pub const SEARCH_PATH: &str = "/decision-search";
/// The text search, under a jurisdiction: its own engines and filters, no cross search.
pub const TEXTES_PATH: &str = "/norm-search";
/// Route du moteur d'annonces (pot des événements en cours).
pub const JOURNALS_PATH: &str = "/journal-search";

/// Le corpus que la route interroge. Trois pages de recherche, trois corpus
/// disjoints : la barre, le mode IA et l'autocomplétion en dépendent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corpus {
    Decisions,
    Textes,
    Journals,
}

impl Corpus {
    /// Mode d'autocomplétion, vide quand le corpus n'en a pas.
    pub fn suggest_mode(self) -> &'static str {
        match self {
            Corpus::Decisions => "jurisprudence",
            Corpus::Textes => "textes",
            // Le pot n'a pas d'index de suggestions : ses entrées sont des
            // facettes, pas des mots-clés.
            Corpus::Journals => "",
        }
    }

    pub fn placeholder(self, ui: &SearchUi) -> &'static str {
        match self {
            Corpus::Decisions => ui.placeholder_decisions,
            Corpus::Textes => ui.placeholder_texts,
            Corpus::Journals => ui.placeholder_notices,
        }
    }
}

pub fn corpus() -> Corpus {
    let path = current_search_path();
    if path.ends_with(TEXTES_PATH) {
        Corpus::Textes
    } else if path.ends_with(JOURNALS_PATH) {
        Corpus::Journals
    } else {
        Corpus::Decisions
    }
}

/// The search page on screen, its jurisdiction included: mutations and submissions
/// stay there. Read from `window.location`, the calls only happen in client handlers.
#[cfg(feature = "hydrate")]
fn current_search_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default()
}

#[cfg(not(feature = "hydrate"))]
fn current_search_path() -> String {
    SEARCH_PATH.to_string()
}

/// Href complet de la recherche pour une query string (sans `?` de tête).
pub fn search_href(qs: &str) -> String {
    let path = current_search_path();
    if qs.is_empty() {
        path
    } else {
        format!("{path}?{qs}")
    }
}

/// Clone le map, applique `f` (mutation libre), supprime `page`, et renvoie la
/// query string (sans `?`).
fn mutate(map: &ParamsMap, drop_page: bool, f: impl FnOnce(&mut ParamsMap)) -> String {
    let mut next = map.clone();
    f(&mut next);
    if drop_page {
        next.remove("page");
    }
    strip_leading_q(next.to_query_string())
}

/// `to_query_string` renvoie `"?a=b"` ou `""` ; on retire le `?` de tête.
fn strip_leading_q(qs: String) -> String {
    qs.strip_prefix('?').map(str::to_string).unwrap_or(qs)
}

/// Réécrit une clé mono-valeur : `Some` ⇒ remplace, `None` ⇒ supprime. Optionnel
/// drop de `page`.
pub fn with_param(map: &ParamsMap, key: &str, value: Option<&str>, drop_page: bool) -> String {
    mutate(map, drop_page, |next| {
        next.remove(key);
        if let Some(v) = value {
            next.insert(key.to_string(), v.to_string());
        }
    })
}

/// Rewrites a multi-valued key with `values`, none removing it. Drops `page`.
pub fn with_values(map: &ParamsMap, key: &str, values: &[String]) -> String {
    mutate(map, true, |next| {
        next.remove(key);
        for v in values {
            next.insert(key.to_string(), v.clone());
        }
    })
}

/// Bascule une valeur dans une clé multi-valeur (présente ⇒ retirée, absente ⇒
/// ajoutée). Drop `page`.
pub fn toggle_multi(map: &ParamsMap, key: &str, value: &str) -> String {
    mutate(map, true, |next| {
        let current = next.remove(key).unwrap_or_default();
        let kept: Vec<String> = if current.iter().any(|v| v == value) {
            current.into_iter().filter(|v| v != value).collect()
        } else {
            let mut v = current;
            v.push(value.to_string());
            v
        };
        for v in kept {
            next.insert(key.to_string(), v);
        }
    })
}

/// The parts a search runs in, by URL value.
pub const PARTS: [&str; 4] = ["headnote", "judgment", "annexes", "conclusions"];

/// A decision's parts, what a search runs in when `searchIn` lists none.
pub const DECISION_PARTS: [&str; 3] = ["headnote", "judgment", "annexes"];

/// A part's label, and the hint that explains it when it needs one.
pub fn part_copy(part: &str, ui: &SearchUi) -> (&'static str, Option<&'static str>) {
    match part {
        "headnote" => (ui.part_headnote, Some(ui.part_headnote_hint)),
        "judgment" => (ui.part_judgment, None),
        "annexes" => (ui.part_annexes, None),
        _ => (ui.part_conclusions, Some(ui.part_conclusions_hint)),
    }
}

/// The parts searched: the ones `searchIn` lists, a decision's parts when it
/// lists none.
pub fn parts_searched(map: &ParamsMap) -> Vec<&'static str> {
    let listed = map.get_all("searchIn").unwrap_or_default();
    let kept: Vec<&'static str> = PARTS
        .iter()
        .copied()
        .filter(|v| listed.iter().any(|l| l == v))
        .collect();
    if kept.is_empty() {
        DECISION_PARTS.to_vec()
    } else {
        kept
    }
}

/// Toggles one part. The last part searched stays, and `searchIn` goes once
/// a decision's parts alone are searched again. Drops `page`.
pub fn toggle_part(map: &ParamsMap, value: &str) -> String {
    let searched = parts_searched(map);
    let next: Vec<String> = PARTS
        .iter()
        .copied()
        .filter(|v| (*v == value) != searched.contains(v))
        .map(str::to_string)
        .collect();
    if next.is_empty() {
        mutate(map, false, |_| {})
    } else if next == DECISION_PARTS {
        with_values(map, "searchIn", &[])
    } else {
        with_values(map, "searchIn", &next)
    }
}

/// Réécrit les bornes de dates (`from`/`to`) en une passe. Drop `page`. Port du
/// `onChange` du DateRangePicker.
pub fn with_dates(map: &ParamsMap, from: Option<&str>, to: Option<&str>) -> String {
    mutate(map, true, |next| {
        next.remove("dateFrom");
        next.remove("dateTo");
        if let Some(f) = from.filter(|s| !s.is_empty()) {
            next.insert("dateFrom".to_string(), f.to_string());
        }
        if let Some(t) = to.filter(|s| !s.is_empty()) {
            next.insert("dateTo".to_string(), t.to_string());
        }
    })
}

/// Supprime un ensemble de clés (conserve le reste). Pas de drop `page`
/// supplémentaire.
pub fn without_keys(map: &ParamsMap, keys: &[&str]) -> String {
    mutate(map, false, |next| {
        for key in keys {
            next.remove(key);
        }
    })
}

/// Construit la query string d'une soumission de requête : `q` (trim), drop
/// `page`, `aiMode` selon le toggle. Conserve filtres + sort existants. Une
/// requête vide **retire** `q` : la barre vidée sur un corpus qui l'accepte
/// revient à l'état sans requête.
///
/// `ai_mode` : `None` pour un corpus sans mode IA (textes) — aucun param posé ;
/// `Some(true)` = défaut, param retiré ; `Some(false)` = OFF explicite, seul cas
/// écrit dans l'URL.
pub fn with_query(map: &ParamsMap, query: &str, ai_mode: Option<bool>) -> String {
    mutate(map, true, |next| {
        next.remove("q");
        if !query.is_empty() {
            next.insert("q".to_string(), query.to_string());
        }
        next.remove("aiMode");
        if ai_mode == Some(false) {
            next.insert("aiMode".to_string(), "false".to_string());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> ParamsMap {
        let mut m = ParamsMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        m
    }

    #[test]
    fn toggle_adds_then_removes() {
        let m = map_of(&[("q", "x")]);
        let qs = toggle_multi(&m, "jurisdictionType", "TA");
        assert!(qs.contains("jurisdictionType=TA"));
        assert!(qs.contains("q=x"));
        // Re-toggle retire la valeur.
        let m2 = map_of(&[("q", "x"), ("jurisdictionType", "TA")]);
        let qs2 = toggle_multi(&m2, "jurisdictionType", "TA");
        assert!(!qs2.contains("jurisdictionType=TA"));
    }

    #[test]
    fn toggle_drops_page() {
        let m = map_of(&[("q", "x"), ("page", "3")]);
        let qs = toggle_multi(&m, "jurisdictionType", "TA");
        assert!(!qs.contains("page="));
    }

    #[test]
    fn une_requete_vide_retire_le_parametre() {
        let m = map_of(&[("q", "voirie"), ("fond", "marche")]);
        let qs = with_query(&m, "", None);
        assert!(!qs.contains("q="));
        assert!(qs.contains("fond=marche"));
    }

    #[test]
    fn relevance_clears_sort() {
        let m = map_of(&[("q", "x"), ("sort", "date_desc")]);
        let qs = with_param(&m, "sort", None, true);
        assert!(!qs.contains("sort="));
    }

    #[test]
    fn parts_are_toggled_from_every_part_and_keep_one() {
        let parts: Vec<lj_dtos::DecisionPart> = PARTS
            .iter()
            .map(|v| serde_json::from_value(serde_json::json!(v)).unwrap())
            .collect();
        assert_eq!(parts, lj_dtos::DecisionPart::ALL);

        let all = map_of(&[("q", "x"), ("page", "3")]);
        assert_eq!(
            toggle_part(&all, "annexes"),
            "q=x&searchIn=headnote&searchIn=judgment"
        );
        let judgment = map_of(&[("q", "x"), ("searchIn", "judgment")]);
        assert_eq!(toggle_part(&judgment, "judgment"), "q=x&searchIn=judgment");
        assert_eq!(
            toggle_part(&judgment, "headnote"),
            "q=x&searchIn=headnote&searchIn=judgment"
        );
        let two = map_of(&[
            ("q", "x"),
            ("searchIn", "headnote"),
            ("searchIn", "judgment"),
        ]);
        assert_eq!(toggle_part(&two, "annexes"), "q=x");

        assert_eq!(
            toggle_part(&all, "conclusions"),
            "q=x&searchIn=headnote&searchIn=judgment&searchIn=annexes&searchIn=conclusions"
        );
        let alone = map_of(&[("q", "x"), ("searchIn", "conclusions")]);
        assert_eq!(
            toggle_part(&alone, "conclusions"),
            "q=x&searchIn=conclusions"
        );
        let four = map_of(&[
            ("q", "x"),
            ("searchIn", "headnote"),
            ("searchIn", "judgment"),
            ("searchIn", "annexes"),
            ("searchIn", "conclusions"),
        ]);
        assert_eq!(toggle_part(&four, "conclusions"), "q=x");
    }
}
