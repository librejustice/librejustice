//! Helpers de composition + formatters d'affichage.
//!
//! - `cn()` : port de `lib/utils.ts` (concat de classes Tailwind statiques).
//! - formatters : port de `lib/format.ts` (juridiction, score, comptes, dates
//!   FR). Logique pure (pas d'I/O) ; les libelles longs viennent des enums DTO.

use lj_dtos::JuridictionType;

/// Concatene des fragments de classes Tailwind : trim, drop des vides, join sur
/// un espace.
///
/// On NE porte PAS `tailwind-merge` : les classes des templates lj-web sont
/// statiques (pas de conflit a deduper a l'execution). Ajouter un dedup serait
/// de la complexite gratuite (regle repo #11). Si une variante conditionnelle
/// introduit un jour un vrai conflit utility, le resoudre a la source.
pub fn cn<'a, I: IntoIterator<Item = &'a str>>(classes: I) -> String {
    classes
        .into_iter()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sigle court d'un ordre de juridiction (port de `JURIDICTION_SHORT`).
pub fn juridiction_short(jur: JuridictionType) -> &'static str {
    match jur {
        JuridictionType::Ta => "TA",
        JuridictionType::Caa => "CAA",
        JuridictionType::Ce => "CE",
        JuridictionType::Constit => "Cons. const.",
        JuridictionType::Tc => "TC",
        JuridictionType::Cc => "CC",
        JuridictionType::Ca => "CA",
        JuridictionType::Tj => "TJ",
        JuridictionType::Tcom => "TCOM",
        JuridictionType::Cedh => "CEDH",
        JuridictionType::Cjue => "CJUE",
        JuridictionType::Cnda => "CNDA",
    }
}

/// Libelle long d'un ordre de juridiction (port de `JURIDICTION_TYPE_LABELS` /
/// `formatJuridiction`).
pub fn format_juridiction(jur: JuridictionType) -> &'static str {
    match jur {
        JuridictionType::Ta => "Tribunal administratif",
        JuridictionType::Caa => "Cour administrative d'appel",
        JuridictionType::Ce => "Conseil d'État",
        JuridictionType::Constit => "Conseil constitutionnel",
        JuridictionType::Tc => "Tribunal des conflits",
        JuridictionType::Cc => "Cour de cassation",
        JuridictionType::Ca => "Cour d'appel",
        JuridictionType::Tj => "Tribunal judiciaire",
        JuridictionType::Tcom => "Tribunal de commerce",
        JuridictionType::Cedh => "Cour européenne des droits de l'homme",
        JuridictionType::Cjue => "Cour de justice de l'Union européenne",
        JuridictionType::Cnda => "Cour nationale du droit d'asile",
    }
}

/// Nom de juridiction nettoye, ou `None` si vide apres normalisation.
/// Port de `sanitizeJurisdictionName` (suppression chambre / juge des referes).
fn sanitize_jurisdiction_name(value: Option<&str>) -> Option<String> {
    let normalized = value.map(str::trim).filter(|s| !s.is_empty())?;
    // `\s+,` -> `,` : recolle une virgule precedee d'espaces.
    let mut out = String::with_capacity(normalized.len());
    let chars: Vec<char> = normalized.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            // Lookahead : sequence d'espaces suivie d'une virgule -> on saute les
            // espaces et on n'emet que la virgule.
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == ',' {
                out.push(',');
                i = j + 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    // Coupe `, <Nre> chambre…` et `, juge des référés…` en fin de chaine.
    let cut = strip_trailing_chamber(&out);
    let cut = strip_trailing_refere(cut);
    let trimmed = cut.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Retire un suffixe `, [N(re|ère|e)] chambre…` (insensible a la casse), porte
/// de la regex `/,\s*(\d{1,2}(?:re|ère|e)?\s+chambre.*)$/i`.
fn strip_trailing_chamber(s: &str) -> &str {
    if let Some(comma) = s.rfind(',') {
        let tail = s[comma + 1..].trim_start();
        let lower = tail.to_lowercase();
        // Doit commencer par 1-2 chiffres puis (optionnel) re|ère|e, espaces, "chambre".
        if matches_chamber(&lower) {
            return s[..comma].trim_end();
        }
    }
    s
}

fn matches_chamber(lower: &str) -> bool {
    let bytes: Vec<char> = lower.chars().collect();
    let mut i = 0;
    let mut digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() && digits < 2 {
        i += 1;
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    for suffix in ["re", "ère", "e"] {
        if lower[byte_offset(&bytes, i)..].starts_with(suffix) {
            i += suffix.chars().count();
            break;
        }
    }
    let rest = &lower[byte_offset(&bytes, i)..];
    let rest = rest.trim_start();
    rest.starts_with("chambre")
}

fn byte_offset(chars: &[char], char_idx: usize) -> usize {
    chars[..char_idx].iter().map(|c| c.len_utf8()).sum()
}

/// Retire un suffixe `, juge des référés…`, porte de
/// `/,\s*(juge des référés.*)$/i`.
fn strip_trailing_refere(s: &str) -> &str {
    if let Some(comma) = s.rfind(',') {
        let tail = s[comma + 1..].trim_start().to_lowercase();
        if tail.starts_with("juge des référés") {
            return s[..comma].trim_end();
        }
    }
    s
}

/// Nom affichable pour une decision : nom de juridiction nettoye, sinon libelle
/// long du type. Port de `formatDecisionJurisdiction`.
pub fn format_decision_jurisdiction(jur: JuridictionType, name: Option<&str>) -> String {
    sanitize_jurisdiction_name(name).unwrap_or_else(|| format_juridiction(jur).to_string())
}

/// Variante courte : nom nettoye avec abreviations TA/CAA/CE en tete, sinon le
/// sigle. Port de `formatShortDecisionJurisdiction`.
pub fn format_short_decision_jurisdiction(jur: JuridictionType, name: Option<&str>) -> String {
    match sanitize_jurisdiction_name(name) {
        None => juridiction_short(jur).to_string(),
        Some(name) => {
            let lower = name.to_lowercase();
            if lower.starts_with("tribunal administratif") {
                format!("TA{}", &name["Tribunal administratif".len()..])
            } else if lower.starts_with("cour administrative d'appel") {
                format!("CAA{}", &name["Cour administrative d'appel".len()..])
            } else if lower.starts_with("conseil d'état") {
                format!("CE{}", &name["Conseil d'État".len()..])
            } else {
                name
            }
        }
    }
}

/// Score formate a 3 decimales. Port de `formatScore`.
pub fn format_score(score: f64) -> String {
    format!("{score:.3}")
}

/// Plafond d'affichage du nombre de resultats (port de `RESULTS_CAP`).
const RESULTS_CAP: i64 = 400;

/// Libelle FR du nombre de resultats. Port de `formatResultsCount`.
pub fn format_results_count(total: i64) -> String {
    match total {
        0 => "Aucun résultat".to_string(),
        1 => "1 résultat".to_string(),
        n if n > RESULTS_CAP => format!("{RESULTS_CAP}+ résultats"),
        n => format!("{} résultats", group_thousands(n)),
    }
}

/// Insere une espace insecable fine tous les 3 chiffres (rendu `toLocaleString
/// ("fr-FR")` : separateur U+202F).
pub fn group_thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Nombre de pages de pagination (plafonne par `RESULTS_CAP` et `max_pages`).
/// Port de `totalPages`.
pub fn total_pages(total: i64, limit: i64, max_pages: i64) -> i64 {
    let capped = total.min(RESULTS_CAP);
    // ceil(capped / limit) sans `div_ceil` (instable sur cette toolchain).
    let pages = (capped + limit - 1) / limit;
    pages.min(max_pages)
}

/// Noms de mois FR longs (rendu `toLocaleDateString("fr-FR", { month: "long" })`).
const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// Numéro d'article en graphie française : la forme compacte LEGI colle le
/// préfixe de partie au numéral (« L1142-1 », « R123-4 », « D45 »), la
/// convention des juristes l'écrit « L. 1142-1 ». Seul un préfixe lettre
/// unique L/R/D/A directement suivi d'un chiffre est réécrit — les autres
/// formes (numéraux nus, « LO119 », étoilés « R*011 », libellés « Annexe V »)
/// passent telles quelles.
pub fn format_article_num(num: &str) -> String {
    let mut chars = num.chars();
    match (chars.next(), chars.next()) {
        (Some(p @ ('L' | 'R' | 'D' | 'A')), Some(d)) if d.is_ascii_digit() => {
            format!("{p}. {}", &num[1..])
        }
        _ => num.to_string(),
    }
}

/// Formate une date ISO `YYYY-MM-DD` en `j mois aaaa` (FR). Port de
/// `formatIsoDate` : `None`/vide -> `—` ; valeur non parsable -> renvoyee telle
/// quelle.
pub fn format_iso_date(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return "—".to_string();
    };
    match parse_iso_date(value) {
        Some((y, m, d)) => format!("{d} {} {y}", MONTHS_FR[(m - 1) as usize]),
        None => value.to_string(),
    }
}

/// Parse `YYYY-MM-DD` en `(année, mois 1-12, jour 1-31)` ; `None` si invalide.
fn parse_iso_date(value: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if (1..=12).contains(&m) && (1..=31).contains(&d) {
        Some((y, m, d))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::format_article_num;

    #[test]
    fn format_article_num_dots_single_letter_prefixes_only() {
        assert_eq!(format_article_num("L1142-1"), "L. 1142-1");
        assert_eq!(format_article_num("R123-4"), "R. 123-4");
        assert_eq!(format_article_num("D45"), "D. 45");
        assert_eq!(format_article_num("A412-1"), "A. 412-1");
        // Formes hors périmètre : inchangées.
        assert_eq!(format_article_num("1240"), "1240");
        assert_eq!(format_article_num("LO119"), "LO119");
        assert_eq!(format_article_num("R*011"), "R*011");
        assert_eq!(format_article_num("LP. 974-2"), "LP. 974-2");
        assert_eq!(format_article_num("Annexe V"), "Annexe V");
    }
}
