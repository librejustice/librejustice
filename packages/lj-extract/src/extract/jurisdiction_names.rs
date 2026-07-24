//! Nom de juridiction : table `location` Judilibre (+ renommage TAE lu dans
//! l'EN-TÊTE, zone par tokens du scan — ADR 0157) et réécriture canonique des
//! noms de greffe administratifs (TA/CAA/CE), dont les conventions nourrissent
//! l'identité de dédoublonnage ([`crate::identity`]) et la facette juridiction
//! ([`crate::facets::city_from_name`]).

use lj_core::decision::Decision;
use regex::Regex;
use std::sync::OnceLock;

// ─────────────────────── location Judilibre → nom ───────────────────────────

include!("jurisdiction_locations.rs");

fn location_to_name(loc: &str) -> Option<&'static str> {
    location_table()
        .iter()
        .find(|(k, _)| *k == loc)
        .map(|(_, v)| *v)
}

/// Nom depuis la métadonnée `location` Judilibre ; un tribunal de commerce
/// dont l'en-tête annonce le « tribunal des activités économiques » est
/// renommé (expérimentation TAE). Repli : nom de greffe verbatim.
pub(super) fn from_location(decision: &Decision, header: Option<&str>) -> Option<String> {
    if let Some(loc) = &decision.jurisdiction_location {
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
    decision.jurisdiction_name.clone().filter(|s| !s.is_empty())
}

// ──────────────── réécriture canonique des noms admin ───────────────────────

fn lower_particles() -> &'static std::collections::HashSet<&'static str> {
    static S: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        [
            "de", "du", "d", "en", "le", "la", "les", "sur", "sous", "et", "au", "aux",
        ]
        .into_iter()
        .collect()
    })
}

/// `_title_place`.
fn title_place(place: &str) -> String {
    if place.is_empty()
        || !place.chars().any(|c| c.is_uppercase())
        || place.chars().any(|c| c.is_lowercase())
    {
        return place.to_string();
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"([ \-])").unwrap());
    // split en gardant les séparateurs (comme Python re.split avec groupe).
    let mut parts: Vec<String> = Vec::new();
    let mut last = 0;
    for m in re.find_iter(place) {
        parts.push(place[last..m.start()].to_string());
        parts.push(m.as_str().to_string());
        last = m.end();
    }
    parts.push(place[last..].to_string());

    let mut first_word = true;
    let mut result = String::new();
    for part in parts {
        if part == " " || part == "-" {
            result.push_str(&part);
        } else {
            let word = capitalize(&part);
            if !first_word && lower_particles().contains(word.to_lowercase().as_str()) {
                result.push_str(&word.to_lowercase());
            } else {
                result.push_str(&word);
            }
            first_word = false;
        }
    }
    result
}

/// Python `str.capitalize` : 1ère lettre maj, reste min.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Nom canonique d'une juridiction administrative : « Conseil d'État »
/// constant, TA/CAA réécrits depuis le nom de greffe (casse de la ville,
/// forme longue de « CAA de X »).
pub(super) fn admin_name(d: &Decision) -> Option<String> {
    let raw = d.jurisdiction_name.as_ref().map(|s| s.trim().to_string());
    if d.jurisdiction_type.as_deref() == Some("CE") {
        return Some("Conseil d'État".to_string());
    }
    let raw = raw.filter(|s| !s.is_empty())?;

    static RE_SP: OnceLock<Regex> = OnceLock::new();
    let compact = RE_SP
        .get_or_init(|| Regex::new(r"\s+").unwrap())
        .replace_all(&raw, " ")
        .to_string();
    let lower = compact.to_lowercase();

    if lower.starts_with("tribunal administratif ") {
        let place = compact["Tribunal Administratif ".len()..]
            .trim()
            .to_string();
        let place_lower = place.to_lowercase();
        if place_lower.starts_with("de ") {
            let tail = place[3..].trim();
            if tail.to_lowercase().starts_with("d ") {
                return Some(
                    format!("Tribunal administratif d'{}", title_place(tail[2..].trim()))
                        .trim()
                        .to_string(),
                );
            }
            return Some(
                format!("Tribunal administratif de {}", title_place(tail))
                    .trim()
                    .to_string(),
            );
        }
        if place_lower.starts_with("d'") {
            return Some(
                format!(
                    "Tribunal administratif d'{}",
                    title_place(place[2..].trim())
                )
                .trim()
                .to_string(),
            );
        }
        // Apostrophe perdue par le greffe (« Tribunal Administratif d Amiens »).
        if place_lower.starts_with("d ") {
            return Some(
                format!(
                    "Tribunal administratif d'{}",
                    title_place(place[2..].trim())
                )
                .trim()
                .to_string(),
            );
        }
        if !place.is_empty() {
            let city = title_place(&place);
            let first = city
                .chars()
                .next()
                .map(|c| c.to_lowercase().next().unwrap());
            let article = match first {
                Some('a') | Some('e') | Some('i') | Some('o') | Some('u') | Some('y')
                | Some('h') => "d'",
                _ => "de ",
            };
            return Some(format!("Tribunal administratif {article}{city}"));
        }
        return Some("Tribunal administratif".to_string());
    }

    if lower.starts_with("tribunal administratif") {
        return Some(compact.replacen("Tribunal Administratif", "Tribunal administratif", 1));
    }

    if lower.starts_with("cour administrative d'appel") {
        let prefix_len = "Cour administrative d'appel".len();
        let suffix = compact[prefix_len..].trim();
        let suffix_lower = suffix.to_lowercase();
        if suffix_lower.starts_with("de ") {
            return Some(
                format!(
                    "Cour administrative d'appel de {}",
                    title_place(suffix[3..].trim())
                )
                .trim()
                .to_string(),
            );
        }
        return Some(
            format!("Cour administrative d'appel {}", title_place(suffix))
                .trim()
                .to_string(),
        );
    }

    static RE_CAA: OnceLock<Regex> = OnceLock::new();
    let re_caa = RE_CAA.get_or_init(|| Regex::new(r"(?i)^CAA\s+de\s+(.+)$").unwrap());
    if let Some(c) = re_caa.captures(&compact) {
        return Some(
            format!(
                "Cour administrative d'appel de {}",
                title_place(c[1].trim())
            )
            .trim()
            .to_string(),
        );
    }

    Some(compact)
}
