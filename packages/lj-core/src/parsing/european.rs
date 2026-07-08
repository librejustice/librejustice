//! Sous-module de `parsing` (#26, découpe ADR 0066). Aucune logique changée :
//! déplacement depuis `parsing.rs`, accès aux helpers partagés via `super`.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Sources européennes : CEDH (HUDOC) et CJUE (EUR-Lex) — ADR 0094.
//
// Parsers PURS : reçoivent le corps déjà strippé (HTML→texte au bord lj-sources)
// + les métadonnées désérialisées (`columns` HUDOC / `predicates` CDM). Le texte
// vit dans `texte_integral_*` ; les métadonnées partent verbatim en `source_fields`
// (construites par le pipeline depuis le même `Value`, hors de `Decision`).
// ─────────────────────────────────────────────────────────────────────────────

/// Lit une chaîne non-vide d'une clé d'un objet JSON (`columns`/`predicates`).
fn json_str_nonempty<'a>(obj: &'a Value, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `kpdate` HUDOC (`1960-11-14T00:00:00`, ISO horodaté) → `YYYY-MM-DD`. On retient
/// les 10 premiers caractères ssi ce sont une date ISO valide (`%Y-%m-%d`) — pas
/// de troncature aveugle (#12). `None` sinon.
fn cedh_kpdate_to_iso(kpdate: &str) -> Option<String> {
    let head = kpdate.get(..10)?;
    jiff::civil::Date::strptime("%Y-%m-%d", head)
        .ok()
        .map(|_| head.to_string())
}

/// `judgementdate` HUDOC (`14/11/1960 00:00:00`, `DD/MM/YYYY` + heure) → `YYYY-MM-DD`.
/// Repli quand `kpdate` manque. `None` si le préfixe n'est pas une date FR valide.
fn cedh_judgementdate_to_iso(judgementdate: &str) -> Option<String> {
    let head = judgementdate.split_whitespace().next()?;
    let d = jiff::civil::Date::strptime("%d/%m/%Y", head).ok()?;
    Some(d.strftime("%Y-%m-%d").to_string())
}

/// Parse un arrêt CEDH (HUDOC) en [`Decision`] (ADR 0094). `body_text` est le
/// corps déjà converti HTML→texte au bord (`lj-sources`) ; `columns` est le bloc
/// `results[].columns` désérialisé ; `itemid` est la PK HUDOC (`001-…`).
///
/// `juridiction_type` = `"CEDH"` (posé explicitement). `ecli` lu verbatim de
/// `columns["ecli"]` (arrêts seulement, `None` sinon) — jamais dérivé.
/// `numero_dossiers` = `columns["appno"]` (`;`-séparé → liste). `date_lecture` =
/// `columns["kpdate"]` normalisé ISO `YYYY-MM-DD` (repli `judgementdate` `DD/MM/YYYY`),
/// car la frontière store ne parse que `%Y-%m-%d`. Le titre (`docname`) et toutes
/// les autres colonnes restent en `source_fields` (verbatim, hors `Decision`).
///
/// Erreur franche [`CoreError::Xml`] si `itemid` ou `body_text` est vide
/// (frontière de validation source unique, AGENTS.md #12).
pub fn parse_cedh(
    body_text: &str,
    columns: &Value,
    itemid: &str,
) -> crate::error::Result<Decision> {
    use crate::error::CoreError;

    if itemid.is_empty() {
        return Err(CoreError::Xml("CEDH: itemid vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CEDH {itemid}: corps vide")));
    }

    let sections = extract_sections_xml(body_text);

    let numero_dossiers = json_str_nonempty(columns, "appno").map(|appno| {
        appno
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let numero_dossier = numero_dossiers.as_ref().and_then(|v| v.first().cloned());

    // `date_lecture` doit sortir en ISO `YYYY-MM-DD` : la frontière store ne parse
    // QUE `%Y-%m-%d` (types.rs, #12) et jette silencieusement tout autre format en
    // NULL. HUDOC livre `kpdate` en ISO horodaté (`1960-11-14T00:00:00`) et
    // `judgementdate` en `DD/MM/YYYY HH:MM:SS` : on les normalise ici, au bord source.
    // kpdate prioritaire (déjà ISO, jour identique au judgementdate sur les arrêts).
    let date_lecture = json_str_nonempty(columns, "kpdate")
        .and_then(cedh_kpdate_to_iso)
        .or_else(|| {
            json_str_nonempty(columns, "judgementdate").and_then(cedh_judgementdate_to_iso)
        });

    Ok(Decision {
        source_uid: format!("cedh/{itemid}"),
        member_name: itemid.to_string(),
        ecli: json_str_nonempty(columns, "ecli").map(str::to_string),
        juridiction_code: None,
        juridiction_nom: Some("Cour européenne des droits de l'homme".to_string()),
        juridiction_type: Some("CEDH".to_string()),
        juridiction_location: None,
        numero_dossier,
        numero_dossiers,
        numero_role: None,
        date_lecture,
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        visa_trim: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// Numéro d'affaire usuel dérivé du CELEX (`62020CJ0560` → `C-560/20`). Le CELEX
/// secteur 6 encode : secteur `6`, année d'affaire (4 chiffres), type
/// (`CJ`/`TJ`/`CO`/`TO`/`CC`…), numéro (chiffres, zéros de tête à retirer). La
/// lettre de la juridiction est l'initiale du type (`C`/`T`/`F`). Repli sur le
/// CELEX brut si la forme n'est pas reconnue (grounding §Parsers purs).
fn celex_to_case_number(celex: &str) -> String {
    let bytes = celex.as_bytes();
    // `6` + 4 chiffres année + ≥1 lettre type + ≥1 chiffre numéro.
    if bytes.len() < 7 || bytes[0] != b'6' || !bytes[1..5].iter().all(u8::is_ascii_digit) {
        return celex.to_string();
    }
    let year = &celex[1..5];
    let rest = &celex[5..];
    let type_end = rest.find(|c: char| c.is_ascii_digit()).filter(|&i| i > 0);
    let Some(type_end) = type_end else {
        return celex.to_string();
    };
    let type_code = &rest[..type_end];
    let number = rest[type_end..].trim_start_matches('0');
    let number = if number.is_empty() { "0" } else { number };
    let Some(court) = type_code.chars().next() else {
        return celex.to_string();
    };
    let yy = &year[2..];
    format!("{court}-{number}/{yy}")
}

/// Parse un arrêt CJUE (EUR-Lex) en [`Decision`] (ADR 0094). `body_text` est le
/// texte FR déjà converti (xhtml/html→texte) au bord (`lj-sources`) ;
/// `predicates` est le bloc de prédicats CDM désérialisé ; `celex` est la PK.
///
/// `juridiction_type` = `"CJUE"` (posé explicitement). `ecli` lu verbatim de
/// `predicates["case-law_ecli"]` (100 % présent, `None` si vide) — **jamais
/// dérivé du CELEX** (l'ECLI dérivé serait faux, audit `cjue.md`). `numero_dossiers`
/// = numéro d'affaire dérivé du CELEX (`C-560/20`), repli sur le CELEX brut.
/// `date_lecture` = `predicates["work_date_document"]`. La bannière « objet » de
/// l'en-tête (titre) et les autres prédicats restent en `source_fields` (verbatim,
/// hors `Decision`).
///
/// Erreur franche [`CoreError::Xml`] si `celex` ou `body_text` est vide
/// (frontière de validation source unique, AGENTS.md #12).
pub fn parse_cjue(
    body_text: &str,
    predicates: &Value,
    celex: &str,
) -> crate::error::Result<Decision> {
    use crate::error::CoreError;

    if celex.is_empty() {
        return Err(CoreError::Xml("CJUE: celex vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CJUE {celex}: corps vide")));
    }

    let sections = extract_sections_xml(body_text);

    let case_number = celex_to_case_number(celex);
    let numero_dossiers = Some(vec![case_number.clone()]);

    Ok(Decision {
        source_uid: format!("cjue/{celex}"),
        member_name: celex.to_string(),
        ecli: json_str_nonempty(predicates, "case-law_ecli").map(str::to_string),
        juridiction_code: None,
        juridiction_nom: Some("Cour de justice de l'Union européenne".to_string()),
        juridiction_type: Some("CJUE".to_string()),
        juridiction_location: None,
        numero_dossier: Some(case_number),
        numero_dossiers,
        numero_role: None,
        date_lecture: json_str_nonempty(predicates, "work_date_document").map(str::to_string),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        visa_trim: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// `true` si `source_uid` provient d'une source HTML européenne (ADR 0094) :
/// préfixe `cedh/` (HUDOC) ou `cjue/` (EUR-Lex), posé par [`parse_cedh`] /
/// [`parse_cjue`]. Discriminant de famille du dispatch
/// [`Decision::from_source_fields`] — sûr car ces préfixes sont stables en DB et
/// jamais portés par les autres fonds (XML opendata, JSON Judilibre, DILA, CNDA).
pub(crate) fn source_uid_is_html_europe(source_uid: &str) -> bool {
    source_uid.starts_with("cedh/") || source_uid.starts_with("cjue/")
}

impl Decision {
    /// Branche HTML européenne (CEDH/CJUE) de [`Decision::from_source_fields`]
    /// (ADR 0094/0085) : reconstruit une `Decision` **identique** à
    /// [`parse_cedh`]/[`parse_cjue`] depuis `(full_text, source_fields)`. Pendant
    /// exact de [`build_source_fields`] côté HTML : `source_fields` = les
    /// métadonnées (colonnes HUDOC / prédicats CDM) verbatim + la clé `sections`
    /// (que les parsers ignorent — ils relisent les sections de `full_text`).
    ///
    /// L'inverse est **structurellement** à 0 écart car ces parsers posent
    /// `texte_integral_clean = texte_integral_raw = body_text` (aucun nettoyage),
    /// `metadata_header = visa_trim = ""` : le chunk ne dépend donc que de
    /// `full_text` (stocké verbatim). On réinvoque le parser dédié sur
    /// `(full_text, source_fields, pk)` — `pk` (`itemid`/`celex`) est dérivé du
    /// `source_uid` (`cedh/<itemid>` / `cjue/<celex>`).
    ///
    /// Panique si le parser échoue : à l'aval, `(full_text, source_fields)`
    /// provient d'une décision déjà ingérée (donc valide) — un échec ici signale
    /// une corruption DB ou un mauvais dispatch, pas une donnée externe (#12).
    pub(crate) fn from_source_fields_html_europe(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        if let Some(itemid) = source_uid.strip_prefix("cedh/") {
            parse_cedh(full_text, source_fields, itemid)
                .unwrap_or_else(|e| panic!("from_source_fields CEDH {source_uid}: {e}"))
        } else if let Some(celex) = source_uid.strip_prefix("cjue/") {
            parse_cjue(full_text, source_fields, celex)
                .unwrap_or_else(|e| panic!("from_source_fields CJUE {source_uid}: {e}"))
        } else {
            panic!("from_source_fields_html_europe: source_uid non HTML européen {source_uid}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── CEDH / CJUE (ADR 0094) ─────────────────────────────────────────────

    #[test]
    fn cedh_maps_fields_ecli_appno_list_and_type() {
        // Fixture HUDOC (audit cedh.md §Champs) : corps déjà strippé + `columns`.
        // appno `;`-séparé → liste ; ECLI pris verbatim (arrêt) ; type explicite.
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25;26238/10",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "kpdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "Vu : la procédure suivie devant la Cour. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
        let d = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");

        assert_eq!(d.source_uid, "cedh/001-250438");
        assert_eq!(d.member_name, "001-250438");
        assert_eq!(d.juridiction_type.as_deref(), Some("CEDH"));
        assert_eq!(
            d.juridiction_nom.as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        // ECLI verbatim, jamais dérivé.
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:CE:ECHR:2026:0611JUD002089016")
        );
        // appno → liste ; numero_dossier = premier.
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(
                &[
                    "20890/16".to_string(),
                    "19185/25".to_string(),
                    "26238/10".to_string()
                ][..]
            )
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("20890/16"));
        // date normalisée ISO `YYYY-MM-DD` au bord source (la frontière store ne
        // parse que `%Y-%m-%d` ; un `…T00:00:00` verbatim finissait NULL).
        assert_eq!(d.date_lecture.as_deref(), Some("2026-06-11"));
        // full_text = body_text tel quel.
        assert_eq!(d.texte_integral_clean, body);
        assert_eq!(d.texte_integral_raw, body);
        // Sections re-détectées sur le texte.
        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"visa"));
        assert!(kinds.contains(&"motivations"));
    }

    #[test]
    fn cedh_ecli_none_when_absent_and_kpdate_fallback() {
        // Note d'info (CLINF) : pas d'ECLI ; date_lecture replie sur kpdate.
        let columns = json!({
            "appno": "10000/20",
            "ecli": "",
            "kpdate": "2026-05-01T00:00:00",
        });
        let d =
            parse_cedh("Résumé juridique du litige.", &columns, "002-14552").expect("parse CEDH");
        assert_eq!(d.ecli, None);
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-01"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["10000/20".to_string()][..])
        );
    }

    #[test]
    fn cedh_date_falls_back_to_judgementdate_ddmmyyyy_normalized_iso() {
        // kpdate absent → repli sur judgementdate `DD/MM/YYYY HH:MM:SS`, normalisé
        // ISO. Sans normalisation, ce format finissait NULL en base (store #12).
        let columns = json!({
            "appno": "332/57",
            "ecli": "ECLI:CE:ECHR:1960:1114JUD000033257",
            "judgementdate": "14/11/1960 00:00:00",
        });
        let d = parse_cedh("Arrêt Lawless.", &columns, "001-57516").expect("parse CEDH");
        assert_eq!(d.date_lecture.as_deref(), Some("1960-11-14"));
    }

    #[test]
    fn cedh_errors_on_empty_itemid_or_body() {
        let columns = json!({ "appno": "1/20" });
        assert!(parse_cedh("texte", &columns, "").is_err());
        assert!(parse_cedh("   ", &columns, "001-1").is_err());
    }

    #[test]
    fn cjue_maps_fields_ecli_verbatim_and_case_number_from_celex() {
        // Fixture EUR-Lex (audit cjue.md §Champs) : ECLI ≠ ECLI dérivable du CELEX.
        // CELEX 62020CJ0560 → affaire C-560/20 ; ECLI réel 2024:96 (jamais dérivé).
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
            "subject-matter": "IMMI",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let d = parse_cjue(body, &predicates, "62020CJ0560").expect("parse CJUE");

        assert_eq!(d.source_uid, "cjue/62020CJ0560");
        assert_eq!(d.member_name, "62020CJ0560");
        assert_eq!(d.juridiction_type.as_deref(), Some("CJUE"));
        // ECLI verbatim — surtout PAS dérivé du CELEX (qui donnerait …2020:560).
        assert_eq!(d.ecli.as_deref(), Some("ECLI:EU:C:2024:96"));
        assert_ne!(d.ecli.as_deref(), Some("ECLI:EU:C:2020:560"));
        // Numéro d'affaire dérivé du CELEX (forme usuelle).
        assert_eq!(d.numero_dossier.as_deref(), Some("C-560/20"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["C-560/20".to_string()][..])
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2024-01-30"));
        assert_eq!(d.texte_integral_clean, body);
    }

    #[test]
    fn cjue_celex_to_case_number_variants() {
        // Tribunal (T), ordonnance (CO/TO), CELEX non reconnu → repli brut.
        assert_eq!(celex_to_case_number("62020CJ0560"), "C-560/20");
        assert_eq!(celex_to_case_number("62023CO0614"), "C-614/23");
        assert_eq!(celex_to_case_number("62023TJ0188"), "T-188/23");
        assert_eq!(celex_to_case_number("62016CJ0550"), "C-550/16");
        // Forme inattendue → repli sur le CELEX brut (pas de dérivation hasardeuse).
        assert_eq!(celex_to_case_number("NOTACELEX"), "NOTACELEX");
        assert_eq!(celex_to_case_number("32020R0560"), "32020R0560");
    }

    #[test]
    fn cjue_ecli_none_when_absent_and_errors_on_empty() {
        let predicates = json!({ "case-law_ecli": "", "work_date_document": "2024-01-30" });
        let d = parse_cjue("texte de l'arrêt", &predicates, "62020CJ0560").expect("parse CJUE");
        assert_eq!(d.ecli, None);

        assert!(parse_cjue("texte", &predicates, "").is_err());
        assert!(parse_cjue("  ", &predicates, "62020CJ0560").is_err());
    }

    #[test]
    fn vocab_cedh_cjue_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_type(Some("cedh")).as_deref(),
            Some("CEDH")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("cjue")).as_deref(),
            Some("CJUE")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cedh")).as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cjue")).as_deref(),
            Some("Cour de justice de l'Union européenne")
        );
    }

    #[test]
    fn vocab_constit_tc_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_label(Some("constit")).as_deref(),
            Some("Conseil constitutionnel")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("tc")).as_deref(),
            Some("Tribunal des conflits")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("constit")).as_deref(),
            Some("CONSTIT")
        );
        assert_eq!(vocab::jurisdiction_type(Some("tc")).as_deref(), Some("TC"));
    }

    // ── Round-trip `from_source_fields` ⟷ parse direct (ADR 0085/0094, #37) ──
    //
    // Spec gate : reconstruire une `Decision` depuis `(full_text, source_fields)`
    // reproduit EXACTEMENT le parse direct du corps HTML. La parité est
    // structurelle (clean=raw=body_text, header/visa vides) — le chunk ne dépend
    // que de `full_text`, stocké verbatim. `source_fields` = métadonnées verbatim
    // + `sections` (via `build_source_fields`, calque exact de l'ingest html).

    #[test]
    fn cedh_round_trips_via_source_fields() {
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "Vu : la procédure suivie devant la Cour. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
        let orig = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");
        // `source_fields` calqué sur l'ingest html (métadonnées + sections rebasées).
        let sf = build_source_fields(&columns, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }

    #[test]
    fn cjue_round_trips_via_source_fields() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let orig = parse_cjue(body, &predicates, "62020CJ0560").expect("parse CJUE");
        let sf = build_source_fields(&predicates, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }
}
