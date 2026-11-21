//! Référentiels facettes côté extraction.
//!
//! Les scanners émettent les uids `solution:*`/`procedure:*`/`office:*`/
//! `legal_domain:*` directement aux sites de classification. Reste la
//! dérivation transverse du référentiel `jurisdiction` (code + ligne à la
//! volée).
//!
//! Chaque uid émis par les scanners existe dans le seed des référentiels de
//! facettes (FK) — le test `emitted_uids_exist_in_seed` le garantit. Valeur
//! inconnue en entrée → `None`.

/// Ligne du référentiel `jurisdiction` portée par une décision (le code est
/// la FK de `decisions.jurisdiction_code` ; label/city servent à créer la
/// ligne à la volée, `ON CONFLICT DO NOTHING`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JurisdictionRef {
    pub code: String,
    pub jurisdiction_type: String,
    pub city: Option<String>,
    pub label: String,
}

// ============================================================
// Référentiel jurisdiction — code + ligne à la volée.
// ============================================================

/// Code + ligne `jurisdiction` d'une décision, depuis ses champs extraits.
///
/// - `location` : code de localisation Judilibre (`tj76351`, `7801` pour un
///   TCOM, slug CA), tel que porté par `Decision::jurisdiction_location` OU
///   extrait de `canonical_ref` au backfill ([`location_from_canonical_ref`]).
/// - `jurisdiction_name` : libellé extrait (porte la ville quand la source la
///   donne) — le label de la ligne est RECONSTRUIT canoniquement depuis lui
///   ([`canonical_label`]), jamais recopié brut.
///
/// `jurisdiction` = la cour, pour tous les ordres : la Cassation est une
/// juridiction unique `cc`, la chambre vit dans le siège (`seat`), jamais dans
/// l'identité de juridiction.
pub fn jurisdiction_ref(
    jurisdiction_type: &str,
    location: Option<&str>,
    jurisdiction_name: Option<&str>,
) -> Option<JurisdictionRef> {
    let (code, label_override) = match jurisdiction_type {
        "CC" => ("cc".to_string(), Some("Cour de cassation".to_string())),
        "CE" => ("ce".to_string(), Some("Conseil d'État".to_string())),
        "CNDA" => (
            "cnda".to_string(),
            Some("Cour nationale du droit d'asile".to_string()),
        ),
        "TC" => ("tc".to_string(), Some("Tribunal des conflits".to_string())),
        "CONSTIT" => (
            "constit".to_string(),
            Some("Conseil constitutionnel".to_string()),
        ),
        "CNIL" => (
            "cnil".to_string(),
            Some("Commission nationale de l'informatique et des libertés".to_string()),
        ),
        // Cours européennes : grain juridiction unique, nos colonnes ne portent aucune
        // formation pour elles.
        "CJUE" => (
            "cjue".to_string(),
            Some("Cour de justice de l'Union européenne".to_string()),
        ),
        "CEDH" => (
            "cedh".to_string(),
            Some("Cour européenne des droits de l'homme".to_string()),
        ),
        "DDD" => {
            let (code, label) = match jurisdiction_name.map(|n| lj_fold::slug(n, '_')) {
                Some(n) if n.starts_with("haute_autorite_de_lutte_contre_les_discriminations") => (
                    "halde",
                    "Haute autorité de lutte contre les discriminations et pour l'égalité",
                ),
                Some(n) if n == "commission_nationale_de_deontologie_de_la_securite" => {
                    ("cnds", "Commission nationale de déontologie de la sécurité")
                }
                Some(n) if n == "defenseur_des_enfants" => {
                    ("defenseur_enfants", "Défenseur des enfants")
                }
                Some(n) if n == "mediateur_de_la_republique" => {
                    ("mediateur_republique", "Médiateur de la République")
                }
                _ => ("ddd", "Défenseur des droits"),
            };
            (code.to_string(), Some(label.to_string()))
        }
        "ADLC" => match jurisdiction_name {
            Some("Conseil de la concurrence") => (
                "conseil_concurrence".to_string(),
                Some("Conseil de la concurrence".to_string()),
            ),
            _ => (
                "adlc".to_string(),
                Some("Autorité de la concurrence".to_string()),
            ),
        },
        "AMF" => (
            "amf".to_string(),
            Some("Autorité des marchés financiers".to_string()),
        ),
        "COMEU" => {
            let name = jurisdiction_name?;
            (
                lj_parse::comeu_court_code(name)?.to_string(),
                Some(name.to_string()),
            )
        }
        "ASSISES" | "CCD" => return criminal_court(jurisdiction_type, jurisdiction_name?),
        "TJ" | "CA" | "TCOM" | "CPH" => {
            let loc = normalize_location(location?);
            // Les codes TCOM Judilibre sont des numéros nus → préfixe type.
            let code = if loc.chars().all(|c| c.is_ascii_digit()) {
                format!("{}{loc}", jurisdiction_type.to_lowercase())
            } else {
                loc
            };
            (code, None)
        }
        "CAA"
            if jurisdiction_name
                .is_some_and(|n| lj_fold::slug(n, '_') == "commission_centrale_d_aide_sociale") =>
        {
            (
                "ccas".to_string(),
                Some("Commission centrale d'aide sociale".to_string()),
            )
        }
        "TA" | "CAA" => {
            let city = jurisdiction_name.and_then(city_from_name)?;
            let slug = lj_fold::slug(&city, '_');
            // Nomenclatures fermées (9 CAA, 42 TA) : une ville hors liste est un nom de
            // greffe corrompu (« CAA de VERSAILLESS », « Tribunal Administratif d Amiens »)
            // → `None`, jamais de code fantôme. CAA : l'appelant replie sur le code cour du
            // numéro de requête ; TA : la décision s'ingère sans facette juridiction.
            let closed_set = if jurisdiction_type == "CAA" {
                CAA_CITY_SLUGS
            } else {
                TA_CITY_SLUGS
            };
            if !closed_set.contains(&slug.as_str()) {
                return None;
            }
            let code = format!("{}_{slug}", jurisdiction_type.to_lowercase());
            (code, None)
        }
        _ => return None,
    };
    let label = label_override
        .or_else(|| jurisdiction_name.and_then(canonical_label))
        .or_else(|| type_label(jurisdiction_type).map(str::to_owned))
        .unwrap_or_else(|| code.clone());
    let city = if jurisdiction_type == "CC" {
        None
    } else {
        city_from_name(&label)
    };
    Some(JurisdictionRef {
        code,
        jurisdiction_type: jurisdiction_type.to_owned(),
        city,
        label,
    })
}

/// Villes des neuf CAA officielles, en slug (miroir de
/// [`caa_label_from_docket`]).
const CAA_CITY_SLUGS: &[&str] = &[
    "bordeaux",
    "douai",
    "lyon",
    "marseille",
    "nancy",
    "nantes",
    "paris",
    "toulouse",
    "versailles",
];

/// Villes des 42 TA officiels (art. R. 221-3 CJA), dans l'alphabet
/// d'extraction : slug du complément de nom APRÈS la préposition, donc « de La
/// Réunion » → `reunion`. En queue, les variantes de greffe validées — c'est là
/// qu'on ajoute une graphie source nouvelle et légitime.
const TA_CITY_SLUGS: &[&str] = &[
    "amiens",
    "bastia",
    "besancon",
    "bordeaux",
    "caen",
    "cergy_pontoise",
    "chalons_en_champagne",
    "clermont_ferrand",
    "dijon",
    "grenoble",
    "guadeloupe",
    "guyane",
    "lille",
    "limoges",
    "lyon",
    "marseille",
    "martinique",
    "mayotte",
    "melun",
    "montpellier",
    "montreuil",
    "nancy",
    "nantes",
    "nice",
    "nimes",
    "nouvelle_caledonie",
    "orleans",
    "paris",
    "pau",
    "poitiers",
    "polynesie_francaise",
    "rennes",
    "reunion",
    "rouen",
    "saint_barthelemy",
    "saint_martin",
    "saint_pierre_et_miquelon",
    "strasbourg",
    "toulon",
    "toulouse",
    "versailles",
    "wallis_et_futuna",
    // variantes de greffe validées
    "st_barthelemy",
    "st_martin",
];

/// A court sitting in every département, named after it.
struct CriminalCourt {
    kind: &'static str,
    prefix: &'static str,
    label: &'static str,
    words: &'static str,
}

/// « Cour criminelle » alone also names the same court during its 2019 to 2022 experiment.
const CRIMINAL_COURTS: [CriminalCourt; 2] = [
    CriminalCourt {
        kind: "ASSISES",
        prefix: "assises",
        label: "Cour d'assises",
        words: "cour d assises",
    },
    CriminalCourt {
        kind: "CCD",
        prefix: "ccd",
        label: "Cour criminelle départementale",
        words: "cour criminelle",
    },
];

/// The 101 départements by INSEE code, each with the preposition its courts' names take.
const DEPARTEMENTS: [(&str, &str); 101] = [
    ("01", "de l'"),
    ("02", "de l'"),
    ("03", "de l'"),
    ("04", "des "),
    ("05", "des "),
    ("06", "des "),
    ("07", "de l'"),
    ("08", "des "),
    ("09", "de l'"),
    ("10", "de l'"),
    ("11", "de l'"),
    ("12", "de l'"),
    ("13", "des "),
    ("14", "du "),
    ("15", "du "),
    ("16", "de la "),
    ("17", "de la "),
    ("18", "du "),
    ("19", "de la "),
    ("2A", "de la "),
    ("2B", "de la "),
    ("21", "de la "),
    ("22", "des "),
    ("23", "de la "),
    ("24", "de la "),
    ("25", "du "),
    ("26", "de la "),
    ("27", "de l'"),
    ("28", "d'"),
    ("29", "du "),
    ("30", "du "),
    ("31", "de la "),
    ("32", "du "),
    ("33", "de la "),
    ("34", "de l'"),
    ("35", "d'"),
    ("36", "de l'"),
    ("37", "d'"),
    ("38", "de l'"),
    ("39", "du "),
    ("40", "des "),
    ("41", "de "),
    ("42", "de la "),
    ("43", "de la "),
    ("44", "de la "),
    ("45", "du "),
    ("46", "du "),
    ("47", "de "),
    ("48", "de la "),
    ("49", "de "),
    ("50", "de la "),
    ("51", "de la "),
    ("52", "de la "),
    ("53", "de la "),
    ("54", "de "),
    ("55", "de la "),
    ("56", "du "),
    ("57", "de la "),
    ("58", "de la "),
    ("59", "du "),
    ("60", "de l'"),
    ("61", "de l'"),
    ("62", "du "),
    ("63", "du "),
    ("64", "des "),
    ("65", "des "),
    ("66", "des "),
    ("67", "du "),
    ("68", "du "),
    ("69", "du "),
    ("70", "de la "),
    ("71", "de "),
    ("72", "de la "),
    ("73", "de la "),
    ("74", "de la "),
    ("75", "de "),
    ("76", "de la "),
    ("77", "de "),
    ("78", "des "),
    ("79", "des "),
    ("80", "de la "),
    ("81", "du "),
    ("82", "de "),
    ("83", "du "),
    ("84", "du "),
    ("85", "de la "),
    ("86", "de la "),
    ("87", "de la "),
    ("88", "des "),
    ("89", "de l'"),
    ("90", "du "),
    ("91", "de l'"),
    ("92", "des "),
    ("93", "de la "),
    ("94", "du "),
    ("95", "du "),
    ("971", "de la "),
    ("972", "de la "),
    ("973", "de la "),
    ("974", "de "),
    ("976", "de "),
];

/// La Réunion drops its article, which a court's « de la » already reads.
fn departement_key(name: &str) -> String {
    let key = lj_fold::slug(name, ' ');
    match key.strip_prefix("la ") {
        Some(rest) => rest.to_string(),
        None => key,
    }
}

/// Longest first, so that « loire atlantique » reads before « loire ».
fn departement_keys() -> &'static [(Vec<String>, &'static str, &'static str)] {
    static KEYS: std::sync::OnceLock<Vec<(Vec<String>, &str, &str)>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys: Vec<_> = DEPARTEMENTS
            .iter()
            .map(|&(code, preposition)| {
                let name = departement_name(code);
                let words: Vec<String> = departement_key(name)
                    .split(' ')
                    .map(str::to_string)
                    .collect();
                (words, code, preposition)
            })
            .collect();
        keys.sort_by_key(|(words, _, _)| std::cmp::Reverse(words.len()));
        keys
    })
}

fn departement_name(code: &str) -> &'static str {
    lj_norm::referential_labels::departement_label(code)
        .unwrap_or_else(|| panic!("a département INSEE names: {code}"))
}

fn criminal_court_ref(court: &CriminalCourt, code: &str, preposition: &str) -> JurisdictionRef {
    let name = departement_name(code);
    JurisdictionRef {
        code: format!(
            "{}_{}",
            court.prefix,
            departement_key(name).replace(' ', "_")
        ),
        jurisdiction_type: court.kind.to_string(),
        city: Some(name.to_string()),
        label: format!("{} {preposition}{name}", court.label),
    }
}

/// The first département a preposition opens after the court's words, whatever qualifies the
/// court around them: « La cour d'assises d'appel de Paris statuant en appel ».
fn criminal_court(jurisdiction_type: &str, name: &str) -> Option<JurisdictionRef> {
    let court = CRIMINAL_COURTS
        .iter()
        .find(|c| c.kind == jurisdiction_type)?;
    let folded = format!(" {} ", lj_fold::slug(name, ' '));
    let at = folded.find(&format!(" {} ", court.words))? + court.words.len() + 1;
    let rest: Vec<&str> = folded[at..].split_whitespace().collect();
    (0..rest.len())
        .filter(|&i| matches!(rest[i], "de" | "du" | "des" | "d"))
        .find_map(|i| {
            let mut words = &rest[i + 1..];
            if rest[i] == "de" && matches!(words.first(), Some(&"la" | &"l")) {
                words = &words[1..];
            }
            departement_keys()
                .iter()
                .find(|(key, _, _)| {
                    key.len() <= words.len() && key.iter().zip(words).all(|(k, w)| k == w)
                })
                .map(|&(_, code, preposition)| criminal_court_ref(court, code, preposition))
        })
}

/// Every cour d'assises and cour criminelle départementale, as the referential holds them.
pub fn criminal_courts() -> Vec<JurisdictionRef> {
    CRIMINAL_COURTS
        .iter()
        .flat_map(|court| {
            DEPARTEMENTS
                .iter()
                .map(move |&(code, preposition)| criminal_court_ref(court, code, preposition))
        })
        .collect()
}

/// Libellé CAA depuis le code cour du numéro de requête (« 12BX02667 » →
/// « Cour administrative d'appel de Bordeaux ») : les neuf codes officiels,
/// pour les décisions anciennes sans nom de juridiction dans le texte.
pub fn caa_label_from_docket(docket: &str) -> Option<&'static str> {
    let code: String = docket.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    Some(match code.as_str() {
        "BX" => "Cour administrative d'appel de Bordeaux",
        "DA" => "Cour administrative d'appel de Douai",
        "LY" => "Cour administrative d'appel de Lyon",
        "MA" => "Cour administrative d'appel de Marseille",
        "NC" => "Cour administrative d'appel de Nancy",
        "NT" => "Cour administrative d'appel de Nantes",
        "PA" => "Cour administrative d'appel de Paris",
        "VE" => "Cour administrative d'appel de Versailles",
        "TL" => "Cour administrative d'appel de Toulouse",
        _ => return None,
    })
}

/// Localisation depuis `canonical_ref` (backfill) : `tj|tj76351|rg|date`,
/// `ca|ca basse terre|rg|date`, `tcom|7801|rg|date`.
pub fn location_from_canonical_ref(jurisdiction_type: &str, canonical_ref: &str) -> Option<String> {
    if !matches!(jurisdiction_type, "TJ" | "CA" | "TCOM") {
        return None;
    }
    let mut parts = canonical_ref.split('|');
    let _type = parts.next()?;
    let loc = parts.next()?.trim();
    if loc.is_empty() {
        None
    } else {
        Some(loc.to_owned())
    }
}

/// (préfixe minuscule à matcher, forme canonique affichable). Le label stocké
/// dans `jurisdiction` est TOUJOURS reconstruit `<forme canonique>
/// <complément>` — une seule façon d'écrire, quelle que soit la casse source.
const NAME_PREFIXES: &[(&str, &str)] = &[
    ("tribunal judiciaire", "Tribunal judiciaire"),
    ("tribunal de grande instance", "Tribunal de grande instance"),
    (
        "tribunal des activités économiques",
        "Tribunal des activités économiques",
    ),
    ("tribunal mixte de commerce", "Tribunal mixte de commerce"),
    ("tribunal de commerce", "Tribunal de commerce"),
    // Papeete écrit les deux formes ; c'est la même juridiction, canonisée sur
    // la désignation officielle — celle que Nouméa emploie seule.
    (
        "tribunal civil de première instance",
        "Tribunal de première instance",
    ),
    (
        "tribunal de première instance",
        "Tribunal de première instance",
    ),
    ("tribunal administratif", "Tribunal administratif"),
    ("cour administrative d'appel", "Cour administrative d'appel"),
    ("cour d'appel", "Cour d'appel"),
    ("tribunal de proximité", "Tribunal de proximité"),
    ("conseil de prud'hommes", "Conseil de prud'hommes"),
];

/// Scinde un libellé source en (type canonique, complément géographique
/// verbatim, préposition comprise) : « Tribunal judiciaire du Havre » →
/// `("Tribunal judiciaire", "du Havre")`.
fn split_name(name: &str) -> Option<(&'static str, &str)> {
    let lower = name.to_lowercase();
    let (prefix, canon) = NAME_PREFIXES.iter().find(|(p, _)| lower.starts_with(p))?;
    Some((canon, name[prefix.len()..].trim()))
}

/// Libellé canonique reconstruit depuis un libellé source. `None` si le type
/// n'est pas reconnu (on retombera sur [`type_label`]).
fn canonical_label(name: &str) -> Option<String> {
    let (canon, rest) = split_name(name)?;
    Some(if rest.is_empty() {
        canon.to_owned()
    } else {
        format!("{canon} {rest}")
    })
}

/// Libellé canonique nu du type (libellé source absent ou méconnaissable) —
/// la ligne naît sans ville et guérit dès qu'une décision du même code en
/// porte une (`ensure_jurisdictions` upgrade).
fn type_label(jurisdiction_type: &str) -> Option<&'static str> {
    Some(match jurisdiction_type {
        "TJ" => "Tribunal judiciaire",
        "CA" => "Cour d'appel",
        "TCOM" => "Tribunal de commerce",
        "CPH" => "Conseil de prud'hommes",
        "TA" => "Tribunal administratif",
        "CAA" => "Cour administrative d'appel",
        _ => return None,
    })
}

/// Ville depuis un libellé de juridiction (« Tribunal judiciaire du Havre » →
/// « Le Havre » n'est PAS reconstitué : on garde le complément tel quel,
/// « du Havre » → « Havre » serait faux ; on strippe seulement le type +
/// la préposition et on garde la forme source).
fn city_from_name(name: &str) -> Option<String> {
    let (_, rest) = split_name(name)?;
    let lower = rest.to_lowercase();
    let rest = ["de la ", "de ", "d'", "du ", "des "]
        .iter()
        .find_map(|p| lower.starts_with(p).then(|| rest[p.len()..].trim()))
        .unwrap_or(rest);
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_owned())
    }
}

/// Normalise un code de localisation : minuscules, espaces → `_`.
fn normalize_location(loc: &str) -> String {
    lj_fold::slug(loc, '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn jurisdiction_ref_grains() {
        let j =
            jurisdiction_ref("TJ", Some("tj76351"), Some("Tribunal judiciaire du Havre")).unwrap();
        assert_eq!(j.code, "tj76351");
        assert_eq!(j.city.as_deref(), Some("Havre"));
        assert_eq!(j.label, "Tribunal judiciaire du Havre");

        let j =
            jurisdiction_ref("TCOM", Some("7801"), Some("Tribunal de commerce d'Evry")).unwrap();
        assert_eq!(j.code, "tcom7801");
        assert_eq!(j.city.as_deref(), Some("Evry"));

        let j = jurisdiction_ref(
            "CA",
            Some("ca basse terre"),
            Some("Cour d'appel de Basse-Terre"),
        )
        .unwrap();
        assert_eq!(j.code, "ca_basse_terre");

        let j = jurisdiction_ref("CC", None, Some("Cour de cassation")).unwrap();
        assert_eq!(j.code, "cc");
        assert_eq!(j.label, "Cour de cassation");
        assert_eq!(j.city, None);

        let j = jurisdiction_ref("TA", None, Some("Tribunal administratif de Melun")).unwrap();
        assert_eq!(j.code, "ta_melun");
        assert_eq!(jurisdiction_ref("TA", None, None), None);

        let j = jurisdiction_ref("CE", None, Some("Conseil d'Etat")).unwrap();
        assert_eq!(j.code, "ce");
        assert_eq!(j.label, "Conseil d'État");

        let j = jurisdiction_ref("CJUE", None, None).unwrap();
        assert_eq!(j.code, "cjue");
        assert_eq!(j.label, "Cour de justice de l'Union européenne");
        assert_eq!(j.city, None);
        let j = jurisdiction_ref("CEDH", None, None).unwrap();
        assert_eq!(j.code, "cedh");
        assert_eq!(j.label, "Cour européenne des droits de l'homme");
    }

    #[test]
    fn the_central_social_aid_commission_is_placed_by_its_name() {
        let j = jurisdiction_ref("CAA", None, Some("Commission centrale d’aide sociale")).unwrap();
        assert_eq!(j.code, "ccas");
        assert_eq!(j.label, "Commission centrale d'aide sociale");
    }

    #[test]
    fn a_criminal_trial_court_is_placed_by_the_departement_its_heading_names() {
        let code = |kind, name| jurisdiction_ref(kind, None, Some(name)).map(|j| j.code);
        for (kind, name, expected) in [
            (
                "ASSISES",
                "Cour d'assises de Paris statuant en appel",
                "assises_paris",
            ),
            (
                "ASSISES",
                "La cour d'assises d'appel de Paris statuant en appel a été convaincue",
                "assises_paris",
            ),
            (
                "ASSISES",
                "EXTRAIT DES MINUTES DU GREFFE DE LA COUR D'ASSISES DE PARIS",
                "assises_paris",
            ),
            (
                "ASSISES",
                "Cour d'assises spécialement composée de Paris",
                "assises_paris",
            ),
            (
                "ASSISES",
                "COUR D'ASSISES DE LA HAUTE-GARONNE",
                "assises_haute_garonne",
            ),
            ("ASSISES", "Cour d'assises du Rhône", "assises_rhone"),
            (
                "ASSISES",
                "cour d'assises de Seine-Saint-Denis",
                "assises_seine_saint_denis",
            ),
            ("ASSISES", "COUR D'ASSISES DE LA REUNION", "assises_reunion"),
            ("ASSISES", "Cour d'assises de l'Eure", "assises_eure"),
            (
                "ASSISES",
                "Cour d'assises d'Eure-et-Loir",
                "assises_eure_et_loir",
            ),
            (
                "CCD",
                "COUR CRIMINELLE DÉPARTEMENTALE DE VAUCLUSE",
                "ccd_vaucluse",
            ),
            (
                "CCD",
                "Cour criminelle départementale du Vaucluse",
                "ccd_vaucluse",
            ),
            (
                "CCD",
                "Cour criminelle de la Seine-Maritime",
                "ccd_seine_maritime",
            ),
        ] {
            assert_eq!(code(kind, name).as_deref(), Some(expected), "{name}");
        }
        assert_eq!(code("ASSISES", "Cour d'assises de la Seine"), None);
        assert_eq!(
            code("ASSISES", "Cour criminelle départementale du Var"),
            None
        );
        let paris = jurisdiction_ref("ASSISES", None, Some("COUR D'ASSISES DE PARIS")).unwrap();
        assert_eq!(paris.label, "Cour d'assises de Paris");
        assert_eq!(paris.city.as_deref(), Some("Paris"));
    }

    #[test]
    fn every_criminal_trial_court_reads_back_its_own_label() {
        let courts = criminal_courts();
        assert_eq!(courts.len(), 202);
        let codes: BTreeSet<&str> = courts.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes.len(), courts.len());
        for court in &courts {
            assert_eq!(
                jurisdiction_ref(&court.jurisdiction_type, None, Some(&court.label)).as_ref(),
                Some(court),
            );
        }
    }

    #[test]
    fn each_authority_the_defenseur_des_droits_succeeded_keeps_its_code() {
        let code = |name| jurisdiction_ref("DDD", None, name).unwrap().code;
        assert_eq!(code(Some("Défenseur des droits")), "ddd");
        assert_eq!(code(None), "ddd");
        assert_eq!(
            code(Some(
                "Haute autorité de lutte contre les discriminations et pour l'égalité"
            )),
            "halde"
        );
        assert_eq!(
            code(Some("Commission nationale de déontologie de la sécurité")),
            "cnds"
        );
        assert_eq!(code(Some("Défenseur des enfants")), "defenseur_enfants");
        assert_eq!(
            code(Some("Médiateur de la République")),
            "mediateur_republique"
        );
    }

    /// Un nom de greffe corrompu ne fabrique pas de code fantôme.
    #[test]
    fn caa_city_outside_closed_set_yields_none() {
        assert_eq!(
            jurisdiction_ref(
                "CAA",
                None,
                Some("Cour administrative d'appel de Versailless"),
            ),
            None
        );
        assert_eq!(
            jurisdiction_ref(
                "CAA",
                None,
                Some("Cour administrative d'appel de Montpellier"),
            ),
            None
        );
        let j = jurisdiction_ref(
            "CAA",
            None,
            Some("Cour administrative d'appel de Versailles"),
        )
        .unwrap();
        assert_eq!(j.code, "caa_versailles");
    }

    /// 42 cours (art. R. 221-3 CJA) : ville inconnue → `None`, la décision
    /// s'ingère sans facette ; l'article de tête et les variantes de greffe validées
    /// passent.
    #[test]
    fn ta_city_outside_closed_set_yields_none() {
        assert_eq!(
            jurisdiction_ref("TA", None, Some("Tribunal administratif de Bidonville")),
            None
        );
        let j = jurisdiction_ref("TA", None, Some("Tribunal administratif de La Réunion")).unwrap();
        assert_eq!(j.code, "ta_reunion");
        let j =
            jurisdiction_ref("TA", None, Some("Tribunal administratif de St Barthélemy")).unwrap();
        assert_eq!(j.code, "ta_st_barthelemy");
        let j = jurisdiction_ref(
            "TA",
            None,
            Some("Tribunal administratif de Saint-Barthélemy"),
        )
        .unwrap();
        assert_eq!(j.code, "ta_saint_barthelemy");
    }

    /// Le label est reconstruit canoniquement, jamais recopié brut du libellé
    /// source.
    #[test]
    fn labels_are_canonical() {
        let j =
            jurisdiction_ref("TJ", Some("tj76351"), Some("TRIBUNAL JUDICIAIRE du Havre")).unwrap();
        assert_eq!(j.label, "Tribunal judiciaire du Havre");

        // Libellé source nu : label nu canonique, la ville arrivera d'une décision
        // sœur du même code (ensure upgrade).
        let j = jurisdiction_ref("TJ", Some("tj76351"), Some("Tribunal judiciaire")).unwrap();
        assert_eq!(j.label, "Tribunal judiciaire");
        assert_eq!(j.city, None);

        // Libellé absent → label nu du type, jamais le code brut.
        let j = jurisdiction_ref("CA", Some("ca paris"), None).unwrap();
        assert_eq!(j.label, "Cour d'appel");

        // Le TAE garde sa dénomination propre.
        let j = jurisdiction_ref(
            "TCOM",
            Some("7501"),
            Some("Tribunal des activités économiques de Paris"),
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal des activités économiques de Paris");
        assert_eq!(j.city.as_deref(), Some("Paris"));
    }

    /// Overseas forms are recognised: the label keeps the court's own name and city.
    #[test]
    fn overseas_forms_keep_their_city() {
        let j = jurisdiction_ref(
            "TJ",
            Some("tj98735"),
            Some("Tribunal civil de première instance de Papeete"),
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal de première instance de Papeete");
        assert_eq!(j.city.as_deref(), Some("Papeete"));

        let j = jurisdiction_ref(
            "TJ",
            Some("tj98818"),
            Some("Tribunal de première instance de Nouméa"),
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal de première instance de Nouméa");
        assert_eq!(j.city.as_deref(), Some("Nouméa"));

        let j = jurisdiction_ref(
            "TCOM",
            Some("9721"),
            Some("Tribunal mixte de commerce de Fort-de-France"),
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal mixte de commerce de Fort-de-France");
        assert_eq!(j.city.as_deref(), Some("Fort-de-France"));
    }

    #[test]
    fn location_from_canonical_ref_variants() {
        assert_eq!(
            location_from_canonical_ref("TJ", "tj|tj76351|25 00006|2025-05-16").as_deref(),
            Some("tj76351")
        );
        assert_eq!(
            location_from_canonical_ref("CA", "ca|ca basse terre|16 00641|2018-03-12").as_deref(),
            Some("ca basse terre")
        );
        assert_eq!(
            location_from_canonical_ref("TCOM", "tcom|7801|2024f00448|2025-02-18").as_deref(),
            Some("7801")
        );
        assert_eq!(
            location_from_canonical_ref("CC", "cc|97-86.457|1998-12-15"),
            None
        );
    }

    /// The `facet_value` seed as the migrations leave it: every uid inserted,
    /// minus what a later one deletes, by uid, by prefix or by whole facet. A
    /// seeded row is immutable, so a namespace renamed in base is renamed here.
    fn seeded_uids() -> BTreeSet<String> {
        let rename = |uid: &str| -> String {
            for (old, new) in [
                ("juridiction:", "jurisdiction_type:"),
                ("domaine:", "legal_domain:"),
                ("chambre:", "matiere:"),
                ("voie:", "procedure:"),
            ] {
                if let Some(suffix) = uid.strip_prefix(old) {
                    return format!("{new}{suffix}");
                }
            }
            uid.to_string()
        };
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../lj-store/migrations"
        ))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
        files.sort();
        let mut seed: BTreeSet<String> = BTreeSet::new();
        for file in files {
            let read = std::fs::read_to_string(&file).unwrap();
            // Comments out first: an apostrophe in the prose pairs with the
            // quotes of the statement and shifts every literal read below.
            let sql: String = read
                .lines()
                .filter_map(|l| l.split("--").next())
                .collect::<Vec<_>>()
                .join("\n");
            for statement in sql.split(';') {
                if statement.contains("INSERT INTO facet_value") {
                    for line in statement.lines() {
                        if let Some(uid) = line
                            .trim_start()
                            .strip_prefix("('")
                            .and_then(|r| r.split('\'').next())
                            .filter(|u| u.contains(':'))
                        {
                            seed.insert(rename(uid));
                        }
                    }
                } else if statement.contains("DELETE FROM facet_value") {
                    for literal in statement.split('\'').skip(1).step_by(2) {
                        let prefix = match literal.contains(':') {
                            true => literal.trim_end_matches('%').to_string(),
                            false => format!("{literal}:"),
                        };
                        seed.retain(|u| !u.starts_with(&prefix));
                    }
                }
            }
        }
        seed
    }

    /// Every uid the scanners can emit exists in the seed, which the
    /// `facet_value` FK requires. The lists below are the emission sites.
    #[test]
    fn emitted_uids_exist_in_seed() {
        let seed = seeded_uids();
        let mut emitted: Vec<String> = Vec::new();
        for s in [
            "REJET",
            "IRRECEVABILITE",
            "CONFIRMATION",
            "INFIRMATION",
            "INFIRMATION_PARTIELLE",
            "NON_LIEU_A_STATUER",
            "SATISFACTION_TOTALE",
            "SATISFACTION_PARTIELLE",
            "CASSATION",
            "CASSATION_PARTIELLE",
            "DESISTEMENT",
            "CONFORMITE",
            "NON_CONFORMITE",
            "INELIGIBILITE",
            "ANNULATION",
            "REFORMATION",
            "VIOLATION",
            "NON_VIOLATION",
            "ANNULATION_PARTIELLE",
            "NON_ADMISSION",
            "DECHEANCE",
            "REGLEMENT_AMIABLE",
            "RADIATION",
            "RECEVABLE",
            "AUTRE",
        ] {
            emitted.push(format!("solution:{s}"));
        }
        for v in [
            "QPC",
            "PAPC",
            "RECOURS_REVISION",
            "FILTRAGE_R222_1",
            "REFERE_LIBERTE",
            "REFERE_MESURES_UTILES",
            "REFERE_PRECONTRACTUEL",
            "REFERE_PROVISION",
            "REFERE_SUSPENSION",
            "REFERE_CIVIL",
            "RECTIFICATION_INTERPRETATION",
            "CHAMBRE_DU_CONSEIL",
        ] {
            emitted.push(format!("procedure:{v}"));
        }
        for o in ["JCP", "JAF", "JLD", "JEX", "PREMIER_PRESIDENT"] {
            emitted.push(format!("office:{o}"));
        }
        for d in [
            "PUBLIC_DROIT_ETRANGERS_NATIONALITE",
            "CIVIL_DROIT_PERSONNES_FAMILLE",
            "SOCIAL_DROIT_AIDE_ACTION_SOCIALE",
            "COMMERCIAL_DROIT_CONSOMMATION",
            "COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE",
        ] {
            emitted.push(format!("legal_domain:{d}"));
        }
        // The formation parse emits from its tables, and the two label tables
        // of lj-core mirror the referential the API renders from.
        for (uid, _) in crate::formation::FORMATION_SEED
            .iter()
            .chain(crate::formation::OFFICE_SEED_EXTRA)
            .chain(lj_core::titles::FORMATION_LABELS)
            .chain(lj_core::titles::OFFICE_LABELS)
        {
            emitted.push((*uid).to_string());
        }

        for uid in &emitted {
            assert!(seed.contains(uid), "uid émis absent du seed : {uid}");
        }
    }
}
