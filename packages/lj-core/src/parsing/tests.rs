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
    // text → full_text ; zones → sections rebasées.
    assert!(!obj.contains_key("text"));
    assert!(!obj.contains_key("zones"));
    assert_eq!(
        obj.get("sections").unwrap(),
        &json!([
            { "kind": "expose", "start": 0, "end": 4 },
            { "kind": "dispositif", "start": 4, "end": 13 },
        ])
    );
    // reste verbatim.
    assert_eq!(obj.get("themes").unwrap(), &json!(["responsabilité"]));
    assert_eq!(obj.get("nac").unwrap(), &json!("12A"));
    assert!(obj.contains_key("visa"));
}

#[test]
fn from_source_fields_json_round_trips_sections() {
    // Invariant chemin linéaire (#26, ADR 0085) : payload → build_source_fields
    // → Decision::from_source_fields doit reproduire le parse direct
    // (texte/sections/scalaires). Texte ASCII propre (clean_texte = identité) →
    // offsets `zones` bruts == offsets `sections` nettoyés, ce qui isole la
    // logique d'inversion `sections → zones` (la parité raw≠clean est couverte
    // par l'oracle extract-fields-parity sur corpus, pas un test unitaire).
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
    // Sections reproduites (visa synthétique comprise, reformée depuis `visa`).
    assert_eq!(new.sections, orig.sections);
    let kinds: Vec<&str> = new.sections.iter().map(|s| s.kind.as_str()).collect();
    assert!(kinds.contains(&"visa"));
    assert!(kinds.contains(&"expose"));
    assert!(kinds.contains(&"dispositif"));
}

#[test]
fn from_source_fields_xml_round_trips_decision_scalars() {
    // Invariant chemin linéaire XML (#26, ADR 0085) : XML → (full_text,
    // source_fields) → Decision::from_source_fields reproduit les scalaires
    // (entrée des extractors facettes) + le même texte nettoyé + les mêmes
    // sections que le parse direct. `&amp;` exerce l'échappement source.
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

    // Scalaires (entrée des extractors facettes) identiques.
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
    // Texte nettoyé stable (clean_texte idempotent) + sections identiques.
    assert_eq!(new.texte_integral_clean, orig.texte_integral_clean);
    assert_eq!(new.metadata_header, orig.metadata_header);
    assert_eq!(new.sections, orig.sections);
}

#[test]
fn from_source_fields_dispatches_on_source_fields_shape() {
    // Discriminant ADR 0085 : `source_fields` XML porte `<Dossier>`/`<Audience>`
    // (jamais présents en JSON Judilibre) → branche XML ; sinon JSON. Un mauvais
    // branchement = corruption silencieuse, d'où ce test.
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

    // Familles scrapées (#37) : le préfixe du `source_uid` route AVANT la forme
    // de `source_fields` (les colonnes HUDOC `appno`/`ecli` n'ont aucun
    // discriminant XML/DILA → tomberaient sinon dans la branche JSON). Le préfixe
    // `cedh/` route vers le parseur dédié — `jurisdiction_type = CEDH`, pas le repli
    // JSON (`None`).
    let cedh_columns = json!({ "appno": "1/20", "ecli": "ECLI:CE:ECHR:2026:X" });
    let cedh_body = "Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
    let cedh_sf = build_source_fields(&cedh_columns, &[]);
    assert!(!source_fields_is_xml(&cedh_sf));
    let cedh_new = Decision::from_source_fields(cedh_body, &cedh_sf, "cedh/001-1");
    assert_eq!(cedh_new.jurisdiction_type.as_deref(), Some("CEDH"));
}

#[test]
fn source_fields_excludes_synthetic_visa_section() {
    // La section visa synthétique (SECTION_NO_OFFSET) n'indexe pas full_text
    // → exclue ; reconstruite au rendu depuis source_fields["visa"].
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

/// Reconstruit une section depuis le triplet stocké `{kind,start,end}` —
/// son texte est la tranche de `full_text` (gate ADR 0085 : pas de texte de
/// section stocké).
fn section_from_stored(full_text: &str, v: &Value) -> DecisionSection {
    let start = v.get("start").and_then(Value::as_u64).unwrap() as usize;
    let end = v.get("end").and_then(Value::as_u64).unwrap() as usize;
    DecisionSection {
        label: String::new(),
        kind: v.get("kind").and_then(Value::as_str).unwrap().to_string(),
        start_char: start,
        end_char: end,
        text: char_slice(full_text, start, end),
    }
}

/// CERTIFICATION (ADR 0085) : sur les payloads JSON réels, l'entrée du
/// chunker (`metadata_header` + `visa_trim` + `texte_integral_clean`)
/// reconstruite depuis `(full_text, source_fields)` est **identique** à
/// celle produite par le parse direct du payload. Gate par
/// `LJ_VERIFY_PAYLOADS=<jsonl>` (inerte en CI). Un texte par ligne JSON.
#[test]
fn chunker_input_reconstructible_from_full_text_and_source_fields() {
    let Ok(path) = std::env::var("LJ_VERIFY_PAYLOADS") else {
        return;
    };
    let content = std::fs::read_to_string(&path).expect("payloads jsonl");
    let (mut n, mut skipped) = (0usize, 0usize);
    let (mut mh_bad, mut vt_bad, mut inv_bad) = (0usize, 0usize, 0usize);
    let mut first_vt: Option<String> = None;

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

        // 1. metadata_header reconstruit depuis source_fields (mêmes clés
        //    sources, text/zones n'y entrent pas).
        if build_metadata_header(&sf) != d.metadata_header {
            mh_bad += 1;
        }

        // 2. visa_trim reconstruit depuis les sections stockées (texte =
        //    tranche de full_text).
        let recon: Vec<DecisionSection> = sf
            .get("sections")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| section_from_stored(full_text, v))
                    .collect()
            })
            .unwrap_or_default();
        let vt = build_visa_trim(&recon);
        if vt != d.visa_trim {
            vt_bad += 1;
            if first_vt.is_none() {
                first_vt = Some(format!(
                    "uid={} | orig_len={} recon_len={}",
                    d.source_uid,
                    char_len(&d.visa_trim),
                    char_len(&vt),
                ));
            }
        }

        // 3. invariant racine : section.text == char_slice(full_text, …)
        //    pour expose/moyens (les seules lues par build_visa_trim).
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
             metadata_header mismatch={mh_bad} | visa_trim mismatch={vt_bad} | \
             invariant(section.text==slice) violations={inv_bad}"
    );
    if let Some(ex) = first_vt {
        eprintln!("  premier visa_trim divergent : {ex}");
    }
    assert_eq!(
        mh_bad, 0,
        "metadata_header non reconstructible à l'identique"
    );
    assert_eq!(vt_bad, 0, "visa_trim non reconstructible à l'identique");
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
    // Identification est une feuille ici → captée comme scalaire.
    assert_eq!(dossier.get("Identification").unwrap(), "Sous-arbre");
    let audience = sf.get("Audience").and_then(Value::as_object).unwrap();
    assert_eq!(audience.get("Date_Audience").unwrap(), "2022-04-15");
    assert_eq!(audience.get("Formation_Jugement").unwrap(), "3e chambre");
    // Le texte intégral ne fuit pas dans source_fields (→ full_text).
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

/// CERTIFICATION XML (ADR 0085) : pendant de la certif JSON pour les payloads
/// opendata. Sur des XML réels (dossier `LJ_VERIFY_XML_DIR`, *.xml), l'entrée
/// chunker reconstruite depuis `(full_text, source_fields_xml)` est identique
/// au parse direct : `metadata_header` via `build_metadata_header_xml_from_fields`,
/// `visa_trim` via `extract_sections_xml(full_text)` (recalcul, pas de stockage).
#[test]
fn xml_chunker_input_reconstructible_from_full_text_and_source_fields() {
    let Ok(dir) = std::env::var("LJ_VERIFY_XML_DIR") else {
        return;
    };
    let (mut n, mut skipped) = (0usize, 0usize);
    let (mut mh_bad, mut vt_bad) = (0usize, 0usize);
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
        let full_text = &d.texte_integral_clean;
        let sf = build_source_fields_xml(&raw);

        // 1. metadata_header reconstruit depuis source_fields.
        if build_metadata_header_xml_from_fields(&sf) != d.metadata_header {
            mh_bad += 1;
            first_bad.get_or_insert_with(|| format!("metadata_header diverge : {path:?}"));
        }

        // 2. visa_trim : sections recalculées sur full_text (pas de stockage).
        let recon_sections = extract_sections_xml(full_text);
        if build_visa_trim_xml(&recon_sections) != d.visa_trim {
            vt_bad += 1;
            first_bad.get_or_insert_with(|| format!("visa_trim diverge : {path:?}"));
        }
    }

    eprintln!(
        "CERTIF XML chunker-input : n={n} skipped={skipped} | \
             metadata_header mismatch={mh_bad} | visa_trim mismatch={vt_bad}"
    );
    if let Some(ex) = first_bad {
        eprintln!("  premier divergent : {ex}");
    }
    assert_eq!(
        mh_bad, 0,
        "metadata_header XML non reconstructible à l'identique"
    );
    assert_eq!(vt_bad, 0, "visa_trim XML non reconstructible à l'identique");
}

#[test]
fn xml_general_entity_refs_are_resolved_in_text() {
    // quick-xml 0.40 emet `&amp;` / `&#160;` / `&#39;` comme events GeneralRef
    // distincts ; le parser doit les resoudre comme lxml cote Python.
    // Source reelle ORCA_24NT03363 : « SARL BONDIGUEL &amp; ASSOCIES ».
    let xml = b"<Document><Dossier>\
<Nom_Juridiction>TA</Nom_Juridiction>\
<Avocat_Requerant>SARL BONDIGUEL &amp; ASSOCIES</Avocat_Requerant>\
</Dossier></Document>";
    let d = parse_xml(xml, "ORCA_1.xml", None);
    assert_eq!(
        d.avocat_requerant.as_deref(),
        Some("SARL BONDIGUEL & ASSOCIES")
    );

    // Refs numeriques : &#160; (NBSP) et &#39; (apostrophe) recolles au texte.
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
    // inconnu → renvoyé tel quel (pas .lower()).
    assert_eq!(
        vocab::jurisdiction_label(Some("XYZ")).as_deref(),
        Some("XYZ")
    );
    assert_eq!(vocab::jurisdiction_label(None), None);
    assert_eq!(vocab::jurisdiction_label(Some("")), None);

    // type inconnu → None (pas de fallback).
    assert_eq!(vocab::jurisdiction_type(Some("zzz")), None);
    assert_eq!(vocab::jurisdiction_type(Some("tj")).as_deref(), Some("TJ"));

    // formation inconnue → upper().
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

#[test]
fn rfind_double_newline() {
    assert_eq!(char_rfind_double_newline("a\n\nb\n\nc"), Some(4));
    assert_eq!(char_rfind_double_newline("abc"), None);
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
    assert_eq!(d.member_name, "abc123"); // member_name None → decision_id
    assert_eq!(d.jurisdiction_source_code, None);
    assert_eq!(d.chamber.as_deref(), Some("soc"));
    assert_eq!(d.jurisdiction_name.as_deref(), Some("Cour de cassation"));
    assert_eq!(d.jurisdiction_type.as_deref(), Some("CC"));
    assert_eq!(d.jurisdiction_location.as_deref(), Some("cc"));
    // dédoublonnage + trim, ordre préservé.
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
    // champs XML-only nuls côté Judilibre.
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
    // raw_text simple sans HTML : clean_texte ≈ identité (collapse espaces).
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

    // visa présent en tête (pas de preamble → insertion_idx 0).
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

    // visa_trim = expose + moyens joints par \n\n.
    assert!(d.visa_trim.contains("Exposé du litige"));
    assert!(d.visa_trim.contains("moyens du pourvoi"));
}

#[test]
fn dispositif_fallback_on_par_ces_motifs() {
    // Pas de zone dispositif : fallback par marqueur « PAR CES MOTIFS ».
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
    // une seule section dispositif (celle de la zone), pas de doublon fallback.
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
    // <br/> imbriqué : itertext() doit recoller les morceaux.
    let xml = b"<Document><Dossier><Nom_Juridiction>Conseil <br/>d'Etat</Nom_Juridiction></Dossier></Document>";
    let d = parse_xml(xml, "DCE_1.xml", None);
    assert_eq!(d.jurisdiction_name.as_deref(), Some("Conseil d'Etat"));
}

#[test]
fn xml_extract_sections_partitions_text() {
    let cleaned = "En-tete introductif.\n\nVu : les pieces.\n\nConsidérant ce qui suit : motifs.\n\nDECIDE :\n\nArticle 1.";
    let sections = extract_sections_xml(cleaned);
    let kinds: Vec<&str> = sections.iter().map(|s| s.kind.as_str()).collect();
    // preamble (intro) puis visa, motivations, dispositif dans l'ordre.
    assert_eq!(kinds, vec!["preamble", "visa", "motivations", "dispositif"]);
    assert_eq!(sections[0].start_char, 0);
    // offsets en codepoints : la dernière section finit au total de chars.
    assert_eq!(sections.last().unwrap().end_char, char_len(cleaned));
}
