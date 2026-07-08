// Tests figés (spec) pour l'extracteur Judilibre. Inclus via `mod tests;`.

use super::*;
use crate::extract::{
    extract_formation_or_chamber, extract_jurisdiction_name, extract_procedure, extract_solution,
    ProcedureUids,
};
use lj_core::decision::Decision;

fn decision(jt: &str) -> Decision {
    Decision {
        source_uid: "test".into(),
        member_name: "test".into(),
        ecli: None,
        juridiction_code: None,
        juridiction_nom: None,
        juridiction_type: Some(jt.into()),
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
fn docket_numbers_clean_and_joined_pourvois() {
    let mut d = decision("CC");
    d.numero_dossiers = Some(vec!["00-17.842".into()]);
    d.texte_integral_clean =
        "Joint les pourvois n° A 00-18.032 et n° D 00-18.518 ; Sur le moyen…".into();
    let out = extract_docket_numbers(&d, crate::extract::scan_doc(&d).as_ref()).unwrap();
    assert!(out.contains(&"00-17.842".to_string()));
    assert!(out.contains(&"00-18.032".to_string()));
    assert!(out.contains(&"00-18.518".to_string()));
}

#[test]
fn docket_numbers_range_join_skipped() {
    // jonction par plage « X au n° Y » → non énumérée.
    let mut d = decision("CC");
    d.numero_dossiers = Some(vec!["00-44.843".into()]);
    d.texte_integral_clean = "Joint les pourvois n° T 00-44.843 au n° W 00-44.846 ;".into();
    let out = extract_docket_numbers(&d, crate::extract::scan_doc(&d).as_ref()).unwrap();
    assert_eq!(out, vec!["00-44.843".to_string()]);
}

#[test]
fn jurisdiction_name_from_location_and_tae_rename() {
    let mut d = decision("TCOM");
    d.juridiction_location = Some("7501".into());
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal de commerce de Paris")
    );
    // En-tête « tribunal des activités économiques » → renommage.
    d.texte_integral_clean = "TRIBUNAL DES ACTIVITES ECONOMIQUES DE PARIS".into();
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal des activités économiques de Paris")
    );
}

#[test]
fn formation_or_chamber_cc_keeps_label() {
    let mut d = decision("CC");
    d.juridiction_code = Some("soc".into());
    assert_eq!(
        extract_formation_or_chamber(&d).as_deref(),
        Some("Chambre sociale")
    );
}

#[test]
fn formation_or_chamber_ca_humanizes() {
    let mut d = decision("CA");
    d.juridiction_code = Some("CHAMBRE SOCIALE".into());
    assert_eq!(
        extract_formation_or_chamber(&d).as_deref(),
        Some("Chambre sociale")
    );
}

#[test]
fn formation_or_chamber_body_scan_trims_act_stopword() {
    // Champ `chamber` vide (CA) → scan de l'en-tête du corps via
    // `_RE_BODY_NAMED_CHAMBER`. Le crate `regex` n'a pas le lookahead négatif
    // interne `(?!(?:STOP)\b)` de Python : `trim_named_chamber` doit borner le
    // libellé au premier mot-clé d'acte (« ARRET / DU / LE / N° / ORDONNANCE »)
    // — sinon on capture « Chambre civile arret » au lieu de « Chambre civile ».
    let mut d = decision("CA");
    d.texte_integral_clean =
        "COUR D'APPEL DE BASTIA \n\n CHAMBRE CIVILE \n\n ARRET DU \n TROIS JUILLET".into();
    assert_eq!(
        extract_formation_or_chamber(&d).as_deref(),
        Some("Chambre civile")
    );

    // Connecteur « DES » conservé ; mots non-stop préservés (1..3).
    let mut d2 = decision("CA");
    d2.texte_integral_clean =
        "COUR D'APPEL DE GRENOBLE \n CHAMBRE DES EXPROPRIATIONS \n DU 5 MARS".into();
    assert_eq!(
        extract_formation_or_chamber(&d2).as_deref(),
        Some("Chambre des expropriations")
    );

    // Lettre de section « B » préservée avant la borne d'acte.
    let mut d3 = decision("CA");
    d3.texte_integral_clean = "COUR D'APPEL DE BASTIA \n CHAMBRE CIVILE B \n ARRET DU".into();
    assert_eq!(
        extract_formation_or_chamber(&d3).as_deref(),
        Some("Chambre civile B")
    );

    // Premier mot après « chambre » EST un stopword d'acte → Python renvoie
    // None ({1,3} exige >= 1 mot valide) ; pas de capture « Chambre arret ».
    let mut d4 = decision("CA");
    d4.texte_integral_clean = "COUR D'APPEL DE X \n CHAMBRE \n ARRET no 12".into();
    assert_eq!(extract_formation_or_chamber(&d4).as_deref(), None);
}

#[test]
fn solution_cc_cassation_partielle() {
    let mut d = decision("CC");
    d.solution = Some("cassation".into());
    d.texte_integral_clean = "La société X a formé le pourvoi contre l'arrêt rendu par la \
        cour d'appel. PAR CES MOTIFS : CASSE ET ANNULE, mais seulement en ce qu'il a condamné…"
        .into();
    assert_eq!(
        extract_solution(&d).as_deref(),
        Some("solution:CASSATION_PARTIELLE")
    );
}

#[test]
fn solution_cc_cassation_totale() {
    let mut d = decision("CC");
    d.solution = Some("cassation".into());
    d.texte_integral_clean = "La société X a formé le pourvoi contre l'arrêt rendu par la \
        cour d'appel. PAR CES MOTIFS : CASSE ET ANNULE l'arrêt rendu…"
        .into();
    assert_eq!(extract_solution(&d).as_deref(), Some("solution:CASSATION"));
}

#[test]
fn solution_rejet_maps_directly() {
    let mut d = decision("CA");
    d.solution = Some("rejet".into());
    assert_eq!(extract_solution(&d).as_deref(), Some("solution:REJET"));
}

#[test]
fn solution_cc_rnsm_stays_rejet() {
    // RNSM (art. 1014 CPC) : dispositif « REJETTE », formule de clôture
    // « Ainsi décidé… prononcé par le président » — reste REJET, pas
    // IRRECEVABILITE (tranché par l'utilisateur, audit GT 2026-06-06).
    let mut d = decision("CC");
    d.solution = Some("rejet".into());
    d.texte_integral_clean = "REJETTE le pourvoi. Ainsi décidé par la Cour de cassation, \
        chambre civile, et prononcé par le président en son audience publique."
        .into();
    assert_eq!(extract_solution(&d).as_deref(), Some("solution:REJET"));
}

#[test]
fn solution_cc_non_admis_dispositif() {
    // Dispositif explicite « déclare le pourvoi non admis » : IRRECEVABILITE
    // même si la solution éditoriale dit « rejet ».
    let mut d = decision("CC");
    d.solution = Some("rejet".into());
    d.texte_integral_clean =
        "PAR CES MOTIFS : DÉCLARE le pourvoi NON ADMIS. n'admet pas le pourvoi".into();
    assert_eq!(
        extract_solution(&d).as_deref(),
        Some("solution:IRRECEVABILITE")
    );
}

#[test]
fn solution_ca_confirme_vs_partiel() {
    let mut d = decision("CA");
    d.texte_integral_clean = "Mme X APPELANTE M. Y INTIMÉ \
        PAR CES MOTIFS : Confirme le jugement entrepris."
        .into();
    assert_eq!(
        extract_solution(&d).as_deref(),
        Some("solution:CONFIRMATION")
    );

    let mut d2 = decision("CA");
    d2.texte_integral_clean = "Mme X APPELANTE M. Y INTIMÉ \
        PAR CES MOTIFS : Confirme le jugement sauf en ce qu'il a fixé…"
        .into();
    assert_eq!(
        extract_solution(&d2).as_deref(),
        Some("solution:INFIRMATION_PARTIELLE")
    );
}

#[test]
fn solution_none_when_no_solution_no_signal() {
    let d = decision("CA");
    assert_eq!(extract_solution(&d), None);
}

#[test]
fn procedure_qpc_from_solution() {
    let mut d = decision("CC");
    d.solution = Some("qpc_renvoi".into());
    assert_eq!(extract_procedure(&d).voie_uid.as_deref(), Some("voie:QPC"));
}

#[test]
fn procedure_papc_from_header_non_admission() {
    // « NON-ADMISSION » dans le bandeau d'en-tête, `solution` ne le dit pas
    // (souvent `rejet`/`other`) : doit quand même classer PAPC.
    let mut d = decision("CC");
    d.solution = Some("rejet".into());
    d.texte_integral_clean =
        "N° R 26-80.549 F  N° 50668  15 AVRIL 2026  NON-ADMISSION  M. BONNAL président,".into();
    assert_eq!(extract_procedure(&d).voie_uid.as_deref(), Some("voie:PAPC"));
}

#[test]
fn procedure_papc_from_header_rnsm() {
    // « Rejet non spécialement motivé » (art. 1014 CPC) = même piste de filtrage
    // cassation (PAPC) que la non-admission, mais dispositif « REJETTE ».
    let mut d = decision("CC");
    d.solution = Some("rejet".into());
    d.texte_integral_clean =
        "CIV. 1  COUR DE CASSATION  Rejet non spécialement motivé  Mme CHAMPALAUNE, présidente"
            .into();
    assert_eq!(extract_procedure(&d).voie_uid.as_deref(), Some("voie:PAPC"));
}

#[test]
fn procedure_refere_fallback() {
    let mut d = decision("CA");
    d.juridiction_code = Some("Chambre des référés".into());
    assert_eq!(
        extract_procedure(&d).voie_uid.as_deref(),
        Some("voie:REFERE_CIVIL")
    );
}

#[test]
fn procedure_ordinaire_default() {
    let mut d = decision("CA");
    d.juridiction_code = Some("Première chambre civile".into());
    assert_eq!(extract_procedure(&d), ProcedureUids::default());
}

#[test]
fn procedure_jld_from_chamber() {
    let mut d = decision("TJ");
    d.juridiction_code = Some("J.L.D.".into());
    assert_eq!(
        extract_procedure(&d).office_uid.as_deref(),
        Some("office:JLD")
    );
}

#[test]
fn publication_code_joins_multivalue() {
    let mut d = decision("CC");
    d.publication_codes = vec!["b".into(), "l".into()];
    assert_eq!(extract_publication_code(&d).as_deref(), Some("b,l"));
    let d2 = decision("CC");
    assert_eq!(extract_publication_code(&d2), None);
}

#[test]
fn date_lecture_validates_iso() {
    let mut d = decision("CC");
    d.date_lecture = Some("2021-03-15".into());
    assert_eq!(extract_date_lecture(&d).as_deref(), Some("2021-03-15"));
    d.date_lecture = Some("0201-11-23".into());
    assert_eq!(extract_date_lecture(&d), None);
}

#[test]
fn formation_axes_read_source_chamber_field() {
    // La chambre CA/TJ/TCOM vit dans le champ source `chamber`
    // (juridiction_code, texte libre) — même précédence que la colonne
    // composée : champ source d'abord, bandeau scanné en repli.
    let mut d = decision("TJ");
    d.juridiction_code = Some("CTX PROTECTION SOCIALE".into());
    let axes = crate::extract::formation_axes_scanned(&d, None);
    assert_eq!(axes.chambre_uid, Some("chambre:PROTECTION_SOCIALE"));
    assert_eq!(axes.chamber_position.as_deref(), Some("Protection sociale"));

    let mut d2 = decision("CA");
    d2.juridiction_code = Some("Pôle 1 - Chambre 11".into());
    let axes2 = crate::extract::formation_axes_scanned(&d2, None);
    assert_eq!(
        axes2.chamber_position.as_deref(),
        Some("Pôle 1 — 11e chambre")
    );

    // Champ source vide → repli bandeau (comme la colonne composée).
    let mut d3 = decision("CA");
    d3.texte_integral_clean =
        "COUR D'APPEL DE BASTIA \n\n CHAMBRE CIVILE \n\n ARRET DU \n TROIS JUILLET".into();
    let axes3 = crate::extract::formation_axes_scanned(&d3, crate::extract::scan_doc(&d3).as_ref());
    assert_eq!(axes3.chambre_uid, Some("chambre:CIVILE"));
}
