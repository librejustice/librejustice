//! Cas figés (spec) tirés du comportement Python de `extract/opendata.py`.

use lj_core::decision::Decision;

fn decision(jt: &str, uid: &str) -> Decision {
    Decision {
        source_uid: uid.to_string(),
        member_name: String::new(),
        ecli: None,
        juridiction_code: None,
        juridiction_nom: None,
        juridiction_type: Some(jt.to_string()),
        juridiction_location: None,
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
        texte_integral_raw: String::new(),
        texte_integral_clean: String::new(),
        sections: vec![],
        metadata_header: String::new(),
        visa_trim: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: vec![],
    }
}

#[test]
fn jurisdiction_name_ce_is_conseil_detat() {
    assert_eq!(
        crate::extract::extract_jurisdiction_name(&decision("CE", "DCE_1")).as_deref(),
        Some("Conseil d'État")
    );
}

#[test]
fn jurisdiction_name_ta_title_cases_place() {
    let mut d = decision("TA", "DTA_1");
    d.juridiction_nom = Some("Tribunal Administratif de MELUN".to_string());
    assert_eq!(
        crate::extract::extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal administratif de Melun")
    );
    d.juridiction_nom = Some("Tribunal Administratif de AMIENS".to_string());
    // place commence par voyelle → d' (chemin « de … » conservé tel quel ici).
    assert_eq!(
        crate::extract::extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal administratif de Amiens")
    );
}

#[test]
fn caa_name_full_form_from_abbrev() {
    let mut d = decision("CAA", "DCA_1");
    d.juridiction_nom = Some("CAA de DOUAI".to_string());
    assert_eq!(
        crate::extract::extract_jurisdiction_name(&d).as_deref(),
        Some("Cour administrative d'appel de Douai")
    );
}

#[test]
fn formation_case_keeps_acronyms() {
    let mut d = decision("TA", "DTA_1");
    d.formation = Some("JUGE UNIQUE".to_string());
    assert_eq!(
        crate::extract::extract_formation_or_chamber(&d).as_deref(),
        Some("Juge unique")
    );
    d.formation = Some("1ère CHAMBRE".to_string());
    assert_eq!(
        crate::extract::extract_formation_or_chamber(&d).as_deref(),
        Some("1ère chambre")
    );
    d.formation = Some("JU OQTF 6 semaines".to_string());
    assert_eq!(
        crate::extract::extract_formation_or_chamber(&d).as_deref(),
        Some("JU OQTF 6 semaines")
    );
}

#[test]
fn date_lecture_validates_iso() {
    let mut d = decision("TA", "DTA_1");
    d.date_lecture = Some("2024-03-15".to_string());
    assert_eq!(
        crate::extract::extract_date_lecture(&d).as_deref(),
        Some("2024-03-15")
    );
    d.date_lecture = Some("0201-11-23".to_string());
    assert_eq!(crate::extract::extract_date_lecture(&d), None);
}

#[test]
fn solution_rejet_prefix() {
    let mut d = decision("TA", "DTA_1");
    d.solution = Some("Rejet".to_string());
    assert_eq!(
        crate::extract::extract_solution(&d).as_deref(),
        Some("solution:REJET")
    );
    // école gold : ordre admin au dispositif VERBATIM — un acte annulé est
    // une ANNULATION, jamais une « satisfaction » du requérant
    d.solution = Some("Annulation".to_string());
    assert_eq!(
        crate::extract::extract_solution(&d).as_deref(),
        Some("solution:ANNULATION")
    );
    d.solution = Some("Désistement".to_string());
    assert_eq!(
        crate::extract::extract_solution(&d).as_deref(),
        Some("solution:DESISTEMENT")
    );
}

#[test]
fn solution_irrecevabilite_substring() {
    let mut d = decision("TA", "DTA_1");
    d.solution = Some("Rejet pour irrecevabilité".to_string());
    assert_eq!(
        crate::extract::extract_solution(&d).as_deref(),
        Some("solution:IRRECEVABILITE")
    );
}

#[test]
fn procedure_refere_liberte_from_text() {
    let mut d = decision("TA", "ORTA_1");
    d.texte_integral_clean =
        "Vu la requête présentée au titre de l'article L. 521-2 du code de justice administrative"
            .to_string();
    assert_eq!(
        crate::extract::extract_procedure(&d).voie_uid.as_deref(),
        Some("voie:REFERE_LIBERTE")
    );
}

#[test]
fn procedure_ordinaire_default() {
    use crate::extract::ProcedureUids;
    let d = decision("TA", "DTA_1");
    assert_eq!(
        crate::extract::extract_procedure(&d),
        ProcedureUids::default()
    );
}

#[test]
fn docket_numbers_split() {
    let mut d = decision("CAA", "DCA_1");
    d.numero_dossier = Some("21PA01234".to_string());
    assert_eq!(
        crate::extract::extract_docket_numbers(&d),
        Some(vec!["21PA01234".to_string()])
    );
}

#[test]
fn flat_extractors_route_opendata() {
    let d = decision("CE", "DCE_1");

    assert_eq!(
        crate::extract::extract_jurisdiction_name(&d).as_deref(),
        Some("Conseil d'État")
    );
}

#[test]
fn routed_unknown_errors() {
    let mut d = decision("XX", "ZZ_1");
    d.juridiction_type = Some("XX".to_string());
    assert!(crate::extract::routed(&d).is_err());
}
