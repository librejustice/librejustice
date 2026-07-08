//! Extracteurs Judilibre (ordre judiciaire CC/CA/TJ/TCOM).
//!
//! Port fidèle de `extract/judilibre.py`. Module PUR : aucun I/O.
//!
//! Le crate `regex` ne supporte pas les lookaround : les patterns Python qui en
//! utilisent sont compilés sans l'assertion puis revérifiés côté Rust (cf.
//! helpers `*_match` dans `judilibre_outcome.rs`). Les helpers partagés avec
//! l'ordre administratif (`extract/common.py`) sont réutilisés via
//! [`super::common`].

use super::common;
use lj_core::decision::Decision;
use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

// ───────────────────────────── helpers regex ───────────────────────────────

fn ci(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("static judilibre regex must compile")
}

fn ci_dotall(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .expect("static judilibre regex must compile")
}

fn cs(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static judilibre regex must compile")
}

/// `s[:1].upper() + s[1:]` (Python).
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
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

// ───────────────────────── chamber_label (vocab inline) ─────────────────────

/// Port de `judilibre_vocab.chamber_label` (table CC inline ; le module parsing
/// Rust n'expose pas encore ce helper).
fn chamber_label(code: Option<&str>) -> Option<String> {
    let code = code?;
    let label = match code.to_lowercase().as_str() {
        "civ1" => "Première chambre civile",
        "civ2" => "Deuxième chambre civile",
        "civ3" => "Troisième chambre civile",
        "soc" => "Chambre sociale",
        "comm" => "Chambre commerciale",
        "cr" => "Chambre criminelle",
        "mi" => "Chambre mixte",
        "pl" => "Assemblée plénière",
        "ord" | "ordo" => "Ordonnance du Premier président",
        "creun" => "Chambre réunies",
        "allciv" => "Toutes chambres civiles",
        "other" => "Autre formation",
        _ => return Some(code.to_string()),
    };
    Some(label.to_string())
}

// ─────────────────────── location → nom de juridiction ──────────────────────

include!("judilibre_locations.rs");

fn location_to_name(loc: &str) -> Option<&'static str> {
    location_table()
        .iter()
        .find(|(k, _)| *k == loc)
        .map(|(_, v)| *v)
}

// ──────────────────────────── regex cache global ────────────────────────────

include!("judilibre_patterns.rs");

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(build_patterns)
}

// ───────────────────────────── docket numbers ───────────────────────────────

const JOINT_POURVOIS_MAX: usize = 3;

/// Port de `_joined_pourvois`. Le scan positionne l'ancre (« joint les
/// pourvois ») ; la regex de clause ne lit que la fenêtre qui suit —
/// petit span positionné par token, ADR 0157.
fn joined_pourvois(scan: Option<&crate::scan::DocScan>) -> Vec<String> {
    let p = patterns();
    let windows = scan.map(|s| s.joint_pourvois_windows()).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for w in &windows {
        for caps in p.joint_pourvois.captures_iter(w) {
            let clause = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if p.pourvoi_range.is_match(clause) {
                continue;
            }
            for m in p.cc_pourvoi_num.find_iter(clause) {
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

pub(super) fn extract_docket_numbers(
    decision: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    // Python : list(numero_dossiers or [numero_dossier]) — quand `numero_dossiers`
    // est vide/None, la liste vaut `[numero_dossier]` (qui peut contenir un None
    // ignoré ensuite par clean_docket_numbers).
    let mut base: Vec<Option<String>> = match &decision.numero_dossiers {
        Some(v) if !v.is_empty() => v.iter().cloned().map(Some).collect(),
        _ => vec![decision.numero_dossier.clone()],
    };
    if decision.juridiction_type.as_deref() == Some("CC") {
        base.extend(joined_pourvois(scan).into_iter().map(Some));
    }
    common::clean_docket_numbers(Some(&base))
}

// ──────────────────────────────── dates ─────────────────────────────────────

pub(super) fn extract_date_lecture(decision: &Decision) -> Option<String> {
    common::clean_date_iso(decision.date_lecture.as_deref())
}

pub(super) fn extract_date_audience(
    decision: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<String> {
    let value = match decision.date_audience.as_deref() {
        Some(d) if !d.is_empty() => Some(d.to_string()),
        _ => common::extract_textual_audience_date(decision, scan),
    };
    common::clean_date_iso(value.as_deref())
}

// ─────────────────────────── formation / chambre ────────────────────────────

const CHAMBER_ACRONYMS: &[&str] = &["JAF", "JLD", "JEX", "JME", "JAP", "JI"];
const CHAMBER_STOPWORDS: &[&str] = &[
    "de", "du", "des", "la", "le", "les", "et", "en", "sur", "au", "aux", "par", "pour", "ou",
    "ni", "un", "une",
];

// Stopwords d'acte bornant le nom de chambre (cf. `_CHAMBER_STOP`,
// judilibre.py:367). Le regex `body_named_chamber` les exige en BORNE
// (trailing `\s+(?:STOP)\b`) mais — le crate `regex` n'ayant pas de lookahead
// — n'embarque PAS le lookahead négatif interne `(?!(?:STOP)\b)` qui, côté
// Python, empêche un stopword d'être avalé comme MOT du libellé. On reproduit
// ce lookahead a posteriori sur la capture brute via `trim_named_chamber`.
const CHAMBER_BODY_STOP: &[&str] = &[
    "arret",
    "arrêt",
    "audience",
    "ordonnance",
    "jugement",
    "du",
    "le",
    "no",
    "n°",
];

fn is_body_stop(tok: &str) -> bool {
    let low = tok.to_lowercase();
    CHAMBER_BODY_STOP.iter().any(|s| *s == low)
}

/// Reproduit le lookahead négatif interne de `_RE_BODY_NAMED_CHAMBER` :
/// la capture greedy `chambre(?: connecteur)?(?:\s+\w+){1,3}` (sans lookahead)
/// peut avaler un stopword d'acte comme mot du libellé ; Python ne le ferait
/// jamais (le lookahead `(?!(?:STOP)\b)` borne la liste de mots au premier
/// stopword). On rejoue donc : `chambre` + connecteur optionnel (`des|du|de
/// la|d'`) + 1..3 mots arrêtés au premier stopword. Si zéro mot valide ne
/// suit (premier mot = stopword), Python renvoie None ({1,3} exige >= 1 mot).
fn trim_named_chamber(label: &str) -> Option<String> {
    let toks: Vec<&str> = label.split(' ').collect();
    let mut out: Vec<&str> = vec![toks[0]]; // "chambre"
    let mut i = 1;
    // Connecteur optionnel : des | du | d' | « de la ».
    if i < toks.len() {
        let low = toks[i].to_lowercase();
        if low == "des" || low == "du" || low == "d'" || low == "d\u{2019}" {
            out.push(toks[i]);
            i += 1;
        } else if low == "de" && i + 1 < toks.len() && toks[i + 1].to_lowercase() == "la" {
            out.push(toks[i]);
            out.push(toks[i + 1]);
            i += 2;
        }
    }
    // 1..3 mots, bornés au premier stopword (lookahead Python).
    let mut nwords = 0;
    while i < toks.len() && nwords < 3 {
        if is_body_stop(toks[i]) {
            break;
        }
        out.push(toks[i]);
        nwords += 1;
        i += 1;
    }
    if nwords < 1 {
        return None;
    }
    Some(out.join(" "))
}

/// Chambre lue dans le BANDEAU d'en-tête (zone par tokens du scan, ADR 0157
/// — remplace le `head(1500)`) : regex sur petit span positionné.
pub(super) fn chamber_from_body(bandeau: &str) -> Option<String> {
    let p = patterns();
    if let Some(c) = p.body_pole.captures(bandeau) {
        return Some(format!("Pôle {} - Chambre {}", &c[1], &c[2]));
    }
    if p.body_conseil.is_match(bandeau) {
        return Some("Chambre du conseil".to_string());
    }
    if let Some(c) = p.body_named_chamber.captures(bandeau) {
        return trim_named_chamber(&c[1]);
    }
    None
}

/// Port de `_is_acronym_token`.
fn is_acronym_token(tok: &str) -> bool {
    let p = patterns();
    if p.chamber_code_token.is_match(tok) || CHAMBER_ACRONYMS.contains(&tok.to_uppercase().as_str())
    {
        return true;
    }
    let letters: Vec<char> = tok.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() || tok.chars().any(|c| c.is_lowercase()) {
        return false;
    }
    let joined: String = letters.iter().collect::<String>().to_lowercase();
    if CHAMBER_STOPWORDS.contains(&joined.as_str()) {
        return false;
    }
    tok.contains('.') || letters.len() == 1 || letters.len() <= 4
}

/// Port de `_humanize_free_chamber`.
fn humanize_free_chamber(label: &str) -> String {
    let p = patterns();
    let trimmed = label.trim();
    if let Some(c) = p.pole_chambre.captures(trimmed) {
        return format!("Pôle {} - Chambre {}", &c[1], &c[2]);
    }
    let tokens: Vec<String> = label
        .split(' ')
        .map(|t| {
            if is_acronym_token(t) {
                t.to_uppercase()
            } else {
                t.to_lowercase()
            }
        })
        .collect();
    capitalize_first(&tokens.join(" "))
}

pub(super) fn extract_formation_or_chamber(
    decision: &Decision,
    bandeau: Option<&str>,
) -> Option<String> {
    let p = patterns();
    let mut chamber = chamber_label(decision.juridiction_code.as_deref());
    if chamber.as_deref().unwrap_or("").is_empty()
        && decision.juridiction_type.as_deref() != Some("CC")
    {
        chamber = bandeau.and_then(chamber_from_body);
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = chamber {
        if !c.is_empty() {
            parts.push(c);
        }
    }
    if let Some(f) = &decision.formation {
        if !f.is_empty() {
            parts.push(f.clone());
        }
    }
    if parts.is_empty() {
        return None;
    }
    if decision.juridiction_type.as_deref() != Some("CC") {
        parts = parts
            .iter()
            .map(|pp| humanize_free_chamber(&p.chamber_prononce_cut.replace(pp, "")))
            .collect();
    }
    Some(parts.join(" — "))
}

pub(super) fn extract_jurisdiction_name(
    decision: &Decision,
    header: Option<&str>,
) -> Option<String> {
    if let Some(loc) = &decision.juridiction_location {
        if let Some(name) = location_to_name(loc) {
            // sigle TAE dans l'EN-TÊTE (zone par tokens, plié — remplace le
            // `head(800)` + regex ; le titre suit les mentions de greffe)
            let tae = header.is_some_and(|b| {
                crate::compiled::fold_stable(b).contains("tribunal des activites economiques")
            });
            if name.starts_with("Tribunal de commerce") && tae {
                return Some(name.replacen(
                    "Tribunal de commerce",
                    "Tribunal des activités économiques",
                    1,
                ));
            }
            return Some(name.to_string());
        }
    }
    decision.juridiction_nom.clone().filter(|s| !s.is_empty())
}

// ──────────────────────────── publication code ──────────────────────────────

pub(super) fn extract_publication_code(decision: &Decision) -> Option<String> {
    if decision.publication_codes.is_empty() {
        None
    } else {
        Some(decision.publication_codes.join(","))
    }
}

#[cfg(test)]
mod tests {
    include!("judilibre_tests.rs");
}
