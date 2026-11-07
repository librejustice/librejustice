//! Identité canonique inter-sources : `canonical_ref`.
//!
//! `canonical_ref` est la citation légale d'une décision (cour (+ ville) |
//! numéro/RG propre | date), normalisée — l'identité de repli quand l'ECLI
//! officiel manque, et la clé de pont inter-sources. Pure (aucune I/O, aucune
//! dépendance native) : c'est une fonction du `Decision` déjà parsé + de son
//! extracteur. Sa construction est typée par juridiction, le numéro n'étant pas
//! unique de la même façon partout : voir [`decision_canonical_ref`].
//!
//! `canonical_ref` n'est pas garantie unique (affaires sérielles : même cour, RG
//! et date, décisions distinctes). La clé n'est émise que si ses briques fiables
//! sont présentes, sinon `None` : pas d'identité stable, on ne fusionne pas.

use lj_core::decision::Decision;

/// Normalise un numéro de pourvoi : ne garde que chiffres / tiret / point.
/// `n° 93-83.456` → `93-83.456` ; `N° 17-19.227` → `17-19.227`.
pub fn normalize_pourvoi(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect()
}

/// Vrai si le numéro a la **forme d'un pourvoi** (`NN-NN.NNN`) plutôt que d'un
/// n° d'arrêt brut (`12400531`) : un pourvoi normalisé contient un tiret.
pub(crate) fn looks_like_pourvoi(num: &str) -> bool {
    num.contains('-') && num.chars().any(|c| c.is_ascii_digit())
}

/// Vrai si le RG (normalisé : minuscules) a la forme d'un RG CAA préfixé-cour
/// `AAcc…` — deux chiffres (année), deux lettres (code cour : `ma`=Marseille,
/// `nt`=Nantes, `pa`=Paris…), puis un chiffre. Le préfixe identifie la cour de
/// façon déterministe : le RG est alors auto-suffisant, le nom de juridiction est
/// redondant. Faux pour les RG déchet (`rectification`) ou tronqués, qui
/// collisionneraient entre cours → repli sur la clé par nom.
pub(crate) fn looks_like_caa_rg(rg: &str) -> bool {
    let b = rg.as_bytes();
    b.len() >= 5
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_alphabetic()
        && b[3].is_ascii_alphabetic()
        && b[4].is_ascii_digit()
}

/// Normalise un composant **textuel** de la clé (juridiction) : minuscules,
/// accents repliés, tout caractère non alphanumérique réduit à une espace,
/// espaces collapsés. `Tribunal administratif d'Amiens` → `tribunal
/// administratif d amiens`.
pub(crate) fn normalize_component(s: &str) -> String {
    let folded: String = s
        .chars()
        .flat_map(lj_fold::fold_char)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `canonical_ref` d'une décision : identité dérivée de ce que la décision
/// tranche, auto-suffisante (même clé ⟺ même décision), `None` si les
/// discriminants fiables manquent. Distincte de l'`ecli` officiel (résolu en
/// premier), non garantie unique (affaires sérielles).
///
/// Typée par juridiction, le numéro n'étant pas unique de la même façon partout :
/// - Cassation (`CC`) : `cc|<pourvoi minimal>|<date>` — le pourvoi minimal
///   canonicalise les pourvois joints (abrégé `{611,854}` et intégrale `{611}`
///   → `611`), donc abrégé Bulletin et intégrale partagent la clé.
/// - CA / TJ / TCOM : `<type>|<location>|<rg>|<date>` — le `location`
///   (code tribunal Judilibre, unique par tribunal) est obligatoire : sans lui
///   le RG collisionne entre tribunaux. Absent → `None`.
/// - CAA : `caa|<rg>|<date>` — le RG porte le code cour → clé indépendante du
///   nom ; repli `<nom>|<rg>|<date>` si RG non préfixé-cour.
/// - Admin TA/CE : `<nom juridiction (porte la ville)>|<rg>|<date>`.
/// - Autres (CONSTIT/TC/CEDH/CJUE/CNDA…) : `<type>|<numéro>|<date>`, sources
///   sans recoupement.
pub fn decision_canonical_ref(d: &Decision) -> Option<String> {
    let jt = d.jurisdiction_type.as_deref()?;
    let date = crate::extract::extract_date_lecture(d).or_else(|| d.date_lecture.clone())?;
    let date = date.trim();
    if date.is_empty() {
        return None;
    }
    let dockets = crate::extract::extract_docket_numbers(d).unwrap_or_default();

    match jt {
        "CC" => {
            // Pourvoi minimal (déterministe) parmi les pourvois jugés.
            let pourvoi = dockets
                .iter()
                .map(|n| normalize_pourvoi(n))
                .filter(|n| looks_like_pourvoi(n))
                .min()?;
            Some(format!("cc|{pourvoi}|{date}"))
        }
        "CA" | "TJ" | "TCOM" | "CPH" => {
            // RG scopé au tribunal : la `location` (code Judilibre) est requise.
            let location = d
                .jurisdiction_location
                .as_deref()
                .map(normalize_component)?;
            let rg = dockets.first().map(|n| normalize_component(n))?;
            if location.is_empty() || rg.is_empty() {
                return None;
            }
            Some(format!("{}|{location}|{rg}|{date}", jt.to_lowercase()))
        }
        "CAA" => {
            // Le RG CAA porte le code cour (23**MA**01123 = Marseille) → clé
            // auto-suffisante `caa|<rg>|<date>`, indépendante du nom de juridiction (que
            // l'opendata place parfois à un placeholder « CAA »). On n'émet la clé RG que
            // si le RG est au format préfixé-cour (`looks_like_caa_rg`) : un RG déchet
            // (« rectification ») collisionnerait entre cours → repli sur le nom, qui porte
            // la ville.
            let rg = dockets
                .first()
                .map(|n| normalize_component(n))
                .filter(|r| !r.is_empty())?;
            if looks_like_caa_rg(&rg) {
                return Some(format!("caa|{rg}|{date}"));
            }
            let jur = crate::extract::extract_jurisdiction_name(d)
                .or_else(|| d.jurisdiction_name.clone())
                .map(|s| normalize_component(&s))
                .filter(|j| !j.is_empty())?;
            Some(format!("{jur}|{rg}|{date}"))
        }
        "TA" | "CE" => {
            // Admin TA/CE : la ville vit dans le nom de juridiction (le RG n'est pas
            // préfixé-cour — TA mono-source opendata ; CE national mono-cour).
            let jur = crate::extract::extract_jurisdiction_name(d)
                .or_else(|| d.jurisdiction_name.clone())
                .map(|s| normalize_component(&s))?;
            let rg = dockets.first().map(|n| normalize_component(n))?;
            if jur.is_empty() || rg.is_empty() {
                return None;
            }
            Some(format!("{jur}|{rg}|{date}"))
        }
        _ => {
            // Sources sans recoupement : type|numéro|date.
            let numero = dockets.first().map(|n| normalize_component(n))?;
            if numero.is_empty() {
                return None;
            }
            Some(format!("{}|{numero}|{date}", normalize_component(jt)))
        }
    }
}

/// The dockets an arriving target answers to, the probe a batch of freshly
/// written decisions runs against the pending addresses.
///
/// No per-court reformatting, unlike the family keys this replaces: both sides
/// of the join fold through `lj_docket_keys`, so `18-23.954` already meets
/// `1823954`. Two graphies folding does not merge are added here, the eras of a
/// Tribunal des conflits number and the CJUE role a citation writes bare.
pub fn pending_target_dockets(
    jurisdiction_code: Option<&str>,
    docket_numbers: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for docket in docket_numbers {
        let docket = docket.trim();
        if docket.is_empty() {
            continue;
        }
        let other = match jurisdiction_code {
            Some("tc") => Some(
                docket
                    .trim_start_matches(['c', 'C'])
                    .trim_start_matches('0')
                    .to_string(),
            ),
            Some("cjue") => docket.split_once('-').map(|(_, role)| role.to_string()),
            _ => None,
        };
        out.push(docket.to_string());
        out.extend(other.filter(|o| !o.is_empty() && o != docket));
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Decision {
        Decision {
            source_uid: "t".into(),
            member_name: "t".into(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: None,
            jurisdiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: None,
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: vec![],
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: String::new(),
            texte_integral_clean: String::new(),
            sections: vec![],
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }

    #[test]
    fn normalize_pourvoi_strips_noise() {
        assert_eq!(normalize_pourvoi("n° 93-83.456"), "93-83.456");
        assert_eq!(normalize_pourvoi("N°17-19.227"), "17-19.227");
    }

    #[test]
    fn pourvoi_shape_vs_arret_number() {
        assert!(looks_like_pourvoi("17-19.227"));
        assert!(!looks_like_pourvoi("12400531"));
    }

    #[test]
    fn normalize_component_folds_accents_and_punct() {
        assert_eq!(
            normalize_component("Tribunal administratif d'Amiens"),
            "tribunal administratif d amiens"
        );
        assert_eq!(
            normalize_component("Cour d'appel de Lyon"),
            "cour d appel de lyon"
        );
    }

    /// The probe is the decision's own numbers, plus the two graphies folding
    /// leaves apart.
    #[test]
    fn an_arriving_target_probes_by_its_dockets() {
        assert_eq!(
            pending_target_dockets(Some("cc"), &["18-23.954".into()]),
            vec!["18-23.954"]
        );
        assert_eq!(
            pending_target_dockets(Some("tc"), &["C4174".into()]),
            vec!["4174", "C4174"]
        );
        assert_eq!(
            pending_target_dockets(Some("cjue"), &["C-561/19".into()]),
            vec!["561/19", "C-561/19"]
        );
        assert!(pending_target_dockets(Some("cc"), &["  ".into()]).is_empty());
    }

    #[test]
    fn admin_key_uses_docket_and_jurisdiction_name() {
        let mut d = base();
        d.jurisdiction_type = Some("TA".into());
        d.jurisdiction_name = Some("Tribunal administratif d'Amiens".into());
        d.numero_dossier = Some("2204150".into());
        d.date_lecture = Some("2022-08-15".into());
        let key = decision_canonical_ref(&d).unwrap();
        let parts: Vec<&str> = key.split('|').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], "2204150");
        assert_eq!(parts[2], "2022-08-15");
    }

    fn cref(d: &Decision) -> Option<String> {
        decision_canonical_ref(d)
    }

    #[test]
    fn canonical_ref_tj_same_rg_different_location_splits() {
        let mut amiens = base();
        amiens.jurisdiction_type = Some("TJ".into());
        amiens.jurisdiction_location = Some("tj80021".into());
        amiens.numero_dossiers = Some(vec!["26/00051".into()]);
        amiens.date_lecture = Some("2026-01-20".into());
        let mut compiegne = amiens.clone();
        compiegne.jurisdiction_location = Some("tj60159".into());

        assert!(cref(&amiens).is_some());
        assert_ne!(cref(&amiens), cref(&compiegne));
        let repub = amiens.clone();
        assert_eq!(cref(&amiens), cref(&repub));
    }

    #[test]
    fn canonical_ref_tj_without_location_is_none() {
        let mut d = base();
        d.jurisdiction_type = Some("TJ".into());
        d.numero_dossiers = Some(vec!["26/00051".into()]);
        d.date_lecture = Some("2026-01-20".into());
        assert_eq!(cref(&d), None);
    }

    #[test]
    fn canonical_ref_caa_rg_based_bridges_jade_and_opendata() {
        let mut jade = base();
        jade.jurisdiction_type = Some("CAA".into());
        jade.jurisdiction_name = Some("Cour administrative d'appel de Marseille".into());
        jade.numero_dossier = Some("23MA01123".into());
        jade.date_lecture = Some("2024-05-14".into());
        let mut opendata = jade.clone();
        opendata.jurisdiction_name = Some("CAA".into());

        assert_eq!(cref(&jade).unwrap(), "caa|23ma01123|2024-05-14");
        assert_eq!(cref(&jade), cref(&opendata));
    }

    #[test]
    fn canonical_ref_caa_rg_prefix_separates_courts() {
        let mut marseille = base();
        marseille.jurisdiction_type = Some("CAA".into());
        marseille.numero_dossier = Some("24MA00434".into());
        marseille.date_lecture = Some("2024-11-08".into());
        let mut nantes = marseille.clone();
        nantes.numero_dossier = Some("24NT00434".into());
        assert_ne!(cref(&marseille), cref(&nantes));
    }

    #[test]
    fn canonical_ref_caa_junk_rg_falls_back_to_name() {
        let mut d = base();
        d.jurisdiction_type = Some("CAA".into());
        d.jurisdiction_name = Some("Cour administrative d'appel de Lyon".into());
        d.numero_dossier = Some("rectification".into());
        d.date_lecture = Some("2025-03-04".into());
        let key = cref(&d).unwrap();
        assert!(
            !key.starts_with("caa|"),
            "repli attendu, pas la clé RG : {key}"
        );
        assert!(key.contains("|rectification|2025-03-04"));
        let mut noname = d.clone();
        noname.jurisdiction_name = None;
        assert_eq!(cref(&noname), None);
    }

    #[test]
    fn canonical_ref_cassation_joined_pourvois_canonicalize() {
        let mut integrale = base();
        integrale.jurisdiction_type = Some("CC".into());
        integrale.numero_dossiers = Some(vec!["88-14.611".into()]);
        integrale.date_lecture = Some("1990-12-12".into());
        let mut abrege = integrale.clone();
        abrege.numero_dossiers = Some(vec!["88-15.854".into(), "88-14.611".into()]);

        assert_eq!(cref(&integrale), cref(&abrege));
        assert_eq!(cref(&integrale).unwrap(), "cc|88-14.611|1990-12-12");
    }
}
