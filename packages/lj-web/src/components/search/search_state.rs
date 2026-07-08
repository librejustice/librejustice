//! Parsing de l'état URL en `SearchRequest` (port de `loaders.ts` +
//! `queries/search.ts`) et couche de fetch (leptos-fetch).
//!
//! `MAX_PAGES = 10`, `SEARCH_LIMIT = 10`, `offset = (page - 1) * limit`.
//! Les enums sont parsés en filtrant les valeurs invalides (parité
//! `parseEnumAll`). Module privé de la tranche (rattaché via `#[path]`).

use leptos_router::params::ParamsMap;
use lj_dtos::{
    Domaine, JuridictionType, Office, Portee, SearchMode, SearchRequest, Solution, SortOrder,
};

/// Cap de pagination (parité `MAX_PAGES`).
pub const MAX_PAGES: u32 = 10;
/// Taille de page (parité `SEARCH_LIMIT`).
pub const SEARCH_LIMIT: u32 = 10;

/// Clé de cache stable (parité `searchKeys.list`) : query + filtres + page. Sert
/// de clé leptos-fetch ; deux navigations vers la même recherche partagent
/// l'entrée.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchKey {
    pub query: String,
    pub page: u32,
    /// Types de juridiction (`TJ`, `CE`, …) — racines `juridiction:*` du rail.
    pub jur: Vec<String>,
    /// Offices du juge (`JEX`, `JAF`, … — suffixes `office:*`), dropdown dédié.
    pub office: Vec<String>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, …).
    pub jcode: Vec<String>,
    /// Domaines de référence (suffixes `domaine:*`, racines ou feuilles).
    pub domaine: Vec<String>,
    /// Solutions (suffixes `solution:*`).
    pub solution: Vec<String>,
    pub li: Vec<String>,
    pub la: Vec<String>,
    /// Portées jurisprudentielles (suffixes `portee:*`, ADR 0167).
    pub portee: Vec<String>,
    /// Niveaux de publication (suffixes `publication:*`).
    pub publication: Vec<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub sort: String,
    pub ai_mode: bool,
}

/// `true` si `YYYY-MM-DD` (parité `_isoDate`).
fn is_iso_date(v: &str) -> bool {
    let b = v.as_bytes();
    v.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn get_all(map: &ParamsMap, key: &str) -> Vec<String> {
    map.get_all(key).unwrap_or_default()
}

/// Page 1-based, défaut 1, cap `MAX_PAGES` (parité loader).
pub fn parse_page(map: &ParamsMap) -> u32 {
    let raw = map.get("page").and_then(|s| s.parse::<u32>().ok());
    match raw {
        Some(p) if p >= 1 => p.min(MAX_PAGES),
        _ => 1,
    }
}

/// Construit la clé de recherche depuis le query map.
pub fn key_from_map(map: &ParamsMap) -> SearchKey {
    let date = |k: &str| map.get(k).filter(|v| is_iso_date(v));
    SearchKey {
        query: map
            .get("q")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        page: parse_page(map),
        jur: get_all(map, "jur"),
        office: get_all(map, "office"),
        jcode: get_all(map, "jcode"),
        domaine: get_all(map, "domaine"),
        solution: get_all(map, "solution"),
        li: get_all(map, "li"),
        la: get_all(map, "la"),
        portee: get_all(map, "portee"),
        publication: get_all(map, "publication"),
        from: date("from"),
        to: date("to"),
        sort: parse_sort(map.get("sort").as_deref()),
        ai_mode: super::ai_mode::is_ai_mode_param(map.get("aiMode").as_deref()),
    }
}

/// `relevance` | `date_desc` | `date_asc` (défaut `relevance`). Port de `parseSort`.
fn parse_sort(raw: Option<&str>) -> String {
    match raw {
        Some("date_desc") => "date_desc".to_string(),
        Some("date_asc") => "date_asc".to_string(),
        _ => "relevance".to_string(),
    }
}

fn parse_enum_all<T: serde::de::DeserializeOwned>(raws: &[String]) -> Option<Vec<T>> {
    let parsed: Vec<T> = raws
        .iter()
        .filter_map(|r| serde_json::from_value::<T>(serde_json::Value::String(r.clone())).ok())
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn non_empty(v: &[String]) -> Option<Vec<String>> {
    if v.is_empty() {
        None
    } else {
        Some(v.to_vec())
    }
}

/// Construit le `SearchRequest` (limit/offset + filtres). Port de
/// `searchQueryOptions`.
pub fn request_from_key(key: &SearchKey) -> SearchRequest {
    let offset = (key.page - 1) * SEARCH_LIMIT;
    let sort = match key.sort.as_str() {
        "date_desc" => SortOrder::DateDesc,
        "date_asc" => SortOrder::DateAsc,
        _ => SortOrder::Relevance,
    };
    SearchRequest {
        query: key.query.clone(),
        juridiction_type: parse_enum_all::<JuridictionType>(&key.jur),
        solution: parse_enum_all::<Solution>(&key.solution),
        voie: None,
        office: parse_enum_all::<Office>(&key.office),
        legal_domain: parse_enum_all::<Domaine>(&key.domaine),
        jurisdiction_code: non_empty(&key.jcode),
        legal_instrument: non_empty(&key.li),
        legal_article: non_empty(&key.la),
        portee: parse_enum_all::<Portee>(&key.portee),
        publication: non_empty(&key.publication),
        date_from: key.from.clone(),
        date_to: key.to.clone(),
        mode: SearchMode::Auto,
        sort,
        limit: SEARCH_LIMIT,
        offset,
        ai_mode: key.ai_mode,
    }
}
