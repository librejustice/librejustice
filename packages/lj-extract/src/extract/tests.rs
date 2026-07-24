// Tests figés (spec) des extracteurs unifiés — cas tirés du comportement
// Python historique (opendata.py / judilibre.py), portés sur les fonctions
// plates de `extract`.

use crate::extract::{
    docket_numbers_scanned, extract_date_lecture, extract_docket_numbers,
    extract_jurisdiction_name, extract_procedure, extract_publication_code, extract_solution,
    ProcedureUids,
};
use lj_core::decision::Decision;

fn decision(jt: &str) -> Decision {
    Decision {
        source_uid: "test".into(),
        member_name: "test".into(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: None,
        jurisdiction_type: Some(jt.into()),
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

// ───────────────────────────── numéros de dossier ───────────────────────────

#[test]
fn docket_numbers_clean_and_joined_pourvois() {
    let mut d = decision("CC");
    d.numero_dossiers = Some(vec!["00-17.842".into()]);
    d.texte_integral_clean =
        "Joint les pourvois n° A 00-18.032 et n° D 00-18.518 ; Sur le moyen…".into();
    let out = docket_numbers_scanned(&d, crate::extract::scan_doc(&d).as_ref()).unwrap();
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
    let out = docket_numbers_scanned(&d, crate::extract::scan_doc(&d).as_ref()).unwrap();
    assert_eq!(out, vec!["00-44.843".to_string()]);
}

#[test]
fn docket_numbers_split() {
    let mut d = decision("CAA");
    d.numero_dossier = Some("21PA01234".to_string());
    assert_eq!(
        extract_docket_numbers(&d),
        Some(vec!["21PA01234".to_string()])
    );
}

// ───────────────────────────── nom de juridiction ───────────────────────────

#[test]
fn jurisdiction_name_from_location_and_tae_rename() {
    let mut d = decision("TCOM");
    d.jurisdiction_location = Some("7501".into());
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
fn jurisdiction_name_ce_is_conseil_detat() {
    assert_eq!(
        extract_jurisdiction_name(&decision("CE")).as_deref(),
        Some("Conseil d'État")
    );
}

#[test]
fn jurisdiction_name_ta_title_cases_place() {
    let mut d = decision("TA");
    d.jurisdiction_name = Some("Tribunal Administratif de MELUN".to_string());
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal administratif de Melun")
    );
    d.jurisdiction_name = Some("Tribunal Administratif de AMIENS".to_string());
    // place commence par voyelle → d' (chemin « de … » conservé tel quel ici).
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal administratif de Amiens")
    );
    // Apostrophe perdue par le greffe (866 décisions prod « de d Amiens »).
    d.jurisdiction_name = Some("Tribunal Administratif d Amiens".to_string());
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Tribunal administratif d'Amiens")
    );
}

#[test]
fn caa_name_full_form_from_abbrev() {
    let mut d = decision("CAA");
    d.jurisdiction_name = Some("CAA de DOUAI".to_string());
    assert_eq!(
        extract_jurisdiction_name(&d).as_deref(),
        Some("Cour administrative d'appel de Douai")
    );
}

// ─────────────────────────── formation / chambre ────────────────────────────

#[test]
fn formation_axes_read_source_chamber_field() {
    // La chambre CA/TJ/TCOM vit dans le champ source `chamber` (texte libre)
    // — même précédence que la colonne composée : champ source d'abord,
    // bandeau scanné en repli.
    let mut d = decision("TJ");
    d.chamber = Some("CTX PROTECTION SOCIALE".into());
    let axes = crate::extract::formation_axes_scanned(&d, None);
    assert_eq!(axes.chamber_uid, Some("chamber:PROTECTION_SOCIALE"));
    assert_eq!(axes.chamber_position.as_deref(), Some("Protection sociale"));

    let mut d2 = decision("CA");
    d2.chamber = Some("Pôle 1 - Chambre 11".into());
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
    assert_eq!(axes3.chamber_uid, Some("chamber:CIVILE"));
}

#[test]
fn formation_axes_juge_unique_du_texte() {
    // Composition dite par le texte au bloc de composition d'en-tête (TJ).
    let mut d = decision("TJ");
    d.texte_integral_clean = "TRIBUNAL JUDICIAIRE DE NANTERRE \n\n JUGEMENT DU 3 MAI 2024 \n\n \
        Fairouz HAMMAOUI, Vice-présidente, statuant en juge unique, \n assistée de Inès CELMA, \
        Greffier \n\n PAR CES MOTIFS : Déboute la demanderesse."
        .into();
    let axes = crate::extract::formation_axes_scanned(&d, crate::extract::scan_doc(&d).as_ref());
    assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));

    // ORDONNANCE de référé TA (membre ORTA_), signée par ses articles CJA —
    // le juge des référés statue seul (L. 511-2 CJA).
    let mut d2 = decision("TA");
    d2.source_uid = "TA_202503.zip/TA38/ORTA_2502403_20250314.xml".into();
    d2.texte_integral_clean = "Vu la requête, présentée au titre de l'article L. 521-2 du code \
        de justice administrative. Par ces motifs : la requête est rejetée."
        .into();
    let axes2 = crate::extract::formation_axes_scanned(&d2, crate::extract::scan_doc(&d2).as_ref());
    assert_eq!(axes2.formation_uid, Some("formation:JUGE_UNIQUE"));

    // Un JUGEMENT (DTA_) citant L. 521-x raconte le référé antérieur de
    // l'affaire — pas de lecture de composition.
    let mut d3 = decision("TA");
    d3.source_uid = "TA_202503.zip/TA38/DTA_2502403_20250314.xml".into();
    d3.texte_integral_clean = d2.texte_integral_clean.clone();
    let axes3 = crate::extract::formation_axes_scanned(&d3, crate::extract::scan_doc(&d3).as_ref());
    assert_eq!(axes3.formation_uid, None);
}

// ────────────────────────────────── solution ────────────────────────────────

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
fn solution_admin_rejet_prefix() {
    let mut d = decision("TA");
    d.solution = Some("Rejet".to_string());
    assert_eq!(extract_solution(&d).as_deref(), Some("solution:REJET"));
    // école gold : ordre admin au dispositif VERBATIM — un acte annulé est
    // une ANNULATION, jamais une « satisfaction » du requérant
    d.solution = Some("Annulation".to_string());
    assert_eq!(extract_solution(&d).as_deref(), Some("solution:ANNULATION"));
    d.solution = Some("Désistement".to_string());
    assert_eq!(
        extract_solution(&d).as_deref(),
        Some("solution:DESISTEMENT")
    );
}

#[test]
fn solution_admin_irrecevabilite_substring() {
    let mut d = decision("TA");
    d.solution = Some("Rejet pour irrecevabilité".to_string());
    assert_eq!(
        extract_solution(&d).as_deref(),
        Some("solution:IRRECEVABILITE")
    );
}

// ─────────────────────────────────── voie ───────────────────────────────────

#[test]
fn procedure_qpc_from_solution() {
    let mut d = decision("CC");
    d.solution = Some("qpc_renvoi".into());
    assert_eq!(
        extract_procedure(&d).procedure_uid.as_deref(),
        Some("procedure:QPC")
    );
}

#[test]
fn procedure_papc_from_header_non_admission() {
    // « NON-ADMISSION » dans le bandeau d'en-tête, `solution` ne le dit pas
    // (souvent `rejet`/`other`) : doit quand même classer PAPC.
    let mut d = decision("CC");
    d.solution = Some("rejet".into());
    d.texte_integral_clean =
        "N° R 26-80.549 F  N° 50668  15 AVRIL 2026  NON-ADMISSION  M. BONNAL président,".into();
    assert_eq!(
        extract_procedure(&d).procedure_uid.as_deref(),
        Some("procedure:PAPC")
    );
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
    assert_eq!(
        extract_procedure(&d).procedure_uid.as_deref(),
        Some("procedure:PAPC")
    );
}

#[test]
fn procedure_refere_fallback() {
    let mut d = decision("CA");
    d.chamber = Some("Chambre des référés".into());
    assert_eq!(
        extract_procedure(&d).procedure_uid.as_deref(),
        Some("procedure:REFERE_CIVIL")
    );
}

#[test]
fn procedure_refere_liberte_from_text() {
    let mut d = decision("TA");
    d.texte_integral_clean =
        "Vu la requête présentée au titre de l'article L. 521-2 du code de justice administrative"
            .to_string();
    assert_eq!(
        extract_procedure(&d).procedure_uid.as_deref(),
        Some("procedure:REFERE_LIBERTE")
    );
}

#[test]
fn procedure_ordinaire_default() {
    let mut d = decision("CA");
    d.chamber = Some("Première chambre civile".into());
    assert_eq!(extract_procedure(&d), ProcedureUids::default());
    assert_eq!(extract_procedure(&decision("TA")), ProcedureUids::default());
}

#[test]
fn procedure_jld_from_chamber() {
    let mut d = decision("TJ");
    d.chamber = Some("J.L.D.".into());
    assert_eq!(
        extract_procedure(&d).office_uid.as_deref(),
        Some("office:JLD")
    );
}

// ──────────────────────────── publication / dates ───────────────────────────

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

// ────────────────────────────────── routage ─────────────────────────────────

#[test]
fn routed_unknown_errors() {
    let d = decision("XX");
    assert!(crate::extract::routed(&d).is_err());
}

// ──────────────────────────── fusion méta / corps ────────────────────────────

#[test]
fn merge_meta_first_prefers_mixed_case_slice_for_same_name() {
    // Métadonnée CAPS + tranche corps du MÊME nom en casse mixte (accents
    // restaurés) : la tranche corps prend la place de tête.
    let out = crate::extract::merge_meta_first(
        Some("POMEON".into()),
        vec!["Poméon".into(), "Autre".into()],
    );
    assert_eq!(out, vec!["Poméon".to_string(), "Autre".to_string()]);
    // Recouvrement PARTIEL (la méta porte deux noms) : la méta reste telle
    // quelle, la tranche redondante est absorbée sans remplacement.
    let out =
        crate::extract::merge_meta_first(Some("RIQUELME DUPONT".into()), vec!["Riquelme".into()]);
    assert_eq!(out, vec!["RIQUELME DUPONT".to_string()]);
}
