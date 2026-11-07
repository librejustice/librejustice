use super::*;
use serde_json::json;

fn sec(kind: &str, start: usize, end: usize) -> DecisionSection {
    DecisionSection {
        label: kind.to_string(),
        kind: kind.to_string(),
        start_char: start,
        end_char: end,
        text: String::new(),
    }
}

#[test]
fn source_fields_drops_text_rebases_zones_keeps_rest() {
    let payload = json!({
        "text": "le texte brut",
        "zones": { "expose": [{ "start": 0, "end": 5 }] },
        "visa": [{ "title": "Cass. civ. 1, 2012" }],
        "themes": ["responsabilité"],
        "nac": "12A",
    });
    let sections = vec![sec("expose", 0, 4), sec("dispositif", 4, 13)];
    let sf = build_source_fields(&payload, &sections);
    let obj = sf.as_object().expect("objet");
    assert!(!obj.contains_key("text"));
    assert!(!obj.contains_key("zones"));
    assert_eq!(
        obj.get("sections").unwrap(),
        &json!([
            { "kind": "expose", "start": 0, "end": 4 },
            { "kind": "dispositif", "start": 4, "end": 13 },
        ])
    );
    assert_eq!(obj.get("themes").unwrap(), &json!(["responsabilité"]));
    assert_eq!(obj.get("nac").unwrap(), &json!("12A"));
    assert!(obj.contains_key("visa"));
}

#[test]
fn from_source_fields_json_round_trips_sections() {
    // Invariant chemin linéaire : payload → `build_source_fields` →
    // `Decision::from_source_fields` doit reproduire le parse direct
    // (texte/sections/scalaires). Texte ASCII propre (clean_texte = identité) →
    // offsets `zones` bruts == offsets `sections` nettoyés.
    let full_text = "alpha beta gamma";
    let payload = json!({
        "id": "CCASS_1",
        "text": full_text,
        "zones": {
            "expose": [{ "start": 0, "end": 5 }],
            "motivations": [{ "start": 6, "end": 10 }],
            "dispositif": [{ "start": 11, "end": 16 }],
        },
        "ecli": "ECLI:FR:CCASS:2020:XX",
        "visa": [{ "title": "Vu l'article 1240 du code civil" }],
    });
    let orig = parse_judilibre(&payload, Some("judilibre/CCASS_1"));
    let sf = build_source_fields(&payload, &orig.sections);

    let new = Decision::from_source_fields(full_text, &sf, "judilibre/CCASS_1");
    assert_eq!(new.texte_integral_clean, orig.texte_integral_clean);
    assert_eq!(new.ecli, orig.ecli);
    assert_eq!(new.solution, orig.solution);
    assert_eq!(new.sections, orig.sections);
    let kinds: Vec<&str> = new.sections.iter().map(|s| s.kind.as_str()).collect();
    assert!(kinds.contains(&"visa"));
    assert!(kinds.contains(&"expose"));
    assert!(kinds.contains(&"dispositif"));
}

#[test]
fn from_source_fields_reads_a_french_text_in_capitals_recased() {
    let caps = "ATTENDU QUE LA COUR D'APPEL A RETENU QUE LE BAIL ETAIT RESILIE DE PLEIN DROIT ; \
                QU'EN STATUANT AINSI, ALORS QUE LE BAILLEUR N'AVAIT DELIVRE AUCUN COMMANDEMENT, \
                LA COUR D'APPEL A VIOLE LE TEXTE SUSVISE ET PRIVE SA DECISION DE BASE LEGALE AU \
                REGARD DES ARTICLES 1134 ET 1184 DU CODE CIVIL ;\nPAR CES MOTIFS : CASSE ET ANNULE";
    let sf = build_source_fields(&json!({ "id": "CCASS_CAPS", "text": caps }), &[]);
    let read = Decision::from_source_fields(caps, &sf, "judilibre/CCASS_CAPS");
    assert!(read
        .texte_integral_clean
        .starts_with("Attendu que la cour d'appel a retenu que le bail était résilié"));
    assert!(read
        .texte_integral_clean
        .ends_with("PAR CES MOTIFS : CASSE ET ANNULE"));

    let english = "THE COURT HOLDS THAT THERE HAS BEEN A VIOLATION OF ARTICLE 6 OF THE \
                   CONVENTION, THE APPLICANT HAVING HAD NO ACCESS TO A COURT FOR THE \
                   DETERMINATION OF HIS CIVIL RIGHTS AND OBLIGATIONS FOR ELEVEN YEARS, AND \
                   AWARDS HIM FIVE THOUSAND EUROS IN RESPECT OF NON-PECUNIARY DAMAGE.";
    let cedh_sf = build_source_fields(&json!({ "appno": "1/20", "lang": "eng" }), &[]);
    let read = Decision::from_source_fields(english, &cedh_sf, "cedh/001-2");
    assert_eq!(read.texte_integral_clean, english);
}

#[test]
fn from_source_fields_xml_round_trips_decision_scalars() {
    // Invariant chemin linéaire XML : XML → (full_text, source_fields) →
    // `Decision::from_source_fields` reproduit les scalaires, le même texte nettoyé et
    // les mêmes sections que le parse direct. `&amp;` exerce l'échappement source.
    let xml = r#"<Document>
<Dossier>
<Code_Juridiction>TA34</Code_Juridiction>
<Nom_Juridiction>Tribunal administratif de Lyon</Nom_Juridiction>
<Numero_Dossier>1900123</Numero_Dossier>
<Date_Lecture>2022-05-10</Date_Lecture>
<Type_Recours>Plein contentieux</Type_Recours>
<Solution>Rejet</Solution>
<Code_Publication>C</Code_Publication>
</Dossier>
<Audience>
<Date_Audience>2022-04-15</Date_Audience>
<Numero_Role>1900123</Numero_Role>
<Formation_Jugement>3e chambre</Formation_Jugement>
</Audience>
<Decision><Texte_Integral>Vu la requête &amp; les pièces. Par ces motifs, décide : rejet.</Texte_Integral></Decision>
</Document>"#
            .as_bytes();

    let orig = parse_xml(xml, "DTA_1", None);
    let sf = build_source_fields_xml(xml);
    let new = Decision::from_source_fields(&orig.texte_integral_clean, &sf, "DTA_1");

    assert_eq!(new.jurisdiction_source_code, orig.jurisdiction_source_code);
    assert_eq!(new.chamber, orig.chamber);
    assert_eq!(new.jurisdiction_name, orig.jurisdiction_name);
    assert_eq!(new.jurisdiction_type, orig.jurisdiction_type);
    assert_eq!(new.numero_dossier, orig.numero_dossier);
    assert_eq!(new.date_lecture, orig.date_lecture);
    assert_eq!(new.type_recours, orig.type_recours);
    assert_eq!(new.solution, orig.solution);
    assert_eq!(new.publication_codes, orig.publication_codes);
    assert_eq!(new.numero_role, orig.numero_role);
    assert_eq!(new.formation, orig.formation);
    assert_eq!(new.date_audience, orig.date_audience);
    assert_eq!(new.texte_integral_clean, orig.texte_integral_clean);
    assert_eq!(new.metadata_header, orig.metadata_header);
    assert_eq!(new.sections, orig.sections);
}

#[test]
fn from_source_fields_dispatches_on_source_fields_shape() {
    // Discriminant : `source_fields` XML porte `<Dossier>`/`<Audience>` (jamais
    // présents en JSON Judilibre) → branche XML ; sinon JSON.
    let xml = r#"<Document>
<Dossier>
<Nom_Juridiction>Tribunal administratif de Lyon</Nom_Juridiction>
<Date_Lecture>2022-05-10</Date_Lecture>
</Dossier>
<Decision><Texte_Integral>Par ces motifs, décide : rejet.</Texte_Integral></Decision>
</Document>"#
        .as_bytes();
    let xml_orig = parse_xml(xml, "DTA_1", None);
    let xml_sf = build_source_fields_xml(xml);
    assert!(source_fields_is_xml(&xml_sf));
    let xml_new = Decision::from_source_fields(&xml_orig.texte_integral_clean, &xml_sf, "DTA_1");
    assert_eq!(xml_new.jurisdiction_name, xml_orig.jurisdiction_name);
    assert_eq!(xml_new.texte_integral_clean, xml_orig.texte_integral_clean);
    assert_eq!(xml_new.sections, xml_orig.sections);

    let full_text = "alpha beta gamma";
    let payload = json!({
        "id": "CCASS_1",
        "text": full_text,
        "zones": { "dispositif": [{ "start": 11, "end": 16 }] },
        "solution": "Cassation",
    });
    let json_orig = parse_judilibre(&payload, Some("judilibre/CCASS_1"));
    let json_sf = build_source_fields(&payload, &json_orig.sections);
    assert!(!source_fields_is_xml(&json_sf));
    let json_new = Decision::from_source_fields(full_text, &json_sf, "judilibre/CCASS_1");
    assert_eq!(json_new.solution, json_orig.solution);
    assert_eq!(
        json_new.texte_integral_clean,
        json_orig.texte_integral_clean
    );
    assert_eq!(json_new.sections, json_orig.sections);

    // Familles scrapées : le préfixe du `source_uid` route AVANT la forme de
    // `source_fields` — les colonnes HUDOC `appno`/`ecli` n'ont aucun discriminant
    // XML/DILA. Le préfixe `cedh/` route vers le parseur dédié.
    let cedh_columns = json!({ "appno": "1/20", "ecli": "ECLI:CE:ECHR:2026:X" });
    let cedh_body = "Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
    let cedh_sf = build_source_fields(&cedh_columns, &[]);
    assert!(!source_fields_is_xml(&cedh_sf));
    let cedh_new = Decision::from_source_fields(cedh_body, &cedh_sf, "cedh/001-1");
    assert_eq!(cedh_new.jurisdiction_type.as_deref(), Some("CEDH"));
}

#[test]
fn source_fields_excludes_synthetic_visa_section() {
    // La section visa synthétique (SECTION_NO_OFFSET) n'indexe pas full_text →
    // exclue ; reconstruite au rendu depuis source_fields["visa"].
    let payload = json!({ "text": "x", "visa": [{ "title": "T" }] });
    let sections = vec![
        sec("visa", SECTION_NO_OFFSET, SECTION_NO_OFFSET),
        sec("dispositif", 0, 1),
    ];
    let secs = build_source_fields(&payload, &sections)
        .as_object()
        .unwrap()
        .get("sections")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(secs.len(), 1);
    assert_eq!(secs[0].get("kind").unwrap(), "dispositif");
}

#[test]
fn source_fields_null_for_non_object_payload() {
    // XML opendata : payload non-JSON-objet → pas de source_fields.
    assert_eq!(build_source_fields(&json!("raw xml"), &[]), Value::Null);
}

#[test]
fn source_fields_no_sections_key_when_no_real_offsets() {
    let sf = build_source_fields(&json!({ "text": "x", "themes": ["a"] }), &[]);
    assert!(!sf.as_object().unwrap().contains_key("sections"));
}

/// On real JSON payloads, the chunker's input rebuilt from `(full_text,
/// source_fields)` equals the direct parse's. Gated by `LJ_VERIFY_PAYLOADS=<jsonl>`,
/// one payload a line.
#[test]
fn chunker_input_reconstructible_from_full_text_and_source_fields() {
    let Ok(path) = std::env::var("LJ_VERIFY_PAYLOADS") else {
        return;
    };
    let content = std::fs::read_to_string(&path).expect("payloads jsonl");
    let (mut n, mut skipped) = (0usize, 0usize);
    let (mut mh_bad, mut inv_bad) = (0usize, 0usize);

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let payload: Value = serde_json::from_str(line).expect("json");
        let d = parse_judilibre(&payload, None);
        if d.jurisdiction_type.is_none() || d.texte_integral_clean.is_empty() {
            skipped += 1;
            continue;
        }
        n += 1;
        let full_text = &d.texte_integral_clean;
        let sf = build_source_fields(&payload, &d.sections);

        // 1. metadata_header reconstruit depuis source_fields.
        if build_metadata_header(&sf) != d.metadata_header {
            mh_bad += 1;
        }

        // 2. An expose or moyens section's text is its slice of full_text.
        for s in d
            .sections
            .iter()
            .filter(|s| s.kind == "expose" || s.kind == "moyens")
        {
            if s.text != char_slice(full_text, s.start_char, s.end_char) {
                inv_bad += 1;
            }
        }
    }

    eprintln!(
        "CERTIF chunker-input : n={n} skipped={skipped} | \
             metadata_header mismatch={mh_bad} | \
             invariant(section.text==slice) violations={inv_bad}"
    );
    assert_eq!(
        mh_bad, 0,
        "metadata_header non reconstructible à l'identique"
    );
    assert_eq!(inv_bad, 0, "invariant section.text==slice violé");
}

#[test]
fn source_fields_xml_groups_dossier_audience_scalars() {
    let xml = r#"<Document>
<Dossier>
<Nom_Juridiction>Tribunal administratif de Lyon</Nom_Juridiction>
<Type_Recours>Plein contentieux</Type_Recours>
<Solution>Rejet</Solution>
<Identification>Sous-arbre</Identification>
</Dossier>
<Audience>
<Date_Audience>2022-04-15</Date_Audience>
<Formation_Jugement>3e chambre</Formation_Jugement>
</Audience>
<Decision><Texte_Integral>corps</Texte_Integral></Decision>
</Document>"#
        .as_bytes();
    let sf = build_source_fields_xml(xml);
    let dossier = sf.get("Dossier").and_then(Value::as_object).unwrap();
    assert_eq!(
        dossier.get("Nom_Juridiction").unwrap(),
        "Tribunal administratif de Lyon"
    );
    assert_eq!(dossier.get("Type_Recours").unwrap(), "Plein contentieux");
    assert_eq!(dossier.get("Solution").unwrap(), "Rejet");
    assert_eq!(dossier.get("Identification").unwrap(), "Sous-arbre");
    let audience = sf.get("Audience").and_then(Value::as_object).unwrap();
    assert_eq!(audience.get("Date_Audience").unwrap(), "2022-04-15");
    assert_eq!(audience.get("Formation_Jugement").unwrap(), "3e chambre");
    assert!(sf.get("Decision").is_none());
}

#[test]
fn metadata_header_xml_reconstructs_from_source_fields() {
    let xml = r#"<Document>
<Dossier>
<Nom_Juridiction>CAA de Nantes</Nom_Juridiction>
<Date_Lecture>2023-01-12</Date_Lecture>
<Type_Recours>Excès de pouvoir</Type_Recours>
<Solution>Annulation</Solution>
</Dossier>
<Audience>
<Date_Audience>2022-12-20</Date_Audience>
<Formation_Jugement>1re chambre</Formation_Jugement>
</Audience>
</Document>"#
        .as_bytes();
    let direct = build_metadata_header_xml(&build_tree(xml).unwrap());
    let recon = build_metadata_header_xml_from_fields(&build_source_fields_xml(xml));
    assert_eq!(recon, direct);
    assert!(direct.contains("CAA de Nantes | 2023-01-12"));
}

/// The XML counterpart for the opendata payloads, on the `*.xml` of
/// `LJ_VERIFY_XML_DIR`: `metadata_header` rebuilt from `source_fields_xml` equals
/// the direct parse's.
#[test]
fn xml_chunker_input_reconstructible_from_full_text_and_source_fields() {
    let Ok(dir) = std::env::var("LJ_VERIFY_XML_DIR") else {
        return;
    };
    let (mut n, mut skipped) = (0usize, 0usize);
    let mut mh_bad = 0usize;
    let mut first_bad: Option<String> = None;

    for entry in std::fs::read_dir(&dir).expect("xml dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("xml") {
            continue;
        }
        let raw = std::fs::read(&path).expect("xml bytes");
        let d = parse_xml(&raw, path.file_name().unwrap().to_str().unwrap(), None);
        if d.texte_integral_clean.is_empty() {
            skipped += 1;
            continue;
        }
        n += 1;
        let sf = build_source_fields_xml(&raw);

        // 1. metadata_header reconstruit depuis source_fields.
        if build_metadata_header_xml_from_fields(&sf) != d.metadata_header {
            mh_bad += 1;
            first_bad.get_or_insert_with(|| format!("metadata_header diverge : {path:?}"));
        }
    }

    eprintln!(
        "CERTIF XML chunker-input : n={n} skipped={skipped} | \
             metadata_header mismatch={mh_bad}"
    );
    if let Some(ex) = first_bad {
        eprintln!("  first divergent: {ex}");
    }
    assert_eq!(
        mh_bad, 0,
        "metadata_header XML non reconstructible à l'identique"
    );
}

#[test]
fn xml_general_entity_refs_are_resolved_in_text() {
    // quick-xml émet `&amp;` / `&#160;` / `&#39;` comme events GeneralRef distincts ;
    // le parser doit les résoudre.
    let xml = b"<Document><Dossier>\
<Nom_Juridiction>TA</Nom_Juridiction>\
<Avocat_Requerant>SARL BONDIGUEL &amp; ASSOCIES</Avocat_Requerant>\
</Dossier></Document>";
    let d = parse_xml(xml, "ORCA_1.xml", None);
    assert_eq!(
        d.avocat_requerant.as_deref(),
        Some("SARL BONDIGUEL & ASSOCIES")
    );

    let xml2 = b"<Document><Dossier>\
<Nom_Juridiction>Conseil&#160;d&#39;Etat</Nom_Juridiction>\
</Dossier></Document>";
    let d2 = parse_xml(xml2, "DCE_2.xml", None);
    assert_eq!(d2.jurisdiction_name.as_deref(), Some("Conseil\u{a0}d'Etat"));
}

// ── Vocab ────────────────────────────────────────────────────────────────

#[test]
fn vocab_known_and_fallback() {
    assert_eq!(
        vocab::jurisdiction_label(Some("CC")).as_deref(),
        Some("Cour de cassation")
    );
    assert_eq!(
        vocab::jurisdiction_label(Some("XYZ")).as_deref(),
        Some("XYZ")
    );
    assert_eq!(vocab::jurisdiction_label(None), None);
    assert_eq!(vocab::jurisdiction_label(Some("")), None);

    assert_eq!(vocab::jurisdiction_type(Some("zzz")), None);
    assert_eq!(vocab::jurisdiction_type(Some("tj")).as_deref(), Some("TJ"));
    assert_eq!(
        vocab::jurisdiction_type(Some("cph")).as_deref(),
        Some("CPH")
    );

    assert_eq!(
        vocab::formation_label(Some("fs")).as_deref(),
        Some("Formation de section")
    );
    assert_eq!(vocab::formation_label(Some("zz")).as_deref(), Some("ZZ"));

    assert_eq!(
        vocab::chamber_label(Some("civ1")).as_deref(),
        Some("Première chambre civile")
    );
    assert_eq!(vocab::type_label(Some("arret")).as_deref(), Some("Arrêt"));
    assert_eq!(
        vocab::solution_label(Some("rejet")).as_deref(),
        Some("Rejet")
    );
}

// ── strip_html ─────────────────────────────────────────────────────────

#[test]
fn strip_html_removes_tags_and_trims() {
    assert_eq!(
        strip_html(Some("  <p>Vu l'<b>article</b></p>  ")),
        "Vu l'article"
    );
    assert_eq!(strip_html(None), "");
    assert_eq!(strip_html(Some("")), "");
}

// ── char helpers ─────────────────────────────────────────────────────────

#[test]
fn char_helpers_are_codepoint_based() {
    let s = "éàç-xyz";
    assert_eq!(char_len(s), 7);
    assert_eq!(char_slice(s, 0, 3), "éàç");
    assert_eq!(char_take(s, 4), "éàç-");
    assert_eq!(char_find_from(s, "xyz", 0), Some(4));
    assert_eq!(char_find_from(s, "xyz", 5), None);
}

// ── metadata header ──────────────────────────────────────────────────────

#[test]
fn metadata_header_three_lines() {
    let payload = json!({
        "jurisdiction": "cc",
        "chamber": "civ1",
        "decision_date": "2023-05-17",
        "type": "arret",
        "solution": "rejet",
        "formation": "fs",
    });
    let header = build_metadata_header(&payload);
    assert_eq!(
        header,
        "Cour de cassation, Première chambre civile | 2023-05-17\n\
             Recours : Arrêt | Solution : Rejet\n\
             Formation : Formation de section"
    );
}

#[test]
fn metadata_header_partial() {
    let payload = json!({ "jurisdiction": "tj", "decision_date": "2020-01-02" });
    assert_eq!(
        build_metadata_header(&payload),
        "Tribunal judiciaire | 2020-01-02"
    );
}

// ── parse_judilibre : identité, numéros, padding ───────────────────────────

#[test]
fn parse_basic_identity_and_numbers() {
    let payload = json!({
        "id": "abc123",
        "jurisdiction": "cc",
        "chamber": "soc",
        "location": "cc",
        "decision_date": "2022-03-10",
        "update_date": "2022-04-01",
        "formation": "fs",
        "type": "arret",
        "solution": "cassation",
        "publication": ["b", "r"],
        "numbers": ["22-18.339", "22-18.339", " 21-10.000 ", ""],
        "number": "22-18.3392218339",
        "text": "Vu la procédure. PAR CES MOTIFS, la Cour rejette le pourvoi.",
    });
    let d = parse_judilibre(&payload, None);

    assert_eq!(d.source_uid, "judilibre/abc123");
    assert_eq!(d.member_name, "abc123");
    assert_eq!(d.jurisdiction_source_code, None);
    assert_eq!(d.chamber.as_deref(), Some("soc"));
    assert_eq!(d.jurisdiction_name.as_deref(), Some("Cour de cassation"));
    assert_eq!(d.jurisdiction_type.as_deref(), Some("CC"));
    assert_eq!(d.jurisdiction_location.as_deref(), Some("cc"));
    assert_eq!(
        d.numero_dossiers.as_deref(),
        Some(&["22-18.339".to_string(), "21-10.000".to_string()][..])
    );
    assert_eq!(d.numero_dossier.as_deref(), Some("22-18.339"));
    assert_eq!(d.date_lecture.as_deref(), Some("2022-03-10"));
    assert_eq!(d.date_mise_jour.as_deref(), Some("2022-04-01"));
    assert_eq!(d.formation.as_deref(), Some("Formation de section"));
    assert_eq!(d.type_recours.as_deref(), Some("Arrêt"));
    assert_eq!(d.solution.as_deref(), Some("Cassation"));
    assert_eq!(d.publication_codes, vec!["b".to_string(), "r".to_string()]);
    assert_eq!(d.numero_role, None);
    assert_eq!(d.date_audience, None);
    assert_eq!(d.type_decision, None);
    assert_eq!(d.avocat_requerant, None);
}

#[test]
fn parse_numbers_fallback_to_scalar_number() {
    let payload = json!({
        "id": "x", "text": "",
        "number": "98-12.345",
    });
    let d = parse_judilibre(&payload, Some("custom-member"));
    assert_eq!(d.member_name, "custom-member");
    assert_eq!(d.numero_dossier.as_deref(), Some("98-12.345"));
    assert_eq!(d.numero_dossiers, None);
}

#[test]
#[should_panic(expected = "payload Judilibre sans 'id'")]
fn parse_without_id_panics() {
    let payload = json!({ "text": "x" });
    let _ = parse_judilibre(&payload, None);
}

// ── sections : zones + visa synthétique ────────────────────────────────────

#[test]
fn sections_from_zones_and_visa() {
    let raw = "Exposé du litige ici. Les moyens du pourvoi sont exposés. Motifs de la Cour.";
    let payload = json!({
        "id": "z1",
        "text": raw,
        "zones": {
            "expose": [{ "start": 0, "end": 21 }],
            "moyens": [{ "start": 22, "end": 56 }],
            "motivations": [{ "start": 57, "end": 75 }],
        },
        "visa": [
            { "title": "<p>Vu l'article 700 du code de procédure civile</p>" },
            { "title": "Vu les articles 1 et 2" },
        ],
    });
    let d = parse_judilibre(&payload, None);

    assert_eq!(d.sections[0].kind, "visa");
    assert_eq!(d.sections[0].start_char, SECTION_NO_OFFSET);
    assert_eq!(
        d.sections[0].text,
        "Vu l'article 700 du code de procédure civile\nVu les articles 1 et 2"
    );

    let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
    assert!(kinds.contains(&"expose"));
    assert!(kinds.contains(&"moyens"));
    assert!(kinds.contains(&"motivations"));
}

#[test]
fn dispositif_fallback_on_par_ces_motifs() {
    let raw = "Exposé du litige détaillé ici. PAR CES MOTIFS, la Cour REJETTE le pourvoi.";
    let payload = json!({
        "id": "f1",
        "text": raw,
        "zones": { "expose": [{ "start": 0, "end": 30 }] },
    });
    let d = parse_judilibre(&payload, None);
    let disp = d.sections.iter().find(|s| s.kind == "dispositif");
    assert!(disp.is_some(), "fallback dispositif attendu");
    let disp = disp.unwrap();
    assert!(disp.text.starts_with("PAR CES MOTIFS"));
    assert_eq!(disp.end_char, char_len(&d.texte_integral_clean));
}

#[test]
fn no_dispositif_fallback_when_zone_present() {
    let raw = "Texte. PAR CES MOTIFS rejette.";
    let payload = json!({
        "id": "f2",
        "text": raw,
        "zones": { "dispositif": [{ "start": 7, "end": 30 }] },
    });
    let d = parse_judilibre(&payload, None);
    let count = d.sections.iter().filter(|s| s.kind == "dispositif").count();
    assert_eq!(count, 1);
}

// ── XML opendata ───────────────────────────────────────────────────────

#[test]
fn classify_uid_by_prefix() {
    assert_eq!(classify_uid("DTA_2204150_20220829").as_deref(), Some("TA"));
    assert_eq!(classify_uid("ORCA_123").as_deref(), Some("CAA"));
    assert_eq!(classify_uid("DCE_999").as_deref(), Some("CE"));
    assert_eq!(classify_uid("foo/DCA_1").as_deref(), Some("CAA"));
    assert_eq!(classify_uid("UNKNOWN_1"), None);
}

#[test]
fn xml_source_uid_with_archive_prefix() {
    let xml = br#"<Document><Donnees_Techniques><Identification>X</Identification></Donnees_Techniques></Document>"#;
    let d = parse_xml(xml, "DTA_1_2.xml", Some("TA_202208.zip/TA34"));
    assert_eq!(d.source_uid, "TA_202208.zip/TA34/DTA_1_2.xml");
    assert_eq!(d.jurisdiction_type.as_deref(), Some("TA"));
}

#[test]
fn xml_source_uid_fallback_uses_identification() {
    let xml = br#"<Document><Donnees_Techniques><Identification>DTA_42_99</Identification></Donnees_Techniques></Document>"#;
    let d = parse_xml(xml, "anyname.xml", None);
    assert_eq!(d.source_uid, "DTA_42_99");
    assert_eq!(d.jurisdiction_type.as_deref(), Some("TA"));
}

#[test]
fn xml_fields_and_metadata_header() {
    let xml = br#"<Document>
            <Dossier>
                <Code_Juridiction>TA13</Code_Juridiction>
                <Nom_Juridiction>Tribunal Administratif de Marseille</Nom_Juridiction>
                <Numero_Dossier>2204150</Numero_Dossier>
                <Date_Lecture>2022-08-29</Date_Lecture>
                <Type_Recours>Plein contentieux</Type_Recours>
                <Solution>Rejet</Solution>
                <Code_Publication>C+</Code_Publication>
            </Dossier>
            <Audience>
                <Formation_Jugement>5eme chambre</Formation_Jugement>
            </Audience>
        </Document>"#;
    let d = parse_xml(xml, "DTA_2204150_20220829.xml", Some("TA_202208.zip"));
    assert_eq!(d.jurisdiction_source_code.as_deref(), Some("TA13"));
    assert_eq!(
        d.jurisdiction_name.as_deref(),
        Some("Tribunal Administratif de Marseille")
    );
    assert_eq!(d.numero_dossier.as_deref(), Some("2204150"));
    assert_eq!(d.date_lecture.as_deref(), Some("2022-08-29"));
    assert_eq!(d.type_recours.as_deref(), Some("Plein contentieux"));
    assert_eq!(d.solution.as_deref(), Some("Rejet"));
    assert_eq!(d.publication_codes, vec!["C+".to_string()]);
    assert_eq!(d.formation.as_deref(), Some("5eme chambre"));
    assert_eq!(
        d.metadata_header,
        "Tribunal Administratif de Marseille | 2022-08-29\n\
             Recours : Plein contentieux | Solution : Rejet\n\
             Formation : 5eme chambre"
    );
}

#[test]
fn xml_date_lecture_falls_back_to_date_audience() {
    let xml = br#"<Document>
            <Dossier><Nom_Juridiction>TA</Nom_Juridiction></Dossier>
            <Audience><Date_Audience>2021-01-15</Date_Audience></Audience>
        </Document>"#;
    let d = parse_xml(xml, "DTA_1.xml", None);
    assert_eq!(d.date_lecture.as_deref(), Some("2021-01-15"));
    assert_eq!(d.date_audience.as_deref(), Some("2021-01-15"));
}

#[test]
fn xml_broken_texte_integral_open_is_repaired() {
    let xml =
        b"<Document><Decision><Texte_Integral></p>Bonjour</Texte_Integral></Decision></Document>";
    let d = parse_xml(xml, "DTA_1.xml", None);
    assert!(d
        .parse_warnings
        .contains(&"xml_repair:texte_integral_orphan_closing_p".to_string()));
    assert!(d.texte_integral_raw.contains("Bonjour"));
}

#[test]
fn xml_itertext_aggregates_nested_text() {
    let xml = b"<Document><Dossier><Nom_Juridiction>Conseil <br/>d'Etat</Nom_Juridiction></Dossier></Document>";
    let d = parse_xml(xml, "DCE_1.xml", None);
    assert_eq!(d.jurisdiction_name.as_deref(), Some("Conseil d'Etat"));
}

#[test]
fn html_text_starts_a_line_at_each_break_paragraph_and_row() {
    let article = br#"<CONTENU>
<p>
<br/>est ainsi r&#233;dig&#233; :</p>
<p>
<br/>&#171; mentionn&#233; &#224; l'article L. 6113-6.<br/>&#171; Les actions &#187;</p>
<div>

<table border="1">
<tr>
<th>
<br/>GRADES</th>
<th>
<br/>DUR&#201;E</th>
</tr>
<tr>
<td align="center">
<br/>6e &#233;chelon</td>
<td align="left"/>
<td align="center">
<br/>3 ans</td>
</tr>
</table>
</div>
<p>Fin.</p>
</CONTENU>"#;
    let text = build_tree(article).unwrap().html_text().unwrap();
    assert_eq!(
        clean_texte(&text),
        "est ainsi rédigé :\n\n« mentionné à l'article L. 6113-6.\n« Les actions »\n\nGRADES | DURÉE\n6e échelon | 3 ans\n\nFin."
    );

    let decision = b"<CONTENU>Vu la proc\xc3\xa9dure suivante :<br/>\n<br/>\n       Proc\xc3\xa9dure ant\xc3\xa9rieure :<br/>\nM. A...</CONTENU>";
    let text = build_tree(decision).unwrap().html_text().unwrap();
    assert_eq!(
        clean_texte(&text),
        "Vu la procédure suivante :\n\nProcédure antérieure :\nM. A..."
    );
}

#[test]
fn xml_extract_sections_reads_the_older_motifs_marker() {
    let cleaned = "Vu la requête enregistrée le 3 mars 2011.\n\nVu les autres pièces du dossier ;\n\nSur les conclusions à fin d'annulation : Considérant qu'il résulte de l'instruction que le préjudice est établi ;\n\nDECIDE :\n\nArticle 1er : la requête est rejetée.";
    let sections = extract_sections_xml(cleaned);
    let kinds: Vec<&str> = sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["preamble", "visa", "motivations", "dispositif"]);
    let motifs = sections.iter().find(|s| s.kind == "motivations").unwrap();
    assert!(motifs.text.starts_with("Sur les conclusions"));
}

/// The texts a decision applies open its visa whatever the case it is printed
/// in, inside the single line of the older Conseil d'État as well.
#[test]
fn xml_extract_sections_reads_the_applied_texts_as_the_visa() {
    let cleaned = "Requête du sieur X... tendant à l'annulation d'un jugement ;\nVu le code rural ; la loi du 7 mars 1963 ;\nConsidérant que la requête manque en fait ;\nRejet.";
    let sections = extract_sections_xml(cleaned);
    let kinds: Vec<&str> = sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["preamble", "visa", "motivations"]);
    let cleaned = "Vu la requête présentée pour la société Foullon ; Vu le code général des impôts ;\nConsidérant que la société soutient ;";
    let sections = extract_sections_xml(cleaned);
    assert_eq!(sections[1].kind, "visa");
    assert!(sections[1].text.starts_with("Vu le code"));
}

#[test]
fn xml_extract_sections_prefers_the_canonical_motifs_marker() {
    let cleaned = "Procédure contentieuse antérieure : le tribunal, considérant que la demande était tardive, l'a rejetée.\n\nVu les autres pièces du dossier ;\n\nConsidérant ce qui suit : motifs.\n\nDECIDE :\n\nArticle 1er.";
    let sections = extract_sections_xml(cleaned);
    let motifs: Vec<&str> = sections
        .iter()
        .filter(|s| s.kind == "motivations")
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(motifs.len(), 1);
    assert!(motifs[0].starts_with("Considérant ce qui suit"));
}

/// The recent Conseil constitutionnel drafting closes its visa and opens its
/// dispositif inside a line, so the motifs boundary is the numbered paragraph.
#[test]
fn xml_extract_sections_reads_the_recent_constitutional_hinges() {
    let cleaned = "LE CONSEIL CONSTITUTIONNEL A ÉTÉ SAISI de la loi.\n\nAu vu des textes suivants : la Constitution ; Et après avoir entendu le rapporteur ; LE CONSEIL CONSTITUTIONNEL S'EST FONDÉ SUR CE QUI SUIT :\n\n1. Les députés requérants défèrent la loi.\n\nLE CONSEIL CONSTITUTIONNEL DÉCIDE :Article 1er. La loi est conforme.";
    let sections = extract_sections_xml(cleaned);
    let kinds: Vec<&str> = sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["preamble", "visa", "motivations", "dispositif"]);
    let motifs = sections.iter().find(|s| s.kind == "motivations").unwrap();
    assert!(motifs.text.starts_with("1. Les députés"), "{}", motifs.text);
    let dispositif = sections.iter().find(|s| s.kind == "dispositif").unwrap();
    assert!(
        dispositif
            .text
            .starts_with("LE CONSEIL CONSTITUTIONNEL DÉCIDE"),
        "{}",
        dispositif.text
    );
}

/// The chapeau runs from the visa to the paragraph that turns to the grief, and
/// a branch going straight to the grief carries none.
#[test]
fn judilibre_chapeau_runs_from_the_visa_to_the_grief() {
    let text = "Exposé du litige.\n\nRéponse de la Cour\n\nVu l'article 1240 du code civil :\n\n5. Il résulte de ce texte que la faute engage son auteur.\n\n6. Pour rejeter la demande, l'arrêt retient que le préjudice est incertain.\n\n7. En statuant ainsi, la cour d'appel a violé le texte susvisé.\n\nVu l'article 624 du code de procédure civile :\n\n8. Aux termes de ce texte, la portée de la cassation est déterminée par le dispositif.\n\nMise hors de cause\n\n9. Il y a lieu de mettre hors de cause la société.\n\nVu l'article 700 du code de procédure civile :\n\n10. Pour condamner la partie, l'arrêt énonce que.";
    let payload = json!({
        "id": "CCASS_CHAPEAU",
        "text": text,
        "zones": { "motivations": [{ "start": 19, "end": text.chars().count() }] },
    });
    let d = parse_judilibre(&payload, Some("judilibre/CCASS_CHAPEAU"));
    let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "motivations",
            "chapeau",
            "motivations",
            "chapeau",
            "motivations"
        ]
    );

    let chapeaux: Vec<&str> = d
        .sections
        .iter()
        .filter(|s| s.kind == "chapeau")
        .map(|s| s.text.as_str())
        .collect();
    let chapeau = chapeaux[0];
    assert!(chapeau.starts_with("Vu l'article 1240"), "{chapeau}");
    assert!(chapeau.ends_with("engage son auteur."), "{chapeau}");
    // The heading closes the second one: « Mise hors de cause » is another branch.
    assert!(
        chapeaux[1].ends_with("par le dispositif."),
        "{}",
        chapeaux[1]
    );
    let chapeau = d.sections.iter().find(|s| s.kind == "chapeau").unwrap();
    assert_eq!(
        d.texte_integral_clean
            .chars()
            .skip(chapeau.start_char)
            .take(chapeau.end_char - chapeau.start_char)
            .collect::<String>()
            .trim(),
        chapeau.text
    );
}

#[test]
fn judilibre_chapeau_closes_on_the_lower_court_as_subject() {
    let text = "Réponse de la Cour\n\nVu l'article 530-1 du code de procédure pénale :\n\n5. Il résulte de ce texte que le tribunal ne peut prononcer une peine plus élevée.\n\n6. Le tribunal de police a condamné le prévenu à une amende de 300 euros.\n\n7. En statuant ainsi, le tribunal a méconnu le texte susvisé.";
    let payload = json!({
        "id": "CCASS_GRIEF",
        "text": text,
        "zones": { "motivations": [{ "start": 0, "end": text.chars().count() }] },
    });
    let d = parse_judilibre(&payload, Some("judilibre/CCASS_GRIEF"));
    let chapeau = d
        .sections
        .iter()
        .find(|s| s.kind == "chapeau")
        .expect("un chapeau");
    assert!(chapeau.text.ends_with("plus élevée."), "{}", chapeau.text);
}

#[test]
fn xml_extract_sections_partitions_text() {
    let cleaned = "En-tete introductif.\n\nVu : les pieces.\n\nConsidérant ce qui suit : motifs.\n\nDECIDE :\n\nArticle 1.";
    let sections = extract_sections_xml(cleaned);
    let kinds: Vec<&str> = sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["preamble", "visa", "motivations", "dispositif"]);
    assert_eq!(sections[0].start_char, 0);
    assert_eq!(sections.last().unwrap().end_char, char_len(cleaned));
}

fn kinds_and_openings(sections: &[DecisionSection]) -> Vec<(&str, String)> {
    sections
        .iter()
        .map(|s| (s.kind.as_str(), s.text.chars().take(24).collect()))
        .collect()
}

/// A cassation the source zoned nothing of: the header, then each paragraph by
/// its opening, the grief and its branches as moyens, the Court's answer as
/// motivations. Rebuilt from storage, it gives the same sections back.
#[test]
fn judilibre_unzoned_cassation_is_cut_by_its_attendus() {
    let text = "LA COUR DE CASSATION, CHAMBRE SOCIALE, a rendu l'arrêt suivant :\n\nSur le pourvoi formé par la société Alpha, en cassation d'un arrêt rendu le 3 mars 1998 par la cour d'appel de Rouen ;\n\nSur le moyen unique :\n\nAttendu, selon l'arrêt attaqué (Rouen, 3 mars 1998), que M. X..., engagé en 1989, a été licencié pour faute grave ;\n\nAttendu que la société fait grief à l'arrêt de l'avoir condamnée à payer une indemnité, alors, selon le moyen :\n\n1°/ que l'abandon de poste caractérise la faute grave ;\n\nMais attendu que la cour d'appel, qui a constaté que le salarié avait été mis en demeure, a légalement justifié sa décision ;\n\nPAR CES MOTIFS :\n\nREJETTE le pourvoi ;";
    let payload = json!({ "id": "CCASS_LEAD", "jurisdiction": "cc", "text": text });
    let d = parse_judilibre(&payload, Some("judilibre/CCASS_LEAD"));
    assert_eq!(
        kinds_and_openings(&d.sections),
        vec![
            ("preamble", "LA COUR DE CASSATION, CH".to_string()),
            ("moyens", "Sur le moyen unique :".to_string()),
            ("expose", "Attendu, selon l'arrêt a".to_string()),
            ("moyens", "Attendu que la société f".to_string()),
            ("motivations", "Mais attendu que la cour".to_string()),
            ("dispositif", "PAR CES MOTIFS :\n\nREJETT".to_string()),
        ]
    );

    let sf = build_source_fields(&payload, &d.sections);
    let rebuilt = Decision::from_source_fields(text, &sf, "judilibre/CCASS_LEAD");
    assert_eq!(rebuilt.sections, d.sections);
}

/// A cour d'appel the source zoned nothing of is cut at its headings, and an
/// exposé without one opens on the first act of the procedure.
#[test]
fn judilibre_unzoned_appeal_is_cut_at_its_headings() {
    let text = "COUR D'APPEL DE BASTIA\n\nCHAMBRE CIVILE\n\nAPPELANT :\n\nM. Charles X..., assisté de Me Y..., avocat au barreau d'Ajaccio\n\nCOMPOSITION DE LA COUR :\n\nMme Martin, président de chambre\n\nPar jugement du 4 juillet 1997, la mise sous curatelle de M. X... a été prononcée par le juge des tutelles.\n\nM. X... a relevé appel de cette décision.\n\nSUR CE :\n\nAttendu qu'il apparaît indispensable d'ordonner une nouvelle expertise ;\n\nPAR CES MOTIFS,\n\nLA COUR, avant dire droit, ordonne une expertise.";
    let payload = json!({ "id": "CA_LEAD", "jurisdiction": "ca", "text": text });
    let d = parse_judilibre(&payload, Some("judilibre/CA_LEAD"));
    assert_eq!(
        kinds_and_openings(&d.sections),
        vec![
            ("preamble", "COUR D'APPEL DE BASTIA\n\n".to_string()),
            ("expose", "Par jugement du 4 juille".to_string()),
            ("motivations", "SUR CE :\n\nAttendu qu'il ".to_string()),
            ("dispositif", "PAR CES MOTIFS,\n\nLA COUR".to_string()),
        ]
    );

    let sf = build_source_fields(&payload, &d.sections);
    let rebuilt = Decision::from_source_fields(text, &sf, "judilibre/CA_LEAD");
    assert_eq!(rebuilt.sections, d.sections);
}

/// Without a dispositif, the whole text is cut, and left whole when the cut
/// finds a single part.
#[test]
fn judilibre_unzoned_text_without_dispositif_is_cut_only_into_parts() {
    let text = "COUR D'APPEL DE BASTIA\n\nAPPELANT :\n\nM. Charles X..., assisté de Me Y..., avocat\n\nEXPOSÉ DU LITIGE :\n\nPar jugement du 4 juillet 1997, la mise sous curatelle de M. X... a été prononcée.\n\nMOTIFS :\n\nAttendu qu'il apparaît indispensable d'ordonner une nouvelle expertise ;";
    let payload = json!({ "id": "CA_NODISP", "jurisdiction": "ca", "text": text });
    let d = parse_judilibre(&payload, Some("judilibre/CA_NODISP"));
    let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["preamble", "expose", "motivations"]);

    let header = "COUR D'APPEL DE BASTIA\n\nAPPELANT :\n\nM. Charles X..., assisté de Me Y..., avocat au barreau d'Ajaccio, demeurant à Bastia, représenté par son curateur, Mme Z..., demeurant à Ajaccio, intimée, le ministère public, partie jointe, et la caisse d'allocations familiales";
    let payload = json!({ "id": "CA_HEADER", "jurisdiction": "ca", "text": header });
    assert!(parse_judilibre(&payload, Some("judilibre/CA_HEADER"))
        .sections
        .is_empty());
}

/// The chapeaux carved at ingest come back when the decision is rebuilt from
/// storage.
#[test]
fn judilibre_chapeau_survives_storage() {
    let text = "Exposé du litige.\n\nRéponse de la Cour\n\nVu l'article 1240 du code civil :\n\n5. Il résulte de ce texte que la faute engage son auteur.\n\n6. Pour rejeter la demande, l'arrêt retient que le préjudice est incertain.\n\n7. En statuant ainsi, la cour d'appel a violé le texte susvisé.\n\nPAR CES MOTIFS, la Cour CASSE l'arrêt.";
    let payload = json!({
        "id": "CCASS_STORED",
        "jurisdiction": "cc",
        "text": text,
        "zones": {
            "expose": [{ "start": 0, "end": 17 }],
            "motivations": [{ "start": 19, "end": 273 }],
            "dispositif": [{ "start": 275, "end": text.chars().count() }],
        },
    });
    let d = parse_judilibre(&payload, Some("judilibre/CCASS_STORED"));
    let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "expose",
            "motivations",
            "chapeau",
            "motivations",
            "dispositif"
        ]
    );

    let sf = build_source_fields(&payload, &d.sections);
    let rebuilt = Decision::from_source_fields(text, &sf, "judilibre/CCASS_STORED");
    assert_eq!(rebuilt.sections, d.sections);
}

/// The moyens annexés leave the dispositif at their heading, whether the source
/// zoned the dispositif over them or zoned nothing, and a « PAR CES MOTIFS » the
/// annexes quote from the attacked decision opens no dispositif.
#[test]
fn judilibre_annexes_close_the_dispositif_at_their_heading() {
    let text = "Sur le moyen unique :\n\nMais attendu que la cour d'appel a légalement justifié sa décision ;\n\nREJETTE le pourvoi ;\n\nMOYENS ANNEXES au présent arrêt\n\nMoyen produit par Me X, avocat aux Conseils.\n\nAUX MOTIFS QUE ; PAR CES MOTIFS, confirme le jugement.";
    let at = |needle: &str| text[..text.find(needle).unwrap()].chars().count();
    let annexes = at("MOYENS ANNEXES");
    let zoned = json!({
        "id": "CCASS_ANNEXED",
        "jurisdiction": "cc",
        "text": text,
        "zones": { "dispositif": [{ "start": at("REJETTE"), "end": text.chars().count() }] },
    });
    let unzoned = json!({ "id": "CCASS_ANNEXED", "jurisdiction": "cc", "text": text });
    for payload in [zoned, unzoned] {
        let d = parse_judilibre(&payload, Some("judilibre/CCASS_ANNEXED"));
        let dispositif = d.get_section("dispositif").unwrap();
        assert!(
            dispositif.text.starts_with("REJETTE"),
            "{}",
            dispositif.text
        );
        assert!(dispositif.end_char <= annexes);
        let tail = d.sections.last().unwrap();
        assert_eq!((tail.kind.as_str(), tail.start_char), ("annexes", annexes));
        assert_eq!(tail.end_char, text.chars().count());

        let sf = build_source_fields(&payload, &d.sections);
        let rebuilt = Decision::from_source_fields(text, &sf, "judilibre/CCASS_ANNEXED");
        assert_eq!(rebuilt.sections, d.sections);
    }
}

#[test]
fn cnil_deliberations_are_cut_at_their_own_drafting() {
    let kinds = |text: &str| -> Vec<String> {
        extract_sections_cnil(text)
            .into_iter()
            .map(|s| s.kind)
            .collect()
    };
    let authorisation = "(Demande d'autorisation n° 1673651)\nLa Commission nationale de l'informatique et des libertés,\nVu la loi n° 78-17 du 6 janvier 1978 ;\nSur la proposition de M. X, commissaire ;\nFormule les observations suivantes :\nSur la finalité du traitement :\nLe traitement a pour finalité la gestion du personnel.\nAutorise, conformément à la présente délibération, la société X à mettre en œuvre le traitement.";
    assert_eq!(
        kinds(authorisation),
        ["preamble", "visa", "motivations", "dispositif"]
    );
    let sanction = "La Commission nationale de l'informatique et des libertés, réunie en sa formation restreinte,\nVu le règlement (UE) 2016/679 ;\nVu les autres pièces du dossier ;\nLa formation restreinte a adopté la décision suivante :\nI. Faits et procédure\n1. La société X exploite un site web.\nII. Motifs de la décision\n2. La formation restreinte relève un manquement.\nPAR CES MOTIFS\nLa formation restreinte de la CNIL décide de prononcer une amende.";
    assert_eq!(
        kinds(sanction),
        ["preamble", "visa", "expose", "motivations", "dispositif"]
    );
}

#[test]
fn conseil_etat_in_capitals_is_cut_without_its_accents() {
    let text = "VU LA REQUETE, ENREGISTREE AU SECRETARIAT DU CONTENTIEUX LE 15 NOVEMBRE 1980 ;\nCONSIDERANT QU'AUX TERMES DE L'ARTICLE 63 DE LA LOI DU 13 JUILLET 1972, LE TEMPS PASSE COMPTE ;\nCONS. QUE LA REQUETE N'EST PAS FONDEE ;\nDECIDE ARTICLE 1ER : LA REQUETE DE M. X... EST REJETEE.";
    let kinds: Vec<String> = extract_sections_xml(text)
        .into_iter()
        .map(|s| s.kind)
        .collect();
    assert_eq!(kinds, ["visa", "motivations", "dispositif"]);
}
