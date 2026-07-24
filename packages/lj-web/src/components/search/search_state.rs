//! Parsing de l'état URL en `SearchRequest` (port de `loaders.ts` +
//! `queries/search.ts`) et couche de fetch (leptos-fetch).
//!
//! `MAX_PAGES = 10`, `SEARCH_LIMIT = 10`, `offset = (page - 1) * limit`.
//! Les enums sont parsés en filtrant les valeurs invalides (parité
//! `parseEnumAll`). Module privé de la tranche (rattaché via `#[path]`).

use leptos_router::params::ParamsMap;
use lj_dtos::{
    Domain, JurisdictionType, Office, SearchMode, SearchRequest, Significance, Solution, SortOrder,
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
    /// Types de juridiction (`TJ`, `CE`, …) — racines `jurisdiction_type:*` du rail.
    pub jurisdiction_type: Vec<String>,
    /// Offices du juge (`JEX`, `JAF`, … — suffixes `office:*`), dropdown dédié.
    pub office: Vec<String>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, …).
    pub jurisdiction_code: Vec<String>,
    /// Chambres (catégorie contrôlée, suffixes `chamber:*`, ADR 0172).
    pub chamber: Vec<String>,
    /// Domaines de référence (suffixes `legal_domain:*`, racines ou feuilles).
    pub legal_domain: Vec<String>,
    /// Solutions (suffixes `solution:*`).
    pub solution: Vec<String>,
    pub legal_instrument: Vec<String>,
    pub legal_article: Vec<String>,
    /// Portées jurisprudentielles (suffixes `significance:*`, ADR 0167).
    pub significance: Vec<String>,
    /// Niveaux de publication (suffixes `publication:*`).
    pub publication: Vec<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
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
        jurisdiction_type: get_all(map, "jurisdictionType"),
        office: get_all(map, "office"),
        jurisdiction_code: get_all(map, "jurisdictionCode"),
        chamber: get_all(map, "chamber"),
        legal_domain: get_all(map, "legalDomain"),
        solution: get_all(map, "solution"),
        legal_instrument: get_all(map, "legalInstrument"),
        legal_article: get_all(map, "legalArticle"),
        significance: get_all(map, "significance"),
        publication: get_all(map, "publication"),
        date_from: date("dateFrom"),
        date_to: date("dateTo"),
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
        jurisdiction_type: parse_enum_all::<JurisdictionType>(&key.jurisdiction_type),
        solution: parse_enum_all::<Solution>(&key.solution),
        procedure: None,
        office: parse_enum_all::<Office>(&key.office),
        legal_domain: parse_enum_all::<Domain>(&key.legal_domain),
        jurisdiction_code: non_empty(&key.jurisdiction_code),
        chamber: non_empty(&key.chamber),
        legal_instrument: non_empty(&key.legal_instrument),
        legal_article: non_empty(&key.legal_article),
        significance: parse_enum_all::<Significance>(&key.significance),
        publication: non_empty(&key.publication),
        date_from: key.date_from.clone(),
        date_to: key.date_to.clone(),
        mode: SearchMode::Auto,
        sort,
        limit: SEARCH_LIMIT,
        offset,
        ai_mode: key.ai_mode,
    }
}
