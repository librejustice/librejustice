//! Jonctions de numéros de dossier lues dans le TEXTE (ADR 0157) : gabarits
//! auto-détectés par le scan — pivot « joint les pourvois » (cassation) et
//! clause « sous le(s) n° » de requête — regex sur petits spans positionnés
//! par tokens, jamais le plein-texte.

use lj_core::decision::Decision;
use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

// ────────────────────────── pourvois joints (pivot) ─────────────────────────

const JOINT_POURVOIS_MAX: usize = 3;

fn re_joint_pourvois() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        RegexBuilder::new(
            r"joint(?:es)?\s+les\s+pourvois\b([^;]*?)(?:;|\bSur\b|\bAttendu\b|\bVu\b|$)",
        )
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .unwrap()
    })
}
fn re_cc_pourvoi_num() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d{2}-\d{2}\.\d{2,3}\b").unwrap())
}
fn re_pourvoi_range() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bau\s+n[°o]").unwrap())
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// Pourvois joints (cassation). Le scan positionne l'ancre (« joint les
/// pourvois ») ; la regex de clause ne lit que la fenêtre qui suit.
pub(super) fn joined_pourvois(scan: Option<&crate::scan::DocScan>) -> Vec<String> {
    let windows = scan.map(|s| s.joint_pourvois_windows()).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for w in &windows {
        for caps in re_joint_pourvois().captures_iter(w) {
            let clause = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if re_pourvoi_range().is_match(clause) {
                continue;
            }
            for m in re_cc_pourvoi_num().find_iter(clause) {
                out.push(m.as_str().to_string());
            }
        }
    }
    let distinct = dedupe(out);
    if distinct.len() <= JOINT_POURVOIS_MAX {
        distinct
    } else {
        Vec::new()
    }
}

// ──────────────────── dossiers joints (clause de requête) ───────────────────

fn re_docket_context() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)sous\s+les?\s+(?:n[°ºo]s?|numéros?)\s+([^.;:\n]{1,160})").unwrap()
    })
}
fn re_docket_context_alt() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:requête|requêtes|pourvoi|pourvois|appel|appels)\b[^.;:\n]{0,100}?\bsous\s+les?\s+(?:n[°ºo]s?|numéros?)\s+([^.;:\n]{1,160})",
        )
        .unwrap()
    })
}
fn re_docket_citation_prefix() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:pourvu|pourvoi\w*)\s+en\s+cassation\s*$").unwrap())
}

fn same_court_docket_pattern(main_docket: &str) -> Option<Regex> {
    static RE_PA: OnceLock<Regex> = OnceLock::new();
    static RE_NUM: OnceLock<Regex> = OnceLock::new();
    // Motif dynamique mais espace minuscule (codes juridiction 2 lettres +
    // 3 longueurs numériques) : memoïsé — la compilation regex par document
    // pesait ~8 % du CPU d'extraction. `Regex` se clone par Arc.
    static CACHE: OnceLock<std::sync::RwLock<std::collections::HashMap<String, Regex>>> =
        OnceLock::new();
    let re_pa = RE_PA.get_or_init(|| Regex::new(r"^\d{2}[A-Z]{2}\d{4,6}$").unwrap());
    let re_num = RE_NUM.get_or_init(|| Regex::new(r"^\d{6,8}$").unwrap());
    let pattern = if re_pa.is_match(main_docket) {
        let code = &main_docket[2..4];
        format!(r"\b\d{{2}}{}\d{{4,6}}\b", regex::escape(code))
    } else if re_num.is_match(main_docket) {
        format!(r"\b\d{{{}}}\b", main_docket.len())
    } else {
        return None;
    };
    let cache = CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    if let Some(re) = cache.read().unwrap().get(&pattern) {
        return Some(re.clone());
    }
    let re = Regex::new(&pattern).ok()?;
    cache
        .write()
        .unwrap()
        .entry(pattern)
        .or_insert_with(|| re.clone());
    Some(re)
}

/// Dossiers joints à la même juridiction (clause « sous le(s) n° » d'une
/// requête/appel). Le scan positionne les ancres (`docket_context_windows`) ;
/// les regex de contexte ne lisent que ces fenêtres.
pub(super) fn joined_docket_numbers(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let main_docket = d.numero_dossier.as_deref().unwrap_or("").trim().to_string();
    if main_docket.is_empty() {
        return None;
    }
    let pattern = same_court_docket_pattern(&main_docket)?;
    let windows = scan.map(|s| s.docket_context_windows()).unwrap_or_default();
    if windows.is_empty() {
        return None;
    }
    let mut found = vec![main_docket.clone()];
    let mut seen = std::collections::HashSet::new();
    seen.insert(main_docket);

    for w in &windows {
        let mut caps_all: Vec<regex::Captures> = re_docket_context().captures_iter(w).collect();
        caps_all.extend(re_docket_context_alt().captures_iter(w));
        for caps in caps_all {
            let m = caps.get(0).unwrap();
            // Python : `text[max(0, start - 45) : start]` — 45 POINTS DE CODE
            // avant le match (pas 45 octets : trancher en octets casse sur
            // l'UTF-8 FR).
            let prefix_full = &w[..m.start()];
            let prefix = match prefix_full.char_indices().nth_back(44) {
                Some((idx, _)) => &prefix_full[idx..],
                None => prefix_full,
            };
            if re_docket_citation_prefix().is_match(prefix) {
                continue;
            }
            let group1 = caps.get(1).unwrap().as_str().to_uppercase();
            for dm in pattern.find_iter(&group1) {
                let docket = dm.as_str().to_string();
                if seen.insert(docket.clone()) {
                    found.push(docket);
                }
            }
        }
    }
    Some(found)
}
