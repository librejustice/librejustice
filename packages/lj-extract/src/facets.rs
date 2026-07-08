//! Référentiels facettes côté extraction (ADR 0146/0148).
//!
//! Depuis v12, les scanners émettent les uids `solution:*`/`voie:*`/`office:*`/
//! `domaine:*` directement aux sites de classification — plus de mapping
//! ancien-monde ici. Restent les dérivations transverses : `publication_uid`
//! (codes source → rang le plus fort) et le référentiel `jurisdiction`
//! (code + ligne à la volée).
//!
//! Chaque uid émis par les scanners existe dans le seed
//! `0100_facet_referentiels.sql` (FK) — le test `emitted_uids_exist_in_seed`
//! le garantit. Valeur inconnue en entrée → `None` (règle #12).

/// Ligne du référentiel `jurisdiction` portée par une décision (le code est
/// la FK de `decisions.jurisdiction_code` ; label/city servent à créer la
/// ligne à la volée, `ON CONFLICT DO NOTHING`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JurisdictionRef {
    pub code: String,
    pub juridiction_type: String,
    pub city: Option<String>,
    pub label: String,
}

/// Codes de publication source (judiciaire minuscules, admin majuscules, cf.
/// `lj-core::publication`) → clé `publication:*`-6, au rang le plus fort.
/// Mapping TOTAL : pas de code → `AUTRE` (toute décision est facettable).
pub fn publication_key(codes: &[String]) -> &'static str {
    let mut best: (u8, &'static str) = (0, "AUTRE");
    for code in codes {
        let (rank, key) = match code.as_str() {
            "A" => (6, "PUBLIE_LEBON"),
            "B" => (5, "MENTIONNE_LEBON"),
            // Signalées : importance au rang des tables (cf. portee_codes).
            "C+" | "R" => (4, "MENTIONNE_LEBON"),
            "C" | "D" | "Z" => (3, "INEDIT_LEBON"),
            "b" | "r" => (6, "PUBLIE_BULLETIN"),
            "n" => (3, "INEDIT_BULLETIN"),
            _ => continue,
        };
        if rank > best.0 {
            best = (rank, key);
        }
    }
    best.1
}

/// Uid complet `publication:*` (mapping TOTAL — toute décision est facettable).
pub fn publication_uid(codes: &[String]) -> String {
    format!("publication:{}", publication_key(codes))
}

// ============================================================
// Référentiel jurisdiction — code + ligne à la volée.
// ============================================================

/// Code + ligne `jurisdiction` d'une décision, depuis ses champs extraits.
///
/// - `location` : code de localisation Judilibre (`tj76351`, `7801` pour un
///   TCOM, slug CA), tel que porté par `Decision::juridiction_location` OU
///   extrait de `canonical_ref` au backfill ([`location_from_canonical_ref`]).
/// - `jurisdiction_name` : libellé extrait (porte la ville quand la source la
///   donne) — le label de la ligne est RECONSTRUIT canoniquement depuis lui
///   ([`canonical_label`]), jamais recopié brut.
/// - `formation_or_chamber` : pour la Cassation, la chambre (grain ADR 0146).
pub fn jurisdiction_ref(
    juridiction_type: &str,
    location: Option<&str>,
    jurisdiction_name: Option<&str>,
    formation_or_chamber: Option<&str>,
) -> Option<JurisdictionRef> {
    let (code, label_override) = match juridiction_type {
        "CC" => cass_code_and_label(formation_or_chamber),
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
        // Cours européennes : lignes de premier rang chez de référence comme chez
        // , au même niveau que CE/CASS — grain juridiction unique (nos
        // colonnes ne portent aucune formation pour elles).
        "CJUE" => (
            "cjue".to_string(),
            Some("Cour de justice de l'Union européenne".to_string()),
        ),
        "CEDH" => (
            "cedh".to_string(),
            Some("Cour européenne des droits de l'homme".to_string()),
        ),
        "TJ" | "CA" | "TCOM" => {
            let loc = normalize_location(location?);
            // Les codes TCOM Judilibre sont des numéros nus → préfixe type.
            let code = if loc.chars().all(|c| c.is_ascii_digit()) {
                format!("{}{loc}", juridiction_type.to_lowercase())
            } else {
                loc
            };
            (code, None)
        }
        "TA" | "CAA" => {
            let city = jurisdiction_name.and_then(city_from_name)?;
            let code = format!("{}_{}", juridiction_type.to_lowercase(), slugify(&city));
            (code, None)
        }
        _ => return None,
    };
    let label = label_override
        .or_else(|| jurisdiction_name.and_then(canonical_label))
        .or_else(|| type_label(juridiction_type).map(str::to_owned))
        .unwrap_or_else(|| code.clone());
    let city = if juridiction_type == "CC" {
        None
    } else {
        city_from_name(&label)
    };
    Some(JurisdictionRef {
        code,
        juridiction_type: juridiction_type.to_owned(),
        city,
        label,
    })
}

/// Localisation depuis `canonical_ref` (backfill) : `tj|tj76351|rg|date`,
/// `ca|ca basse terre|rg|date`, `tcom|7801|rg|date`.
pub fn location_from_canonical_ref(juridiction_type: &str, canonical_ref: &str) -> Option<String> {
    if !matches!(juridiction_type, "TJ" | "CA" | "TCOM") {
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

/// Chambre de la Cour de cassation → (code `cass_*`, label complet). Accepte
/// le code Judilibre brut (`soc`, `cr`, `civ1`…) comme le libellé résolu
/// (« Chambre sociale »). Chambre inconnue / ordonnance → grain `cc`.
fn cass_code_and_label(formation_or_chamber: Option<&str>) -> (String, Option<String>) {
    let f = formation_or_chamber.unwrap_or("").to_lowercase();
    let f = f.trim();
    let hit: Option<(&str, &str)> = if f == "civ1" || f.starts_with("première chambre civile") {
        Some(("cass_civ1", "Cour de cassation, première chambre civile"))
    } else if f == "civ2" || f.starts_with("deuxième chambre civile") {
        Some(("cass_civ2", "Cour de cassation, deuxième chambre civile"))
    } else if f == "civ3" || f.starts_with("troisième chambre civile") {
        Some(("cass_civ3", "Cour de cassation, troisième chambre civile"))
    } else if f == "comm" || f.starts_with("chambre commerciale") {
        Some(("cass_com", "Cour de cassation, chambre commerciale"))
    } else if f == "soc" || f.starts_with("chambre sociale") {
        Some(("cass_soc", "Cour de cassation, chambre sociale"))
    } else if f == "cr" || f.starts_with("chambre criminelle") {
        Some(("cass_crim", "Cour de cassation, chambre criminelle"))
    } else if f == "mi" || f.starts_with("chambre mixte") {
        Some(("cass_mixte", "Cour de cassation, chambre mixte"))
    } else if f == "pl" || f.starts_with("assemblée plénière") {
        Some(("cass_pleniere", "Cour de cassation, assemblée plénière"))
    } else {
        None
    };
    match hit {
        Some((code, label)) => (code.to_owned(), Some(label.to_owned())),
        None => ("cc".to_owned(), Some("Cour de cassation".to_owned())),
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
    ("tribunal de commerce", "Tribunal de commerce"),
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
fn type_label(juridiction_type: &str) -> Option<&'static str> {
    Some(match juridiction_type {
        "TJ" => "Tribunal judiciaire",
        "CA" => "Cour d'appel",
        "TCOM" => "Tribunal de commerce",
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
    slugify(loc)
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                deaccent(c)
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn deaccent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' => 'i',
        'ô' | 'ö' => 'o',
        'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn publication_ranking() {
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(publication_key(&c(&["b", "r"])), "PUBLIE_BULLETIN");
        assert_eq!(publication_key(&c(&["n"])), "INEDIT_BULLETIN");
        assert_eq!(publication_key(&c(&["C", "B"])), "MENTIONNE_LEBON");
        assert_eq!(publication_key(&c(&["A", "B"])), "PUBLIE_LEBON");
        assert_eq!(publication_key(&c(&["C+"])), "MENTIONNE_LEBON");
        assert_eq!(publication_key(&[]), "AUTRE");
        assert_eq!(publication_key(&c(&["xyz"])), "AUTRE");
    }

    #[test]
    fn jurisdiction_ref_grains() {
        // TJ : code Judilibre + ville depuis le nom.
        let j = jurisdiction_ref(
            "TJ",
            Some("tj76351"),
            Some("Tribunal judiciaire du Havre"),
            None,
        )
        .unwrap();
        assert_eq!(j.code, "tj76351");
        assert_eq!(j.city.as_deref(), Some("Havre"));
        assert_eq!(j.label, "Tribunal judiciaire du Havre");

        // TCOM : code numérique nu → préfixé.
        let j = jurisdiction_ref(
            "TCOM",
            Some("7801"),
            Some("Tribunal de commerce d'Evry"),
            None,
        )
        .unwrap();
        assert_eq!(j.code, "tcom7801");
        assert_eq!(j.city.as_deref(), Some("Evry"));

        // CA : slug espaces → underscores.
        let j = jurisdiction_ref(
            "CA",
            Some("ca basse terre"),
            Some("Cour d'appel de Basse-Terre"),
            None,
        )
        .unwrap();
        assert_eq!(j.code, "ca_basse_terre");

        // CASS : grain chambre, code + label résolus, city NULL.
        let j = jurisdiction_ref(
            "CC",
            None,
            Some("Cour de cassation"),
            Some("Chambre sociale"),
        )
        .unwrap();
        assert_eq!(j.code, "cass_soc");
        assert_eq!(j.label, "Cour de cassation, chambre sociale");
        assert_eq!(j.city, None);
        let j = jurisdiction_ref("CC", None, None, Some("soc")).unwrap();
        assert_eq!(j.code, "cass_soc");
        // Chambre inconnue → grain juridiction.
        let j = jurisdiction_ref("CC", None, None, Some("ordo")).unwrap();
        assert_eq!(j.code, "cc");

        // TA : ville depuis le nom.
        let j =
            jurisdiction_ref("TA", None, Some("Tribunal administratif de Melun"), None).unwrap();
        assert_eq!(j.code, "ta_melun");
        // TA sans ville dans le nom → pas de code bancal.
        assert_eq!(jurisdiction_ref("TA", None, None, None), None);

        // CE : unique.
        let j = jurisdiction_ref("CE", None, Some("Conseil d'Etat"), None).unwrap();
        assert_eq!(j.code, "ce");
        assert_eq!(j.label, "Conseil d'État");

        // Cours européennes : grain juridiction, aucun champ source requis.
        let j = jurisdiction_ref("CJUE", None, None, None).unwrap();
        assert_eq!(j.code, "cjue");
        assert_eq!(j.label, "Cour de justice de l'Union européenne");
        assert_eq!(j.city, None);
        let j = jurisdiction_ref("CEDH", None, None, None).unwrap();
        assert_eq!(j.code, "cedh");
        assert_eq!(j.label, "Cour européenne des droits de l'homme");
    }

    /// Une seule façon d'écrire : le label est reconstruit canoniquement,
    /// jamais recopié brut du libellé source.
    #[test]
    fn labels_are_canonical() {
        // Casse source hétérogène → préfixe type recanonisé.
        let j = jurisdiction_ref(
            "TJ",
            Some("tj76351"),
            Some("TRIBUNAL JUDICIAIRE du Havre"),
            None,
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal judiciaire du Havre");

        // Libellé source nu (213 k cas prod) : label nu canonique, la ville
        // arrivera d'une décision sœur du même code (ensure upgrade).
        let j = jurisdiction_ref("TJ", Some("tj76351"), Some("Tribunal judiciaire"), None).unwrap();
        assert_eq!(j.label, "Tribunal judiciaire");
        assert_eq!(j.city, None);

        // Libellé absent → label nu du type, jamais le code brut.
        let j = jurisdiction_ref("CA", Some("ca paris"), None, None).unwrap();
        assert_eq!(j.label, "Cour d'appel");

        // Renommage 2025 : le TAE garde sa dénomination propre.
        let j = jurisdiction_ref(
            "TCOM",
            Some("7501"),
            Some("Tribunal des activités économiques de Paris"),
            None,
        )
        .unwrap();
        assert_eq!(j.label, "Tribunal des activités économiques de Paris");
        assert_eq!(j.city.as_deref(), Some("Paris"));
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

    /// Chaque uid que les scanners v12 peuvent émettre existe dans le seed
    /// 0100 (FK `facet_value`). Listes = sites d'émission de
    /// `judilibre_outcome/special`, `opendata_outcome` et `cnda`.
    #[test]
    fn emitted_uids_exist_in_seed() {
        let sql = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../lj-store/migrations/0100_facet_referentiels.sql"
        ))
        .unwrap();
        let seed: BTreeSet<String> = sql
            .lines()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix("('")?;
                Some(rest[..rest.find('\'')?].to_string())
            })
            .filter(|u| u.contains(':'))
            .collect();

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
        ] {
            emitted.push(format!("voie:{v}"));
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
            emitted.push(format!("domaine:{d}"));
        }
        for codes in [
            vec!["A".to_string()],
            vec!["B".to_string()],
            vec!["C".to_string()],
            vec!["C+".to_string()],
            vec!["b".to_string()],
            vec!["n".to_string()],
            vec![],
        ] {
            emitted.push(publication_uid(&codes));
        }

        for uid in &emitted {
            assert!(seed.contains(uid), "uid émis absent du seed 0100 : {uid}");
        }
    }
}
