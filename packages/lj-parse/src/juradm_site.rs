//! A decision a tribunal administratif or a cour administrative d'appel publishes on its own site:
//! the court, the dockets and the date, read from the head of its PDF.

use std::sync::OnceLock;

use lj_core::dates::parse_date_fr;
use lj_core::decision::Decision;
use regex::Regex;
use serde_json::{json, Value};

use crate::extract_sections_xml;

/// A court with a site, and how the registry of the administrative courts names it.
pub struct JuradmCourt {
    pub code: &'static str,
    pub registry_code: &'static str,
    /// The name the registry's copies carry, which their address is built on.
    pub registry_name: &'static str,
}

impl JuradmCourt {
    pub fn jurisdiction_type(&self) -> &'static str {
        if self.code.starts_with("caa_") {
            "CAA"
        } else {
            "TA"
        }
    }
}

const fn court(
    code: &'static str,
    registry_code: &'static str,
    registry_name: &'static str,
) -> JuradmCourt {
    JuradmCourt {
        code,
        registry_code,
        registry_name,
    }
}

pub const JURADM_COURTS: &[JuradmCourt] = &[
    court(
        "caa_bordeaux",
        "CAA33",
        "Cour administrative d'appel de Bordeaux",
    ),
    court("caa_douai", "CAA59", "Cour administrative d'appel de Douai"),
    court("caa_lyon", "CAA69", "COUR ADMINISTRATIVE D'APPEL DE LYON"),
    court(
        "caa_marseille",
        "CAA13",
        "Cour administrative d'appel de Marseille",
    ),
    court("caa_nancy", "CAA54", "Cour Administrative d'Appel de Nancy"),
    court(
        "caa_nantes",
        "CAA44",
        "Cour Administrative d'Appel de Nantes",
    ),
    court("caa_paris", "CAA75", "Cour administrative d'appel de Paris"),
    court(
        "caa_toulouse",
        "CAA31",
        "Cour administrative d'appel de Toulouse",
    ),
    court(
        "caa_versailles",
        "CAA78",
        "Cour administrative d'appel de Versailles",
    ),
    court("ta_amiens", "TA80", "Tribunal Administratif Amiens"),
    court("ta_bastia", "TA20", "Tribunal Administratif de Bastia"),
    court("ta_besancon", "TA25", "Tribunal Administratif de Besançon"),
    court("ta_bordeaux", "TA33", "Tribunal Administratif de Bordeaux"),
    court("ta_caen", "TA14", "Tribunal Administratif de Caen"),
    court(
        "ta_cergy_pontoise",
        "TA95",
        "Tribunal Administratif de Cergy-Pontoise",
    ),
    court(
        "ta_chalons_en_champagne",
        "TA51",
        "Tribunal Administratif de Châlons-en-Champagne",
    ),
    court(
        "ta_clermont_ferrand",
        "TA63",
        "Tribunal Administratif de Clermont-Ferrand",
    ),
    court("ta_dijon", "TA21", "Tribunal Administratif de Dijon"),
    court("ta_grenoble", "TA38", "Tribunal Administratif de Grenoble"),
    court(
        "ta_guadeloupe",
        "TA105",
        "Tribunal Administratif de la Guadeloupe",
    ),
    court("ta_guyane", "TA106", "Tribunal Administratif de la Guyane"),
    court(
        "ta_la_reunion",
        "TA101",
        "Tribunal Administratif de La Réunion",
    ),
    court("ta_lille", "TA59", "Tribunal Administratif de Lille"),
    court("ta_limoges", "TA87", "Tribunal Administratif de Limoges"),
    court("ta_lyon", "TA69", "Tribunal Administratif de Lyon"),
    court(
        "ta_marseille",
        "TA13",
        "Tribunal Administratif de Marseille",
    ),
    court(
        "ta_martinique",
        "TA102",
        "Tribunal Administratif de la Martinique",
    ),
    court("ta_mayotte", "TA107", "Tribunal Administratif de Mayotte"),
    court("ta_melun", "TA77", "Tribunal Administratif de MELUN"),
    court(
        "ta_montpellier",
        "TA34",
        "Tribunal Administratif de Montpellier",
    ),
    court(
        "ta_montreuil",
        "TA93",
        "Tribunal Administratif de Montreuil",
    ),
    court("ta_nancy", "TA54", "Tribunal Administratif de Nancy"),
    court("ta_nantes", "TA44", "Tribunal Administratif de Nantes"),
    court("ta_nice", "TA06", "Tribunal Administratif de Nice"),
    court("ta_nimes", "TA30", "Tribunal Administratif de Nîmes"),
    court(
        "ta_nouvelle_caledonie",
        "TA104",
        "Tribunal Administratif DE NOUVELLE-CALEDONIE",
    ),
    court("ta_orleans", "TA45", "Tribunal Administratif d'Orléans"),
    court("ta_paris", "TA75", "Tribunal Administratif de Paris"),
    court("ta_pau", "TA64", "Tribunal Administratif de Pau"),
    court("ta_poitiers", "TA86", "Tribunal Administratif de Poitiers"),
    court(
        "ta_polynesie_francaise",
        "TA103",
        "Tribunal Administratif de la Polynésie française",
    ),
    court("ta_rennes", "TA35", "Tribunal Administratif de Rennes"),
    court("ta_rouen", "TA76", "Tribunal Administratif de Rouen"),
    court(
        "ta_saint_barthelemy",
        "TA109",
        "Tribunal Administratif de St Barthélemy",
    ),
    court(
        "ta_saint_martin",
        "TA108",
        "Tribunal Administratif de St Martin",
    ),
    court(
        "ta_strasbourg",
        "TA67",
        "Tribunal Administratif de Strasbourg",
    ),
    court("ta_toulon", "TA83", "Tribunal Administratif de Toulon"),
    court("ta_toulouse", "TA31", "Tribunal Administratif de Toulouse"),
    court(
        "ta_versailles",
        "TA78",
        "Tribunal Administratif de VERSAILLES",
    ),
];

pub fn juradm_court(code: &str) -> Option<&'static JuradmCourt> {
    JURADM_COURTS.iter().find(|c| c.code == code)
}

/// What the head of a decision says of it.
#[derive(Debug, Clone, PartialEq)]
pub struct JuradmHead {
    pub court: &'static JuradmCourt,
    pub numbers: Vec<String>,
    /// `Ordonnance` or `Décision`, as the registry types them.
    pub kind: &'static str,
    pub decision_date: String,
    pub audience_date: Option<String>,
}

impl PartialEq for JuradmCourt {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl std::fmt::Debug for JuradmCourt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}

/// Why a PDF gives no decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JuradmRefused {
    /// No « au nom du peuple français »: a press release, conclusions, a letter.
    NotADecision,
    /// A court outside [`JURADM_COURTS`] at the head, the Conseil d'État for one.
    UnknownCourt,
    NoDocket,
    NoDate,
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// « TRIBUNAL ADMINISTRATIF D'AMIENS » and « Tribunal Administratif Amiens » give one key.
fn court_key(s: &str) -> String {
    let mut key = String::from(" ");
    for word in lj_fold::slug(s, ' ').split(' ') {
        let word = match word {
            "de" | "d" | "du" | "des" | "la" | "l" | "le" => continue,
            "st" => "saint",
            w => w,
        };
        key.push_str(word);
        key.push(' ');
    }
    key
}

/// The decision in `pdftotext`'s rendering, from the first page that says « au nom du peuple
/// français »: a press release may come before it.
pub fn juradm_decision(raw: &str) -> Option<&str> {
    let mut start = 0;
    for page in raw.split('\u{c}') {
        if court_key(page).contains(" au nom peuple francais ") {
            return Some(&raw[start..]);
        }
        start += page.len() + 1;
    }
    None
}

fn one_digit_more(typo: &str, docket: &str) -> bool {
    (0..typo.len()).any(|i| format!("{}{}", &typo[..i], &typo[i + 1..]) == docket)
}

/// The head of a decision, its first page up to its first « Vu ».
pub fn parse_juradm_head(raw: &str) -> Result<JuradmHead, JuradmRefused> {
    static VU: OnceLock<Regex> = OnceLock::new();
    static DOCKET_LINE: OnceLock<Regex> = OnceLock::new();
    static DOCKET: OnceLock<Regex> = OnceLock::new();
    static BODY_DOCKET: OnceLock<Regex> = OnceLock::new();
    static TYPO: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    static AUDIENCE: OnceLock<Regex> = OnceLock::new();
    let raw = juradm_decision(raw).ok_or(JuradmRefused::NotADecision)?;
    let head = &raw[..raw.floor_char_boundary(3_000)];
    let head = match regex(&VU, r"(?m)^\s*Vu\b").find(head) {
        Some(vu) => &head[..vu.start()],
        None => head,
    };
    let key = court_key(head);
    let court = JURADM_COURTS
        .iter()
        .filter_map(|c| key.find(&court_key(c.registry_name)).map(|at| (at, c)))
        .min_by_key(|(at, _)| *at)
        .map(|(_, c)| c)
        .ok_or(JuradmRefused::UnknownCourt)?;

    let docket = regex(&DOCKET, r"\b(?:\d{7}|\d{2}[A-Z]{2}\d{5})\b");
    let docket_line = regex(
        &DOCKET_LINE,
        r"^\s*(?:N\s*(?:°|os?\b)s?)?\s*((?:\d{7}|\d{2}[A-Z]{2}\d{5})(?:/\d+(?:-\d+)?)?(?:\s*(?:,|;|-|\x{2013}|et)?\s*(?:\d{7}|\d{2}[A-Z]{2}\d{5})(?:/\d+(?:-\d+)?)?)*)\s*$",
    );
    let mut numbers: Vec<String> = head
        .lines()
        .filter_map(|line| docket_line.captures(line))
        .flat_map(|c| {
            docket
                .find_iter(c.get(1).unwrap().as_str())
                .map(|m| m.as_str().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    if numbers.is_empty() {
        // The body names other cases too, the merits case of a référé among them: it gives a
        // docket only to a head docket mistyped with one digit too many.
        let typos: Vec<&str> = regex(&TYPO, r"(?m)^\s*N\s*(?:°|os?\b)s?\s*(\d{8})\s*$")
            .captures_iter(head)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        let body = &raw[..raw.floor_char_boundary(8_000)];
        numbers.extend(
            regex(&BODY_DOCKET, r"sous les? n(?:°|os?\b)s?\s*(\d{7})\b")
                .captures_iter(body)
                .map(|c| c[1].to_string())
                .filter(|n| typos.iter().any(|t| one_digit_more(t, n))),
        );
    }
    let mut seen = std::collections::HashSet::new();
    numbers.retain(|n| seen.insert(n.clone()));
    if numbers.is_empty() {
        return Err(JuradmRefused::NoDocket);
    }

    const WEEKDAY: &str = r"(?:(?:lundi|mardi|mercredi|jeudi|vendredi|samedi|dimanche)\s+)?";
    let date_re = DATE.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\b(ordonnance|jugement|arr[êe]t|d[ée]cision|lecture)\s+du\s+{WEEKDAY}(\d{{1,2}}(?:er)?\s+\p{{L}}+\s+\d{{4}})"
        ))
        .unwrap()
    });
    let (kind, decision_date) = date_re
        .captures_iter(head)
        .find_map(|c| {
            let date = parse_date_fr(&c[2])?;
            let kind = if c[1].eq_ignore_ascii_case("ordonnance") {
                "Ordonnance"
            } else {
                "Décision"
            };
            Some((kind, date.to_string()))
        })
        .ok_or(JuradmRefused::NoDate)?;
    let audience_re = AUDIENCE.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\baudience\s+(?:publique\s+)?du\s+{WEEKDAY}(\d{{1,2}}(?:er)?\s+\p{{L}}+\s+\d{{4}})"
        ))
        .unwrap()
    });
    let audience_date = audience_re
        .captures(head)
        .and_then(|c| parse_date_fr(&c[1]))
        .map(|d| d.to_string());
    Ok(JuradmHead {
        court,
        numbers,
        kind,
        decision_date,
        audience_date,
    })
}

/// The page that links the file, the file at the site and in the bucket, and what its head says.
pub fn juradm_site_source_fields(page: &str, pdf: &str, file: &str, head: &JuradmHead) -> Value {
    json!({
        "url_site": page,
        "pdf": pdf,
        "file": file,
        "court": head.court.code,
        "numbers": head.numbers,
        "type": head.kind,
        "decision_date": head.decision_date,
        "audience_date": head.audience_date,
    })
}

pub(crate) fn from_source_fields_juradm_site(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |key: &str| {
        source_fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let court = field("court").as_deref().and_then(juradm_court);
    let numbers: Vec<String> = source_fields
        .get("numbers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Decision {
        source_uid: source_uid.to_string(),
        member_name: source_uid.to_string(),
        ecli: None,
        jurisdiction_source_code: court.map(|c| c.registry_code.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: court.map(|c| c.registry_name.to_string()),
        jurisdiction_type: court.map(|c| c.jurisdiction_type().to_string()),
        jurisdiction_location: None,
        numero_dossier: numbers.first().cloned(),
        numero_dossiers: Some(numbers),
        numero_role: None,
        date_lecture: field("decision_date"),
        date_audience: field("audience_date"),
        date_mise_jour: None,
        formation: None,
        type_decision: field("type"),
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections: extract_sections_xml(full_text),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLEMIA: &str = "TRIBUNAL ADMINISTRATIF\nDE CERGY-PONTOISE\nN° 2623718\n___________\n\nRÉPUBLIQUE FRANÇAISE\n\nAssociation POLEMIA\n___________\n\nAU NOM DU PEUPLE FRANÇAIS\n\nMme\nM\nM\nJuges des référés\n___________\n\nLe juge des référés\nstatuant dans les conditions prévues au\ntroisième alinéa de l’article L. 511-2\ndu code de justice administrative\n\nOrdonnance du 30 octobre 2026\n___________\nVu la procédure suivante :\nPar une requête enregistrée le 29 octobre 2026, l’association Polemia";

    #[test]
    fn the_head_gives_the_court_the_docket_and_the_date() {
        let head = parse_juradm_head(POLEMIA).unwrap();
        assert_eq!(head.court.code, "ta_cergy_pontoise");
        assert_eq!(head.numbers, vec!["2623718"]);
        assert_eq!(head.kind, "Ordonnance");
        assert_eq!(head.decision_date, "2026-10-30");
        assert_eq!(head.audience_date, None);
    }

    #[test]
    fn joined_dockets_are_read_on_every_docket_line() {
        let raw = "COUR ADMINISTRATIVE D'APPEL\nDE LYON\n\nN° 25LY01973\n25LY02004\n__________\nPREFETE DE L’ISERE\nAU NOM DU PEUPLE FRANÇAIS\nAudience du 10 septembre 2025\nOrdonnance du 11 septembre 2025\n__________\n\nVu la procédure suivante :";
        let head = parse_juradm_head(raw).unwrap();
        assert_eq!(head.court.code, "caa_lyon");
        assert_eq!(head.numbers, vec!["25LY01973", "25LY02004"]);
        assert_eq!(head.audience_date.as_deref(), Some("2025-09-10"));
        assert_eq!(head.decision_date, "2025-09-11");
    }

    #[test]
    fn a_mistyped_head_docket_falls_back_on_the_body() {
        let raw = "TRIBUNAL ADMINISTRATIF\nDE BORDEAUX\n\nN° 26006772\nN° 26007358\n___________\nRÉPUBLIQUE FRANÇAISE\n\nAU NOM DU PEUPLE FRANÇAIS\nAudience du 1er octobre 2026\nOrdonnance du 14 octobre 2026\n__________\n\nVu la procédure suivante :\nI. Par une requête enregistrée le 22 août 2026 sous le n° 2606772, complétée\nII. Par une requête enregistrée le 23 août 2026 sous le n° 2607358,";
        let head = parse_juradm_head(raw).unwrap();
        assert_eq!(head.numbers, vec!["2606772", "2607358"]);
        assert_eq!(head.audience_date.as_deref(), Some("2026-10-01"));
    }

    #[test]
    fn a_chamber_suffix_is_dropped_and_another_case_of_the_body_is_no_docket() {
        let referral = "TRIBUNAL ADMINISTRATIF\nDE PARIS\nNo 2604445/6\nRÉPUBLIQUE FRANÇAISE\nAU NOM DU PEUPLE FRANÇAIS\nOrdonnance du 18 février 2026\n___________\nVu la procédure suivante :\n- le dossier de la requête au fond enregistrée le 12 février 2026 sous le n° 2604447 ;";
        assert_eq!(
            parse_juradm_head(referral).unwrap().numbers,
            vec!["2604445"]
        );
        let raw = "TRIBUNAL ADMINISTRATIF\nDE PARIS\nN° 2328997/4-1\nAU NOM DU PEUPLE FRANÇAIS\nDécision du 26 juin 2025\nVu la procédure suivante :";
        assert_eq!(parse_juradm_head(raw).unwrap().numbers, vec!["2328997"]);
        let unnumbered = referral.replace("No 2604445/6\n", "");
        assert_eq!(
            parse_juradm_head(&unnumbered).unwrap_err(),
            JuradmRefused::NoDocket
        );
    }

    #[test]
    fn a_reading_date_may_name_its_weekday_and_the_registry_name_its_court_otherwise() {
        let raw = "REPUBLIQUE FRANCAISE\nAU NOM DU PEUPLE FRANCAIS\n\nTribunal Administratif d'Amiens\nN° 2201345\nLecture du jeudi 18 avril 2024\n1ère Chambre\n";
        let head = parse_juradm_head(raw).unwrap();
        assert_eq!(head.court.code, "ta_amiens");
        assert_eq!(head.kind, "Décision");
        assert_eq!(head.decision_date, "2024-04-18");
        let raw = "TRIBUNAL ADMINISTRATIF\nDE SAINT-BARTHÉLEMY\nN° 2500012\nAU NOM DU PEUPLE FRANÇAIS\nJugement du 2 mai 2025\n";
        assert_eq!(
            parse_juradm_head(raw).unwrap().court.code,
            "ta_saint_barthelemy"
        );
    }

    #[test]
    fn a_press_release_before_the_decision_is_left_out() {
        let raw = format!(
            "Communiqué de presse\nStrasbourg, le 16 octobre 2025\nUn appel est possible devant la cour administrative d’appel de Nancy.\n\u{c}{}",
            POLEMIA.replace("CERGY-PONTOISE", "STRASBOURG")
        );
        let decision = juradm_decision(&raw).unwrap();
        assert!(decision.starts_with("TRIBUNAL ADMINISTRATIF\nDE STRASBOURG"));
        assert_eq!(parse_juradm_head(&raw).unwrap().court.code, "ta_strasbourg");
    }

    #[test]
    fn a_press_release_and_another_court_give_no_decision() {
        let raw = "Communiqué de presse\nNancy, le 25 avril 2024\n\nLa cour administrative d’appel de Nancy rejette";
        assert_eq!(parse_juradm_head(raw), Err(JuradmRefused::NotADecision));
        let raw = "CONSEIL D'ETAT\nstatuant au contentieux\nN° 498765\nRÉPUBLIQUE FRANÇAISE\nAU NOM DU PEUPLE FRANÇAIS\nDécision du 3 mars 2025";
        assert_eq!(parse_juradm_head(raw), Err(JuradmRefused::UnknownCourt));
    }
}
