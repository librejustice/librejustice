//! A decision of the Défenseur des droits, or of an authority it succeeded, from its catalogue
//! notice and the text of its PDF.

use std::collections::BTreeMap;

use lj_core::decision::Decision;
use lj_core::official_url::DDD_NOTICE_URL;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::extract_sections_xml;
use crate::legalis::{legalis_court, LegalisCourt};

const DEFENSEUR: &str = "Défenseur des droits";

/// The authorities the Défenseur des droits succeeded, by the start of the first author the
/// notice names, with the name a decision carries.
const PREDECESSORS: &[(&str, &str)] = &[
    (
        "Haute autorité de lutte contre les discriminations",
        "Haute autorité de lutte contre les discriminations et pour l'égalité",
    ),
    (
        "Commission nationale de déontologie de la sécurité",
        "Commission nationale de déontologie de la sécurité",
    ),
    ("Défenseur des Enfants", "Défenseur des enfants"),
    ("Médiateur de la République", "Médiateur de la République"),
];

pub(crate) fn source_uid_is_ddd(source_uid: &str) -> bool {
    source_uid.starts_with("ddd/")
}

/// What the decision of notice `notice_id` keeps of the catalogue: every field of the notice, as
/// its label reads, and the ids of its files.
pub fn ddd_source_fields(
    notice_id: u64,
    fields: &BTreeMap<String, String>,
    files: &[u64],
) -> Value {
    json!({
        "notice_id": notice_id,
        "notice_url": format!("{DDD_NOTICE_URL}{notice_id}"),
        "files": files,
        "fields": fields,
    })
}

/// The name a decision of the notice carries: its first author when it is an authority the
/// Défenseur des droits succeeded, the Défenseur des droits otherwise, its colleges included.
pub(crate) fn ddd_authority(authors: &str) -> &'static str {
    let first = authors.trim_start();
    PREDECESSORS
        .iter()
        .find(|(start, _)| first.starts_with(start))
        .map_or(DEFENSEUR, |(_, name)| name)
}

pub(crate) fn from_source_fields_ddd(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |label: &str| {
        source_fields
            .get("fields")
            .and_then(|f| f.get(label))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let numero = field("Numéro de décision ou d'affaire").map(str::to_string);
    Decision {
        source_uid: source_uid.to_string(),
        member_name: numero.clone().unwrap_or_default(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some(ddd_authority(field("Auteurs").unwrap_or_default()).to_string()),
        jurisdiction_type: Some("DDD".to_string()),
        jurisdiction_location: None,
        numero_dossiers: numero.clone().map(|n| vec![n]),
        numero_dossier: numero,
        numero_role: None,
        date_lecture: field("Année de publication").and_then(iso_date),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
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

pub fn ddd_jurisprudence_source_uid(notice_id: u64) -> String {
    format!("{JURISPRUDENCE_PREFIX}{notice_id}")
}

const JURISPRUDENCE_PREFIX: &str = "ddd-jurisprudence/";

pub(crate) fn source_uid_is_ddd_jurisprudence(source_uid: &str) -> bool {
    source_uid.starts_with(JURISPRUDENCE_PREFIX)
}

/// `Cour de cassation, Cass. Soc.` of `Cour de cassation, Cass. Soc. , Auteur ; Cour de cassation , Auteur`.
fn first_author(authors: &str) -> &str {
    let first = authors.split(';').next().unwrap_or(authors);
    first
        .trim()
        .strip_suffix("Auteur")
        .map_or(first, |head| head.trim_end().trim_end_matches(','))
        .trim()
}

/// A French court of first instance or of appeal; the others have a source of their own.
pub fn ddd_jurisprudence_court(authors: &str, date: &str) -> Option<LegalisCourt> {
    legalis_court(first_author(authors), date).filter(|c| {
        matches!(
            c.jurisdiction_type,
            "CA" | "CAA" | "TA" | "TJ" | "TCOM" | "CPH"
        )
    })
}

const MINOR_WORDS: &[&str] = &[
    "mineur",
    "mineure",
    "mineurs",
    "mineures",
    "enfant",
    "enfants",
    "enfance",
    "educative",
    "fille",
    "filles",
    "fils",
    "eleve",
    "eleves",
    "scolaire",
    "scolarite",
    "creche",
];

/// A notice whose title, authors or keywords name a minor, a child or child protection.
pub fn ddd_concerns_minor(fields: &BTreeMap<String, String>) -> bool {
    ["Titre", "Auteurs", "Mots-clés"]
        .iter()
        .filter_map(|label| fields.get(*label))
        .any(|value| {
            lj_fold::slug(value, ' ')
                .split(' ')
                .any(|w| MINOR_WORDS.contains(&w))
        })
}

pub(crate) fn from_source_fields_ddd_jurisprudence(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |label: &str| {
        source_fields
            .get("fields")
            .and_then(|f| f.get(label))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let date = field("Année de publication").and_then(iso_date);
    let author = first_author(field("Auteurs").unwrap_or_default());
    let court = date
        .as_deref()
        .and_then(|date| ddd_jurisprudence_court(author, date));
    let dockets: Vec<String> = field("Numéro de décision ou d'affaire")
        .unwrap_or_default()
        .split([';', ','])
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    Decision {
        source_uid: source_uid.to_string(),
        member_name: source_fields["notice_id"].to_string(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: court.as_ref().map(|c| c.name.clone()),
        jurisdiction_type: court.as_ref().map(|c| c.jurisdiction_type.to_string()),
        jurisdiction_location: court.and_then(|c| c.location),
        numero_dossier: dockets.first().cloned(),
        numero_dossiers: (!dockets.is_empty()).then_some(dockets),
        numero_role: None,
        date_lecture: date,
        date_audience: None,
        date_mise_jour: None,
        formation: author
            .split_once(',')
            .map(|(_, room)| room.trim().to_string())
            .filter(|room| !room.is_empty()),
        type_decision: None,
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

/// `16/04/2026` as `2026-04-16`; a year alone is no date.
fn iso_date(value: &str) -> Option<String> {
    static DATE: OnceLock<Regex> = OnceLock::new();
    let c = DATE
        .get_or_init(|| Regex::new(r"^(\d{2})/(\d{2})/(\d{4})$").unwrap())
        .captures(value)?;
    Some(format!("{}-{}-{}", &c[3], &c[2], &c[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;

    #[test]
    fn a_notice_reads_as_a_decision_of_its_authority() {
        let fields: BTreeMap<String, String> = [
            ("Titre", "Délibération n°2010-12 du 18 janvier 2010"),
            ("Auteurs", "Haute autorité de lutte contre les discriminations et pour l'égalité (2005-2011) , Auteur"),
            ("Type de document", "Décisions"),
            ("Année de publication", "18/01/2010"),
            ("Numéro de décision ou d'affaire", "2010-12"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let text = "Vu la loi n° 2004-1486 du 30 décembre 2004 ;\nLe Collège recommande au maire de réexaminer la demande.";
        let d = Decision::from_source_fields(text, &ddd_source_fields(42, &fields, &[7]), "ddd/42");
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Haute autorité de lutte contre les discriminations et pour l'égalité")
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("DDD"));
        assert_eq!(d.numero_dossier.as_deref(), Some("2010-12"));
        assert_eq!(d.date_lecture.as_deref(), Some("2010-01-18"));
    }

    #[test]
    fn a_court_decision_reads_under_its_court_and_docket() {
        let fields: BTreeMap<String, String> = [
            ("Titre", "Jugement correctionnel relatif à une discrimination à l'embauche"),
            ("Auteurs", "Tribunal de grande instance de Bobigny, 17e chambre , Auteur ; Défenseur des droits , Auteur"),
            ("Type de document", "Jurisprudences"),
            ("Année de publication", "21/11/2013"),
            ("Numéro de décision ou d'affaire", "13070000305 ; 13070000306"),
            ("Mots-clés", "[Mots-clés] Religion - Croyances [Mots-clés] Embauche"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let uid = ddd_jurisprudence_source_uid(10628);
        let d = Decision::from_source_fields(
            "Le tribunal déclare le prévenu coupable.",
            &ddd_source_fields(10628, &fields, &[8674]),
            &uid,
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("TJ"));
        assert_eq!(d.jurisdiction_location.as_deref(), Some("tgi_bobigny"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Tribunal de grande instance de Bobigny")
        );
        assert_eq!(d.formation.as_deref(), Some("17e chambre"));
        assert_eq!(d.date_lecture.as_deref(), Some("2013-11-21"));
        assert_eq!(
            d.numero_dossiers,
            Some(vec!["13070000305".into(), "13070000306".into()])
        );
        assert!(!ddd_concerns_minor(&fields));
    }

    #[test]
    fn only_the_french_courts_below_the_supreme_ones_are_kept() {
        let court = |author: &str| {
            ddd_jurisprudence_court(author, "2024-01-01").map(|c| c.location.unwrap_or(c.name))
        };
        assert_eq!(
            court("Conseil de prud'hommes de Saint-Germain-en-Laye , Auteur").as_deref(),
            Some("cph_saint_germain_en_laye")
        );
        assert_eq!(
            court("Cour administrative d'appel de Versailles").as_deref(),
            Some("Cour administrative d'appel de Versailles")
        );
        assert_eq!(
            court("Tribunal pour enfants de Paris , Auteur").as_deref(),
            Some("tj_paris")
        );
        assert_eq!(
            court("Conseil de prud'hommes de Saint-Denis , Auteur").as_deref(),
            Some("cph_saint_denis_de_la_reunion")
        );
        assert_eq!(
            court("Conseil de prud'hommes de Lons-le-Saulnier , Auteur").as_deref(),
            Some("cph_lons_le_saunier")
        );
        assert_eq!(
            court("Conseil de prud'hommes de Chatelleraut , Auteur").as_deref(),
            Some("cph_chatellerault")
        );
        assert_eq!(
            court("Conseil de prud'hommes des Sables d'Olonne , Auteur").as_deref(),
            Some("cph_les_sables_d_olonne")
        );
        assert_eq!(
            court("Tribunal de grande instance de Béthunes , Auteur").as_deref(),
            Some("tgi_bethune")
        );
        assert_eq!(
            court("Tribunal d'instance de Paris 15ème , Auteur").as_deref(),
            Some("ti_paris_15e_arrondissement")
        );
        for (author, seat) in [
            (
                "Tribunal des affaires de sécurité sociale de Saint-Etienne , Auteur",
                "tass_saint_etienne",
            ),
            (
                "Tribunal des affaires de sécurité sociale de l'Allier , Auteur",
                "tass_moulins",
            ),
            (
                "Tribunal des affaires de sécurité sociale des Hauts-de-Seine , Auteur",
                "tass_nanterre",
            ),
            (
                "Tribunal des affaires de sécurité sociale du Val d'Oise , Auteur",
                "tass_pontoise",
            ),
            (
                "Tribunal des affaires de sécurité sociale de la Haute Savoie , Auteur",
                "tass_annecy",
            ),
            (
                "Tribunal des affaires de sécurité sociale d'Ille et Vilaine , Auteur",
                "tass_rennes",
            ),
            (
                "Tribunal des affaires de sécurité sociale de Moselle , Auteur",
                "tass_metz",
            ),
            (
                "Tribunal des affaires de sécurité sociales des Hautes Pyrénées , Auteur",
                "tass_tarbes",
            ),
            (
                "Tribunal des affaires de sécurité sociale des Alpes de Haute-Provence , Auteur",
                "tass_digne_les_bains",
            ),
        ] {
            assert_eq!(court(author).as_deref(), Some(seat), "{author}");
        }
        for other in [
            "Cour de cassation, Cass. Soc. , Auteur ; Cour de cassation , Auteur",
            "Conseil d'État , Auteur",
            "Cour européenne des droits de l'homme , Auteur",
            "Comité des droits de l'enfant (CRC), ONU , Auteur",
        ] {
            assert_eq!(court(other), None, "{other}");
        }
    }

    #[test]
    fn a_minor_in_the_title_or_keywords_marks_the_notice() {
        let notice = |title: &str, keywords: &str| -> BTreeMap<String, String> {
            [("Titre", title), ("Mots-clés", keywords)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert!(ddd_concerns_minor(&notice(
            "Arrêt relatif à l'évaluation de la minorité d'un jeune étranger",
            "[Mots-clés] Mineur non accompagné (MNA)"
        )));
        assert!(ddd_concerns_minor(&notice(
            "Ordonnance relative au refus d'inscription de la fille des requérants",
            ""
        )));
        assert!(ddd_concerns_minor(&notice(
            "Jugement relatif à une mesure d'assistance éducative",
            ""
        )));
        assert!(!ddd_concerns_minor(&notice(
            "Arrêt relatif à un licenciement discriminatoire à raison de l'état de santé",
            "[Mots-clés] Emploi privé [Mots-clés] Handicap"
        )));
    }

    #[test]
    fn a_college_of_the_defenseur_is_the_defenseur() {
        assert_eq!(
            ddd_authority("Justice et libertés , Auteur"),
            "Défenseur des droits"
        );
        assert_eq!(
            ddd_authority("Défenseur des droits , Auteur ; Défense des enfants , Auteur"),
            "Défenseur des droits"
        );
        assert_eq!(
            ddd_authority("Médiateur de la République (1973-2011) , Auteur"),
            "Médiateur de la République"
        );
        assert_eq!(iso_date("2026"), None);
    }
}
