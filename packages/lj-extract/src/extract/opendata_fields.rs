// Champs simples opendata (dates, juridiction, instance, formation, docket…).
// Inclus dans `opendata.rs` (même module).
// Regexes survivantes = chaînes de MÉTADONNÉE greffe (splits, renames ancrés
// `^…$`) ou petits spans du scan (`docket_context_windows`) — audit ADR 0157.

/// `extract_docket_numbers`.
pub fn extract_docket_numbers(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    if let Some(joined) = extract_joined_docket_numbers(d, scan) {
        if !joined.is_empty() {
            let opt: Vec<Option<String>> = joined.into_iter().map(Some).collect();
            return clean_docket_numbers(Some(&opt));
        }
    }
    // Python : `if not decision.numero_dossier: return None` (None ou chaîne vide).
    let nd = d.numero_dossier.as_deref().filter(|s| !s.is_empty())?;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[,;\s]+").unwrap());
    let parts: Vec<Option<String>> = re.split(nd.trim()).map(|s| Some(s.to_string())).collect();
    clean_docket_numbers(Some(&parts))
}

/// `extract_date_lecture`.
pub fn extract_date_lecture(d: &Decision) -> Option<String> {
    clean_date_iso(d.date_lecture.as_deref())
}

/// `extract_date_audience`.
pub fn extract_date_audience(d: &Decision, scan: Option<&crate::scan::DocScan>) -> Option<String> {
    let v = d
        .date_audience
        .clone()
        .or_else(|| extract_textual_audience_date(d, scan));
    clean_date_iso(v.as_deref())
}

/// `extract_formation_or_chamber`.
pub fn extract_formation_or_chamber(d: &Decision) -> Option<String> {
    Some(normalize_formation(d.formation.as_deref()))
}

/// `extract_publication_code`.
pub fn extract_publication_code(d: &Decision) -> Option<String> {
    d.publication_codes.first().cloned()
}

// ---------------------------------------------------------------------------
// jurisdiction name + place title-casing
// ---------------------------------------------------------------------------

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

/// `extract_jurisdiction_name`.
pub fn extract_jurisdiction_name(d: &Decision) -> Option<String> {
    let raw = d.juridiction_nom.as_ref().map(|s| s.trim().to_string());
    if d.juridiction_type.as_deref() == Some("CE") {
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

// ---------------------------------------------------------------------------
// docket numbers joints
// ---------------------------------------------------------------------------

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

fn extract_joined_docket_numbers(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let main_docket = d.numero_dossier.as_deref().unwrap_or("").trim().to_string();
    if main_docket.is_empty() {
        return None;
    }
    let pattern = same_court_docket_pattern(&main_docket)?;
    // Le scan positionne les ancres « sous le(s) n°/numéro(s) » (AdminReq) ;
    // les regex de contexte ne lisent que ces fenêtres — petits spans
    // positionnés par tokens, ADR 0157 (plus de sections ni de plein-texte).
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

// ---------------------------------------------------------------------------
// formation
// ---------------------------------------------------------------------------

fn formation_acronyms() -> &'static std::collections::HashSet<&'static str> {
    static S: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        [
            "JU", "J.U", "OQTF", "DALO", "MESD", "MW", "CH", "JLD", "JCP", "DACA", "RG",
        ]
        .into_iter()
        .collect()
    })
}

/// `_normalize_formation`.
fn normalize_formation(raw: Option<&str>) -> String {
    let raw = match raw {
        Some(r) if !r.is_empty() => r,
        _ => return "INCONNU".to_string(),
    };
    static RE_SP: OnceLock<Regex> = OnceLock::new();
    static RE_PUNCT: OnceLock<Regex> = OnceLock::new();
    static RE_R222: OnceLock<Regex> = OnceLock::new();
    let mut compact = RE_SP
        .get_or_init(|| Regex::new(r"\s+").unwrap())
        .replace_all(raw, " ")
        .trim()
        .to_string();
    compact = RE_PUNCT
        .get_or_init(|| Regex::new(r"\s+([,;:])").unwrap())
        .replace_all(&compact, "$1")
        .to_string();
    compact = RE_R222
        .get_or_init(|| Regex::new(r"(?i)\s*-\s*R\.?\s*222[-\u{2013}]13$").unwrap())
        .replace(&compact, "")
        .to_string();
    compact = formation_case(&compact);
    if compact.is_empty() {
        "INCONNU".to_string()
    } else {
        compact
    }
}

/// `_formation_case`.
fn formation_case(s: &str) -> String {
    let words: Vec<&str> = s.split(' ').collect();
    let mut out: Vec<String> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if w.is_empty() {
            out.push(String::new());
            continue;
        }
        let stripped = w
            .trim_matches(['(', ')', '[', ']', ',', ';', ':', '.'])
            .to_uppercase();
        if formation_acronyms().contains(stripped.as_str()) {
            out.push(w.to_string());
        } else if i == 0 && w.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
            let mut chars = w.chars();
            let first = chars.next().unwrap();
            out.push(first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase());
        } else {
            out.push(w.to_lowercase());
        }
    }
    out.join(" ")
}
