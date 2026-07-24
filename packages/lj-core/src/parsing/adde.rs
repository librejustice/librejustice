//! Parse pur d'un titre de ressource « jurisprudence » de l'ADDE
//! (Avocats pour la défense des droits des étrangers, `adde-association.org`).
//!
//! Le titre d'un post ADDE de la catégorie *jurisprudence* **est** la citation
//! de la décision commentée, sous une forme régulière :
//!
//! - `CE, 5 février 2026, n°499141`
//! - `CAA Paris, 30 janvier 2026, n°24PA04236, 24PA04614 et 25PA00741`
//! - `Cass. civ 1ère, 7 janvier 2026, 24-15449 et 24-15450`
//! - `TA Strasbourg, 2 décembre 2025, n°2509454`
//!
//! On en extrait (juridiction, date de lecture, numéros de dossier normalisés)
//! pour rattacher le commentaire à la ou aux décisions par (dossier, date) —
//! le numéro seul n'est pas unique (un `2509454` de TA existe à plusieurs
//! dates). Aucun I/O : entrée = le titre, sortie = la citation structurée.

use serde_json::{json, Value};
use std::sync::LazyLock;

use regex::Regex;

/// Citation extraite d'un titre ADDE.
#[derive(Debug, Clone, PartialEq)]
pub struct AddeCitation {
    /// Type de juridiction normalisé (`CE`, `CAA`, `TA`, `CC` pour la Cour de
    /// cassation, `CNDA`, `CJUE`, `CEDH`). Informatif — la résolution se fait
    /// par (dossier, date).
    pub jurisdiction: String,
    /// Date de lecture ISO (`2026-02-02`).
    pub date_iso: String,
    /// Numéros de dossier normalisés au format stocké (`docket_numbers`).
    pub dockets: Vec<String>,
}

/// Mois français (avec variantes accentuées) → numéro. Ordre = index + 1.
const MONTHS: &[&str] = &[
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

/// `12 février 2026` / `1er mars 2025` — capture jour, mois, année.
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d{1,2})\s*(?:er)?\s+([a-zàâäéèêëîïôöùûüç]+)\s+(\d{4})").unwrap()
});

/// Numéro de pourvoi Cass. sans point (`24-15449`) → capture pour re-formater.
static CASS_DOCKET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{2}-\d{2})(\d{3})$").unwrap());

fn month_number(name: &str) -> Option<u8> {
    let name = name.to_lowercase();
    MONTHS
        .iter()
        .position(|m| *m == name)
        .map(|i| (i + 1) as u8)
}

/// Normalise la juridiction depuis le premier segment du titre.
fn jurisdiction_from_prefix(prefix: &str) -> Option<String> {
    let p = prefix.trim().to_uppercase();
    // `CAA`/`CEDH` avant `CE` : préfixes plus longs d'abord.
    let kind = if p.starts_with("CAA") {
        "CAA"
    } else if p.starts_with("CEDH") {
        "CEDH"
    } else if p.starts_with("CE") {
        "CE"
    } else if p.starts_with("TA") {
        "TA"
    } else if p.starts_with("CASS") {
        // Cour de cassation = type `CC` dans le référentiel juridiction.
        "CC"
    } else if p.starts_with("CNDA") {
        "CNDA"
    } else if p.starts_with("CJUE") {
        "CJUE"
    } else {
        return None;
    };
    Some(kind.to_string())
}

/// Normalise un numéro de dossier brut vers le format stocké. Cass. : le point
/// séparateur (`24-15449` → `24-15.449`) ; administratif : majuscules, sans
/// espaces (`24pa04236` → `24PA04236`).
fn normalize_docket(raw: &str, jurisdiction: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        // Retire un éventuel « n° » / « n ° » de tête.
        .trim_start_matches(['n', 'N', '°', '.'])
        .to_string();
    if cleaned.is_empty() {
        return None;
    }
    let cleaned = cleaned.to_uppercase();
    if jurisdiction == "CC" {
        if let Some(caps) = CASS_DOCKET_RE.captures(&cleaned) {
            return Some(format!("{}.{}", &caps[1], &caps[2]));
        }
    }
    Some(cleaned)
}

/// Parse un titre ADDE en citation structurée. `None` si la juridiction ou la
/// date sont absentes/illisibles (post hors périmètre jurisprudence).
pub fn parse_adde_title(title: &str) -> Option<AddeCitation> {
    let prefix = title.split(',').next()?;
    let jurisdiction = jurisdiction_from_prefix(prefix)?;

    let caps = DATE_RE.captures(title)?;
    let day: u8 = caps[1].parse().ok()?;
    let month = month_number(&caps[2])?;
    let year: u16 = caps[3].parse().ok()?;
    let date_iso = format!("{year:04}-{month:02}-{day:02}");

    // Les dossiers suivent la date : on prend le reste du titre après la date.
    let after_date = &title[caps.get(0)?.end()..];
    let dockets: Vec<String> = after_date
        .split([',', ';'])
        .flat_map(|seg| seg.split(" et "))
        .filter_map(|seg| normalize_docket(seg, &jurisdiction))
        // Un token de dossier contient au moins un chiffre.
        .filter(|d| d.chars().any(|c| c.is_ascii_digit()))
        .collect();

    if dockets.is_empty() {
        return None;
    }
    Some(AddeCitation {
        jurisdiction,
        date_iso,
        dockets,
    })
}

/// Assemble le `source_fields` d'un commentaire ADDE : un unique lien sortant
/// (`kind: "note"`, accès libre). Aucun corps n'est stocké — seulement le lien.
pub fn build_adde_source_fields(url: &str, date_post: &str) -> Value {
    json!({
        "commentaires": [{
            "kind": "note",
            "title": "Analyse de la décision",
            "publisher": "ADDE",
            "date": date_post,
            "url": url,
            "access": "libre",
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ce_single_docket() {
        let c = parse_adde_title("CE, 5 février 2026, n°499141").unwrap();
        assert_eq!(c.jurisdiction, "CE");
        assert_eq!(c.date_iso, "2026-02-05");
        assert_eq!(c.dockets, vec!["499141"]);
    }

    #[test]
    fn parse_caa_multiple_dockets() {
        let c = parse_adde_title(
            "CAA Paris, 30 janvier 2026, n°24PA04236, 24PA04614, 25PA00741 et 25PA04152",
        )
        .unwrap();
        assert_eq!(c.jurisdiction, "CAA");
        assert_eq!(c.date_iso, "2026-01-30");
        assert_eq!(
            c.dockets,
            vec!["24PA04236", "24PA04614", "25PA00741", "25PA04152"]
        );
    }

    #[test]
    fn parse_cass_dots_dockets() {
        let c = parse_adde_title("Cass. civ 1ère, 7 janvier 2026, 24-15449 et 24-15450").unwrap();
        assert_eq!(c.jurisdiction, "CC");
        assert_eq!(c.date_iso, "2026-01-07");
        assert_eq!(c.dockets, vec!["24-15.449", "24-15.450"]);
    }

    #[test]
    fn parse_ta_accented_month() {
        let c = parse_adde_title("TA Strasbourg, 2 décembre 2025, n°2509454").unwrap();
        assert_eq!(c.jurisdiction, "TA");
        assert_eq!(c.date_iso, "2025-12-02");
        assert_eq!(c.dockets, vec!["2509454"]);
    }

    #[test]
    fn parse_cedh_not_shadowed_by_ce() {
        let c = parse_adde_title("CEDH, 9 avril 2024, n°53600/20").unwrap();
        assert_eq!(c.jurisdiction, "CEDH");
        assert_eq!(c.date_iso, "2024-04-09");
        assert_eq!(c.dockets, vec!["53600/20"]);
    }

    #[test]
    fn rejects_non_jurisprudence_title() {
        assert!(
            parse_adde_title("Pour en finir avec les idées fausses sur les migrations").is_none()
        );
        assert!(parse_adde_title("L'ADDE demande l'abrogation du décret ANEF").is_none());
    }

    #[test]
    fn builds_link_only_bundle() {
        let v = build_adde_source_fields(
            "https://adde-association.org/ce-2-fevrier-2026-507674/",
            "2026-02-03",
        );
        let note = &v["commentaires"][0];
        assert_eq!(note["kind"], "note");
        assert_eq!(note["publisher"], "ADDE");
        assert_eq!(note["access"], "libre");
        assert!(note.get("body").is_none());
    }
}
