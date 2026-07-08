//! Mutation de l'état URL de recherche (port des règles `URLSearchParams` des
//! composants React). Toutes les mutations partent du `ParamsMap` courant (lu
//! via `use_query_map`), appliquent un changement, et renvoient la *query
//! string sans le `?` initial* prête pour `navigate("/recherche?{qs}")`.
//!
//! Invariants (parité loaders.ts / filter-rail / sort-select) :
//! - toute mutation de filtre/tri supprime `page` (retour page 1) ;
//! - `relevance` ⇒ `sort` supprimé ; `aiMode=false` ⇒ `aiMode` supprimé ;
//! - les autres clés sont conservées telles quelles (multi-valeurs incluses).
//!
//! NB : module privé de la tranche, rattaché via `#[path]` depuis un composant
//! (le `mod.rs` figé de `search/` n'expose que les composants).

use leptos_router::params::ParamsMap;

/// Route de la recherche de décisions.
pub const SEARCH_PATH: &str = "/recherche";
/// Route de la recherche de textes (page distincte : moteurs et filtres
/// propres, aucune recherche transverse).
pub const TEXTES_PATH: &str = "/textes";

/// Chemin de recherche courant : `/textes` si on y est déjà (les mutations et
/// soumissions y restent), sinon `/recherche`. Lu sur `window.location` — les
/// appels ne surviennent que dans des handlers client (clic, submit).
#[cfg(feature = "hydrate")]
fn current_search_path() -> &'static str {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .filter(|p| p == TEXTES_PATH)
        .map(|_| TEXTES_PATH)
        .unwrap_or(SEARCH_PATH)
}

#[cfg(not(feature = "hydrate"))]
fn current_search_path() -> &'static str {
    SEARCH_PATH
}

/// Vrai quand la barre vit sur `/textes` (corpus articles). Lecture
/// `window.location` : les deux pages de recherche sont des îlots client
/// (`ClientOnly`), et un changement de route les remonte.
pub fn on_textes_path() -> bool {
    current_search_path() == TEXTES_PATH
}

/// Href complet de la recherche pour une query string (sans `?` de tête).
pub fn search_href(qs: &str) -> String {
    let path = current_search_path();
    if qs.is_empty() {
        path.to_string()
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
/// drop de `page`. Port de `set/delete` + `delete("page")`.
pub fn with_param(map: &ParamsMap, key: &str, value: Option<&str>, drop_page: bool) -> String {
    mutate(map, drop_page, |next| {
        next.remove(key);
        if let Some(v) = value {
            next.insert(key.to_string(), v.to_string());
        }
    })
}

/// Bascule une valeur dans une clé multi-valeur (présente ⇒ retirée, absente ⇒
/// ajoutée). Drop `page`. Port de `toggle(key, value)`.
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

/// Réécrit les bornes de dates (`from`/`to`) en une passe. Drop `page`. Port du
/// `onChange` du DateRangePicker.
pub fn with_dates(map: &ParamsMap, from: Option<&str>, to: Option<&str>) -> String {
    mutate(map, true, |next| {
        next.remove("from");
        next.remove("to");
        if let Some(f) = from.filter(|s| !s.is_empty()) {
            next.insert("from".to_string(), f.to_string());
        }
        if let Some(t) = to.filter(|s| !s.is_empty()) {
            next.insert("to".to_string(), t.to_string());
        }
    })
}

/// Supprime un ensemble de clés (conserve le reste). Pas de drop `page`
/// supplémentaire (parité `resetAll`). Port de `resetAll`.
pub fn without_keys(map: &ParamsMap, keys: &[&str]) -> String {
    mutate(map, false, |next| {
        for key in keys {
            next.remove(key);
        }
    })
}

/// Construit la query string d'une soumission de requête : `q` (trim), drop
/// `page`, `aiMode` selon le toggle. Conserve filtres + sort existants. Port du
/// submit de CompactSearch.
pub fn with_query(map: &ParamsMap, query: &str, ai_mode: bool) -> String {
    mutate(map, true, |next| {
        next.replace("q".to_string(), query.to_string());
        next.remove("aiMode");
        if ai_mode {
            next.insert("aiMode".to_string(), "true".to_string());
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
        let qs = toggle_multi(&m, "jur", "TA");
        assert!(qs.contains("jur=TA"));
        assert!(qs.contains("q=x"));
        // Re-toggle retire la valeur.
        let m2 = map_of(&[("q", "x"), ("jur", "TA")]);
        let qs2 = toggle_multi(&m2, "jur", "TA");
        assert!(!qs2.contains("jur=TA"));
    }

    #[test]
    fn toggle_drops_page() {
        let m = map_of(&[("q", "x"), ("page", "3")]);
        let qs = toggle_multi(&m, "jur", "TA");
        assert!(!qs.contains("page="));
    }

    #[test]
    fn relevance_clears_sort() {
        let m = map_of(&[("q", "x"), ("sort", "date_desc")]);
        let qs = with_param(&m, "sort", None, true);
        assert!(!qs.contains("sort="));
    }
}
