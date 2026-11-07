//! Parsing de l'état URL en `SearchRequest` et couche de fetch (leptos-fetch).
//!
//! `offset = (page - 1) * SEARCH_PAGE_SIZE`, la forme de pagination venant de
//! `helpers`, partagée avec les surfaces textes et annonces.
//! Les enums sont parsés en filtrant les valeurs invalides. Module privé de la
//! tranche (rattaché via `#[path]`).

use leptos_router::params::ParamsMap;

use crate::helpers::{SEARCH_MAX_PAGES, SEARCH_PAGE_SIZE};
use lj_dtos::{
    DecisionPart, Domain, JurisdictionType, SearchMode, SearchRequest, Solution, SortOrder,
};

/// Clé de cache stable : query + filtres + page. Sert
/// de clé leptos-fetch ; deux navigations vers la même recherche partagent
/// l'entrée.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchKey {
    pub query: String,
    pub page: u32,
    /// Types de juridiction (`TJ`, `CE`, …) — racines `jurisdiction_type:*` du rail.
    pub jurisdiction_type: Vec<String>,
    /// Codes du référentiel `jurisdiction` (`tj76351`, `ca_paris`, …).
    pub jurisdiction_code: Vec<String>,
    /// Formations, en paires `<cour>|<clé>` (`ca_paris|C3`).
    pub seat: Vec<String>,
    /// Formation paths (`FR.TJ.JEX`).
    pub formation: Vec<String>,
    /// Domaines du référentiel de place (suffixes `legal_domain:*`, racines ou feuilles).
    pub legal_domain: Vec<String>,
    /// Solutions (suffixes `solution:*`).
    pub solution: Vec<String>,
    pub legal_instrument: Vec<String>,
    pub legal_article: Vec<String>,
    /// Niveaux de publication (suffixes `publication:*`).
    pub publication: Vec<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// The parts searched (`headnote`, `judgment`, `annexes`), every part when empty.
    pub search_in: Vec<String>,
    pub sort: String,
    /// Mode demandé : `auto` (défaut) laisse le résolveur lire la requête.
    pub mode: String,
    pub ai_mode: bool,
}

impl SearchKey {
    /// What the count depends on: the question, its filters, and whether the order
    /// is by date, which lists every decision holding all the words.
    pub fn count_key(&self) -> Self {
        let sort = match self.sort.as_str() {
            "relevance" => parse_sort(None),
            _ => parse_sort(Some("date_desc")),
        };
        Self {
            page: 1,
            sort,
            ai_mode: false,
            ..self.clone()
        }
    }
}

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

/// Page 1-based, défaut 1, cap `MAX_PAGES`.
pub fn parse_page(map: &ParamsMap) -> u32 {
    let raw = map.get("page").and_then(|s| s.parse::<u32>().ok());
    match raw {
        Some(p) if p >= 1 => p.min(SEARCH_MAX_PAGES as u32),
        _ => 1,
    }
}

pub fn key_from_map(map: &ParamsMap) -> SearchKey {
    let date = |k: &str| map.get(k).filter(|v| is_iso_date(v));
    SearchKey {
        query: map
            .get("q")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        page: parse_page(map),
        jurisdiction_type: get_all(map, "jurisdictionType"),
        jurisdiction_code: get_all(map, "jurisdictionCode"),
        seat: get_all(map, "seat"),
        formation: get_all(map, "formation"),
        legal_domain: get_all(map, "legalDomain"),
        solution: get_all(map, "solution"),
        legal_instrument: get_all(map, "legalInstrument"),
        legal_article: get_all(map, "legalArticle"),
        publication: get_all(map, "publication"),
        date_from: date("dateFrom"),
        date_to: date("dateTo"),
        search_in: get_all(map, "searchIn"),
        sort: parse_sort(map.get("sort").as_deref()),
        mode: parse_mode(map.get("mode").as_deref()),
        ai_mode: super::ai_mode::ai_mode_from_param(map.get("aiMode").as_deref()),
    }
}

/// `auto` | `lexical` | `semantic` | `reference` (défaut `auto`).
pub fn parse_mode(raw: Option<&str>) -> String {
    match raw {
        Some("lexical") => "lexical".to_string(),
        Some("semantic") => "semantic".to_string(),
        Some("reference") => "reference".to_string(),
        _ => "auto".to_string(),
    }
}

/// `relevance` | `date_desc` | `date_asc` (défaut `relevance`).
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

pub fn request_from_key(key: &SearchKey) -> SearchRequest {
    let offset = (key.page - 1) * SEARCH_PAGE_SIZE;
    let sort = match key.sort.as_str() {
        "date_desc" => SortOrder::DateDesc,
        "date_asc" => SortOrder::DateAsc,
        _ => SortOrder::Relevance,
    };
    SearchRequest {
        query: key.query.clone(),
        jurisdiction_type: parse_enum_all::<JurisdictionType>(&key.jurisdiction_type),
        solution: parse_enum_all::<Solution>(&key.solution),
        legal_domain: parse_enum_all::<Domain>(&key.legal_domain),
        jurisdiction_code: non_empty(&key.jurisdiction_code),
        seat: non_empty(&key.seat),
        formation: non_empty(&key.formation),
        legal_instrument: non_empty(&key.legal_instrument),
        legal_article: non_empty(&key.legal_article),
        publication: non_empty(&key.publication),
        date_from: key.date_from.clone(),
        date_to: key.date_to.clone(),
        search_in: parse_enum_all::<DecisionPart>(&key.search_in),
        mode: match key.mode.as_str() {
            "lexical" => SearchMode::Lexical,
            "semantic" => SearchMode::Semantic,
            "reference" => SearchMode::Reference,
            _ => SearchMode::Auto,
        },
        sort,
        limit: SEARCH_PAGE_SIZE,
        offset,
        ai_mode: key.ai_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_key_follows_the_set_not_the_page() {
        let page1 = key_from_map(&ParamsMap::new());
        let mut page3 = page1.clone();
        page3.page = 3;
        page3.ai_mode = true;
        assert_ne!(page1, page3);
        assert_eq!(page1.count_key(), page3.count_key());

        let mut recent = page1.clone();
        recent.sort = "date_desc".to_string();
        let mut oldest = page1.clone();
        oldest.sort = "date_asc".to_string();
        assert_ne!(page1.count_key(), recent.count_key());
        assert_eq!(recent.count_key(), oldest.count_key());

        let mut filtre = page1.clone();
        filtre.solution = vec!["solution:cassation".to_string()];
        assert_ne!(page1.count_key(), filtre.count_key());
    }
}
