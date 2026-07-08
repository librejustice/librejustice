//! Nettoyage des dockets, dates ISO, et extraction de la date d'audience
//! depuis le corps. Port de la partie « Dockets & dates » de `extract/common.py`.

use std::collections::HashSet;
use std::sync::LazyLock;

use jiff::civil::Date;
use jiff::Unit;
use regex::Regex;

use lj_core::decision::Decision;

const DOCKET_MAX_LEN: usize = 32;
const DOCKET_MIN_LEN: usize = 4;
const DATE_MIN_YEAR: i32 = 1800;
const DATE_MAX_YEAR: i32 = 2200;

/// `_clean_date_iso`.
pub(crate) fn clean_date_iso(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    let bytes = value.as_bytes();
    if value.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year_str = &value[..4];
    if !year_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = year_str.parse().ok()?;
    if !(DATE_MIN_YEAR..=DATE_MAX_YEAR).contains(&year) {
        return None;
    }
    Some(value.to_string())
}

/// `_clean_docket_numbers`.
pub(crate) fn clean_docket_numbers(values: Option<&[Option<String>]>) -> Option<Vec<String>> {
    let values = values?;
    if values.is_empty() {
        return None;
    }
    let mut cleaned: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in values {
        let Some(raw) = raw else { continue };
        let compact = raw.trim();
        let len = compact.chars().count();
        if !(DOCKET_MIN_LEN..=DOCKET_MAX_LEN).contains(&len) {
            continue;
        }
        let key = compact.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        cleaned.push(compact.to_string());
    }
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// `_FRENCH_MONTHS`.
fn french_month(name: &str) -> Option<i8> {
    Some(match name {
        "janvier" => 1,
        "février" | "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" | "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "décembre" | "decembre" => 12,
        _ => return None,
    })
}

/// `_parse_french_date`.
pub(crate) fn parse_french_date(day: &str, month: &str, year: &str) -> Option<String> {
    let month_num = french_month(&month.to_lowercase())?;
    let day_num: i8 = if day.to_lowercase() == "1er" {
        1
    } else {
        day.parse().ok()?
    };
    let year_num: i16 = year.parse().ok()?;
    Date::new(year_num, month_num, day_num)
        .ok()
        .map(|d| d.to_string())
}

/// `_parse_numeric_date`.
fn parse_numeric_date(day: &str, month: &str, year: &str) -> Option<String> {
    let d: i8 = day.parse().ok()?;
    let m: i8 = month.parse().ok()?;
    let y: i16 = year.parse().ok()?;
    Date::new(y, m, d).ok().map(|date| date.to_string())
}

// Regex d'audience (toutes IGNORECASE) — petits spans positionnés par tokens :
// elles ne lisent que les fenêtres `DocScan::audience_windows`, jamais le
// texte intégral (ADR 0157).
static RE_AUDIENCE_COMPOSITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)audience\s+(?:publique\s+)?du\s+(\d{1,2}|1er)\s+([a-zéûîôàèùç]+)\s+(\d{4})[\s,]*(?:où\s+étaient\s+présent|où\s+siégeaient|en\s+présence\s+de)",
    )
    .unwrap()
});
static RE_AUDIENCE_LABELED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:d[ée]bats?\s+en\s+(?:l['\x{2019}]\s*)?audience\s+publique|plaidoiries\s+tenues\s+en\s+(?:audience\s+publique|chambre\s+du\s+conseil)|d[ée]bats?\s*,?\s+qui\s+se\s+sont\s+déroulé)\s*(?:du|le)?\s*:?\s*(\d{1,2}|1er)\s+([a-zéûîôàèùç]+)\s+(\d{4})",
    )
    .unwrap()
});
// `_RE_AUDIENCE_DEBATS` sans le lookbehind `(?<!délibéré\s)` (porté ci-dessous).
static RE_AUDIENCE_DEBATS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:d[ée]bat[s]?\s+en\s+l['\x{2019}]audience\s+publique|au\s+cours\s+de\s+l['\x{2019}]audience\s+publique|lors\s+des\s+d[ée]bat[s]?|(?:à|a)\s+l['\x{2019}]audience(?:\s+publique)?(?:\s+tenue)?|lors\s+de\s+l['\x{2019}]audience(?:\s+publique)?(?:\s+tenue)?|audience\s+(?:collégiale|des\s+référés|sur\s+incident)|délibéré\s+après\s+l['\x{2019}]audience|l['\x{2019}]affaire\s+a\s+été\s+appelée|l['\x{2019}]affaire\s+(?:a\s+été\s+)?plaidée|(?:l['\x{2019}]affaire\s+a\s+été\s+)?débattue?)\s+(?:du|le|en\s+date\s+du)\s+(\d{1,2}|1er)\s+([a-zéûîôàèùç]+)\s+(\d{4})",
    )
    .unwrap()
});
static RE_AUDIENCE_BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)audience\s+publique(?:,?\s+qui\s+s['\x{2019}]est\s+tenue)?\s+du\s+(\d{1,2}|1er)\s+([a-zéûîôàèùç]+)\s+(\d{4})",
    )
    .unwrap()
});
static RE_AUDIENCE_DEBATS_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:lors\s+des\s+d[ée]bats?|date\s+des\s+d[ée]bats?|d[ée]battue?)\s*(?:du|le|en\s+date\s+du|:)?\s*(\d{1,2})[/.](\d{1,2})[/.](\d{4})",
    )
    .unwrap()
});
static RE_AUDIENCE_COMPOSITION_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)l['\x{2019}]audience\s+(?:publique\s+)?(?:en\s+chambre\s+du\s+conseil\s+)?du\s*(\d{1,2})[/.](\d{1,2})[/.](\d{4})\s*(?:,?\s*où\s+siégeaient|,?\s*où\s+étaient|\s+et\s+même\s+composition)",
    )
    .unwrap()
});
static RE_AUDIENCE_DEBATS_CHAMBRE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)d[ée]bats?\s+(?:à\s+)?l['\x{2019}]audience(?:\s+(?:de\s+|en\s+)?chambre\s+du\s+conseil|\s+publique)?\s+du\s*(\d{1,2})[/.](\d{1,2})[/.](\d{4})",
    )
    .unwrap()
});

fn audience_candidates(text: &str, re: &Regex) -> Vec<String> {
    re.captures_iter(text)
        .filter_map(|c| parse_french_date(&c[1], &c[2], &c[3]))
        .collect()
}

/// Comme [`audience_candidates`] mais écarte les matches précédés de
/// « délibéré » (port du lookbehind `(?<!délibéré\s)`, qui ne porte que sur la
/// branche `(?:à|a) l'audience`).
fn audience_candidates_debats(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in RE_AUDIENCE_DEBATS.captures_iter(text) {
        let whole = m.get(0).unwrap();
        let matched = whole.as_str().to_lowercase();
        let is_a_laudience = matched.starts_with("à l'")
            || matched.starts_with("a l'")
            || matched.starts_with("à l\u{2019}")
            || matched.starts_with("a l\u{2019}");
        if is_a_laudience {
            let prefix = &text[..whole.start()];
            if prefix.to_lowercase().ends_with("délibéré ") {
                continue;
            }
        }
        if let Some(d) = parse_french_date(&m[1], &m[2], &m[3]) {
            out.push(d);
        }
    }
    out
}

fn audience_candidates_num(text: &str, re: &Regex) -> Vec<String> {
    re.captures_iter(text)
        .filter_map(|c| parse_numeric_date(&c[1], &c[2], &c[3]))
        .collect()
}

/// `_extract_textual_audience_date` — les regex de date ne tournent plus sur
/// le texte intégral : le scan (ADR 0156/0157) positionne les ancres
/// (« audience », « débats », « plaidoiries », « débattue », « appelée ») et
/// chaque regex ne voit que les petites fenêtres verbatim autour de ces
/// tokens. Priorité inchangée : premier producteur non vide, dans l'ordre.
pub(crate) fn extract_textual_audience_date(
    decision: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<String> {
    let windows = scan.map(|s| s.audience_windows()).unwrap_or_default();
    if windows.is_empty() {
        return None;
    }
    type Producer<'a> = &'a dyn Fn(&str) -> Vec<String>;
    let producers: [Producer; 7] = [
        &|w| audience_candidates(w, &RE_AUDIENCE_COMPOSITION),
        &|w| audience_candidates(w, &RE_AUDIENCE_LABELED),
        &audience_candidates_debats,
        &|w| audience_candidates(w, &RE_AUDIENCE_BARE),
        &|w| audience_candidates_num(w, &RE_AUDIENCE_DEBATS_NUM),
        &|w| audience_candidates_num(w, &RE_AUDIENCE_COMPOSITION_NUM),
        &|w| audience_candidates_num(w, &RE_AUDIENCE_DEBATS_CHAMBRE_NUM),
    ];
    let candidates: Vec<String> = producers
        .iter()
        .map(|produce| windows.iter().flat_map(|w| produce(w)).collect::<Vec<_>>())
        .find(|c| !c.is_empty())
        .unwrap_or_default();

    if candidates.is_empty() {
        return None;
    }
    let Some(lecture_str) = decision.date_lecture.as_deref() else {
        return candidates.last().cloned();
    };
    let lecture = Date::strptime("%Y-%m-%d", lecture_str).ok()?;
    let parsed: Vec<Date> = candidates
        .iter()
        .filter_map(|c| Date::strptime("%Y-%m-%d", c).ok())
        .collect();

    let nearby: Vec<Date> = parsed
        .iter()
        .copied()
        .filter(|c| {
            *c <= lecture
                && lecture
                    .since((Unit::Day, *c))
                    .map(|s| i64::from(s.get_days()) <= 120)
                    .unwrap_or(false)
        })
        .collect();
    if let Some(max) = nearby.iter().max() {
        return Some(max.to_string());
    }

    let repaired: Vec<Date> = parsed
        .iter()
        .filter(|c| {
            c.year() == lecture.year() - 1
                && c.month() == lecture.month()
                && (i64::from(c.day()) - i64::from(lecture.day())).abs() <= 7
        })
        .filter_map(|c| Date::new(lecture.year(), c.month(), c.day()).ok())
        .collect();
    if let Some(max) = repaired.iter().max() {
        return Some(max.to_string());
    }

    let past: Vec<Date> = parsed.iter().copied().filter(|c| *c <= lecture).collect();
    if let Some(max) = past.iter().max() {
        return Some(max.to_string());
    }
    parsed.last().map(|d| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_date_iso_validates_range_and_shape() {
        assert_eq!(
            clean_date_iso(Some("2021-03-14")),
            Some("2021-03-14".to_string())
        );
        assert_eq!(clean_date_iso(Some("0201-11-23")), None); // année < 1800
        assert_eq!(clean_date_iso(Some("2021/03/14")), None); // séparateur
        assert_eq!(clean_date_iso(None), None);
    }

    #[test]
    fn clean_docket_numbers_filters_length_and_dedupes() {
        let vals = vec![
            Some("23PA00123".to_string()),
            Some("23pa00123".to_string()), // doublon casefold
            Some("ab".to_string()),        // trop court
            None,
        ];
        assert_eq!(
            clean_docket_numbers(Some(&vals)),
            Some(vec!["23PA00123".to_string()])
        );
        assert_eq!(clean_docket_numbers(None), None);
    }

    /// Décision minimale (texte + dates) pour exercer l'extraction d'audience.
    fn audience_decision(text: &str, lecture: Option<&str>) -> lj_core::decision::Decision {
        lj_core::decision::Decision {
            source_uid: "t".into(),
            member_name: "t".into(),
            ecli: None,
            juridiction_code: None,
            juridiction_nom: None,
            juridiction_type: Some("CC".into()),
            juridiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: lecture.map(str::to_string),
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: vec![],
            avocat_requerant: None,
            texte_integral_raw: text.into(),
            texte_integral_clean: text.into(),
            sections: vec![],
            metadata_header: String::new(),
            visa_trim: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }

    #[test]
    fn textual_audience_date_priority_and_selection() {
        // Spec figée + garde-fou du refacto paresseux de
        // `extract_textual_audience_date` (l'ordre des producteurs et le « premier
        // non vide » doivent rester identiques quelle que soit l'évaluation).

        // Producteur 0 (RE_AUDIENCE_COMPOSITION) : « audience publique du … où
        // étaient présents ». Date d'audience <= lecture et proche (< 120 j).
        let d = audience_decision(
            "Vu la procédure. À l'audience publique du 12 mars 2024 où étaient présents \
             les magistrats. Par ces motifs.",
            Some("2024-03-20"),
        );
        assert_eq!(
            extract_textual_audience_date(&d, crate::extract::scan_doc(&d).as_ref()).as_deref(),
            Some("2024-03-12")
        );

        // Aucun motif d'audience → None (le cas fréquent qui doit court-circuiter).
        let none = audience_decision(
            "Vu la requête. Par ces motifs, rejette.",
            Some("2024-03-20"),
        );
        assert_eq!(
            extract_textual_audience_date(&none, crate::extract::scan_doc(&none).as_ref()),
            None
        );

        // Forme « débats … à l'audience publique du … » (producteur 2, via
        // RE_AUDIENCE_DEBATS) : doit être captée et sélectionnée.
        let debats = audience_decision(
            "Les débats se sont tenus à l'audience publique du 3 octobre 2023.",
            Some("2023-11-10"),
        );
        assert_eq!(
            extract_textual_audience_date(&debats, crate::extract::scan_doc(&debats).as_ref())
                .as_deref(),
            Some("2023-10-03")
        );
    }
}
