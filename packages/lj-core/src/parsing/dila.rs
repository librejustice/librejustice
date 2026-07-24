//! Sous-module de `parsing` (#26, découpe ADR 0066). Aucune logique changée :
//! déplacement depuis `parsing.rs`, accès aux helpers partagés via `super`.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// DILA bulk (JADE / CONSTIT / CNIL) — schémas `<TEXTE_JURI_{ADMIN,CONSTIT}>` et
// `<TEXTE_CNIL>` (ADR 0093/0185). Octets déjà réparés au bord lj-sources
// (de-escape `&amp;nbsp;`, mojibake par sous-arbre) ; le parser reste PUR.
// ─────────────────────────────────────────────────────────────────────────────

/// Fonds bulk DILA. Sélectionne le sous-bloc `META_JURI_*` et le routage de
/// juridiction (ADR 0093).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DilaFond {
    /// Jurisprudence administrative (CE / CAA / TC), sous-bloc `META_JURI_ADMIN`.
    Jade,
    /// Conseil constitutionnel, sous-bloc `META_JURI_CONSTIT`.
    Constit,
    /// Délibérations/décisions de la CNIL (ADR 0185), sous-bloc `META_CNIL`. Schéma
    /// `TEXTE_CNIL` sans `META_JURI` : date en `META_CNIL/DATE_TEXTE`, numéro en
    /// `META_CNIL/NUMERO`, pas d'ECLI. `jurisdiction_type = CNIL`.
    Cnil,
}

impl DilaFond {
    /// Tag du sous-bloc juridiction-spécifique (`META/META_SPEC/<…>`).
    fn meta_juri_sub_tag(self) -> &'static str {
        match self {
            DilaFond::Jade => "META_JURI_ADMIN",
            DilaFond::Constit => "META_JURI_CONSTIT",
            DilaFond::Cnil => "META_CNIL",
        }
    }
}

/// Libellé de l'émetteur CNIL (ADR 0185) : le schéma `META_CNIL` ne porte pas de
/// `JURIDICTION`, on le pose ici pour nourrir le `metadata_header`.
const CNIL_JURIDICTION_NOM: &str = "Commission nationale de l'informatique et des libertés";

/// `jurisdiction_type` JADE depuis le préfixe `ANCIEN_ID` (`JG`→CE, `J0..J7`→CAA,
/// `JC`→TC), fallback sur le libellé `JURIDICTION` (audit `dila-jade.md`).
fn jade_jurisdiction_type(ancien_id: Option<&str>, juridiction: Option<&str>) -> Option<String> {
    if let Some(id) = ancien_id.filter(|s| !s.is_empty()) {
        // Préfixe = tout avant le premier `_` (`JG_L_2026_…`, `J1_L_2026_…`).
        let prefix = id.split('_').next().unwrap_or(id);
        if prefix == "JG" {
            return Some("CE".to_string());
        }
        if prefix == "JC" {
            return Some("TC".to_string());
        }
        if let Some(digit) = prefix.strip_prefix('J') {
            if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) {
                return Some("CAA".to_string());
            }
        }
    }
    let jur = juridiction?.to_lowercase();
    if jur.contains("tribunal des conflits") {
        Some("TC".to_string())
    } else if jur.contains("cour administrative d'appel") || jur.starts_with("caa") {
        Some("CAA".to_string())
    } else if jur.contains("conseil d'état") || jur.contains("conseil d'etat") {
        Some("CE".to_string())
    } else {
        None
    }
}

/// `NUMERO` composite DILA → liste de numéros. Split sur `/` ; un préfixe `YYYY-`
/// en tête est propagé aux tokens nus (`2026-321/322/323` → `[2026-321,
/// 2026-322, 2026-323]`, audit `dila-constit.md`, grounding décision #4).
fn parse_numero_composite(numero: &str) -> Vec<String> {
    let mut prefix: Option<String> = None;
    let mut out: Vec<String> = Vec::new();
    for token in numero.split('/') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((head, _)) = token.split_once('-') {
            // Préfixe `YYYY-` du premier token composite (`2026-321`) propagé.
            if head.len() == 4 && head.chars().all(|c| c.is_ascii_digit()) {
                prefix = Some(head.to_string());
            }
            out.push(token.to_string());
        } else if let Some(p) = &prefix {
            out.push(format!("{p}-{token}"));
        } else {
            out.push(token.to_string());
        }
    }
    out
}

/// `source_fields` DILA (ADR 0085), calqué sur [`build_source_fields_xml`] : tous
/// les scalaires feuilles de `META_COMMUN` + `META_JURI` + le sous-bloc du fond,
/// groupés par nœud parent, plus `SCT`/`ANA`/`CITATION_JP`/`LIEN` versés
/// **verbatim**. Le texte intégral part dans `full_text` ; sections et
/// `metadata_header` se recalculent sur `full_text`, rien à rebaser ici.
pub fn build_source_fields_dila(raw: &[u8], fond: DilaFond) -> Value {
    let root = build_tree(raw).unwrap_or_default();
    let mut obj = serde_json::Map::new();

    let mut put_scalars = |path: &str, key: &str| {
        if let Some(node) = root.find(path) {
            let scalars = xml_scalar_children(node);
            if !scalars.is_empty() {
                obj.insert(key.to_string(), Value::Object(scalars));
            }
        }
    };
    put_scalars("META/META_COMMUN", "META_COMMUN");
    put_scalars("META/META_SPEC/META_JURI", "META_JURI");
    let sub_tag = fond.meta_juri_sub_tag();
    put_scalars(&format!("META/META_SPEC/{sub_tag}"), sub_tag);

    // Sous-arbres sémantiques versés verbatim (le texte agrégé `itertext`).
    let mut put_verbatim = |paths: &[&str], key: &str| {
        if let Some(text) = root.find_first(paths).and_then(XmlNode::text) {
            obj.insert(key.to_string(), Value::String(text));
        }
    };
    put_verbatim(&["TEXTE/SOMMAIRE/SCT"], "SCT");
    put_verbatim(&["TEXTE/SOMMAIRE/ANA"], "ANA");
    put_verbatim(
        &["TEXTE/CITATION_JP/CONTENU", "TEXTE/CITATION_JP"],
        "CITATION_JP",
    );
    put_verbatim(&["LIENS/LIEN", "LIENS"], "LIEN");

    Value::Object(obj)
}

/// `true` si `source_fields` provient d'un payload DILA bulk (ADR 0093) :
/// présence des nœuds `META_COMMUN` + `META_JURI` (calqués par
/// [`build_source_fields_dila`], qu'aucun payload XML opendata `<Dossier>` ni
/// JSON Judilibre n'a jamais). Discriminant de famille du dispatch
/// [`Decision::from_source_fields`].
pub(crate) fn source_fields_is_dila(source_fields: &Value) -> bool {
    source_fields.get("META_COMMUN").is_some()
        && (source_fields.get("META_JURI").is_some() || source_fields.get("META_CNIL").is_some())
}

/// Fond DILA déduit du `source_uid` (pivot ADR 0093 : `dila-jade/<ID>` /
/// `dila-constit/<ID>`). C'est le discriminant le plus sûr — il colle au préfixe
/// `source_prefix` posé à l'ingest (`apps/lj-ingest/src/pipeline/dila.rs`), donc
/// stable en DB. Repli sur la forme du `source_fields` (sous-bloc
/// `META_JURI_CONSTIT` ⇒ Constit) si le préfixe est absent, puis défaut Jade.
fn dila_fond_from_uid(source_uid: &str, source_fields: &Value) -> DilaFond {
    if source_uid.starts_with("dila-constit") {
        return DilaFond::Constit;
    }
    if source_uid.starts_with("dila-cnil") {
        return DilaFond::Cnil;
    }
    if source_uid.starts_with("dila-jade") {
        return DilaFond::Jade;
    }
    if source_fields.get("META_CNIL").is_some() {
        DilaFond::Cnil
    } else if source_fields.get("META_JURI_CONSTIT").is_some() {
        DilaFond::Constit
    } else {
        DilaFond::Jade
    }
}

impl Decision {
    /// Branche DILA bulk de [`Decision::from_source_fields`] (ADR 0093/0085) :
    /// reconstruit une `Decision` **identique** à [`parse_dila_xml`] depuis
    /// `(full_text, source_fields)`, sans toucher le brut. Pendant exact de
    /// [`build_source_fields_dila`] : on relit les mêmes scalaires
    /// `META_COMMUN`/`META_JURI`/sous-bloc et on recalcule
    /// sections/`metadata_header`/`visa_trim` sur `full_text` (= `texte_integral_clean`).
    /// `raw == clean == full_text` ; `date_mise_jour` n'est pas porté (donc `None`).
    /// Le fond (Jade/Constit) se déduit du `source_uid` ([`dila_fond_from_uid`]).
    pub(crate) fn from_source_fields_dila(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        let fond = dila_fond_from_uid(source_uid, source_fields);
        let sub_tag = fond.meta_juri_sub_tag();

        // Lecteur scalaire d'un sous-bloc capté par `build_source_fields_dila`
        // (`xml_scalar_children` = première occurrence par tag, comme `find_first`).
        let get = |block: &str, key: &str| -> Option<String> {
            source_fields
                .get(block)
                .and_then(|node| node.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };

        // CNIL : date/numéro dans `META_CNIL` (pas de `META_JURI`, ADR 0185).
        let date_dec = match fond {
            DilaFond::Cnil => get("META_CNIL", "DATE_TEXTE"),
            _ => get("META_JURI", "DATE_DEC"),
        }
        .unwrap_or_default();
        let jurisdiction_name = match fond {
            DilaFond::Cnil => Some(CNIL_JURIDICTION_NOM.to_string()),
            _ => get("META_JURI", "JURIDICTION"),
        };
        let jurisdiction_type = match fond {
            DilaFond::Constit => Some("CONSTIT".to_string()),
            DilaFond::Cnil => Some("CNIL".to_string()),
            DilaFond::Jade => jade_jurisdiction_type(
                get("META_COMMUN", "ANCIEN_ID").as_deref(),
                jurisdiction_name.as_deref(),
            ),
        };

        // ECLI : META_COMMUN/ECLI (CONSTIT) ou sous-bloc/ECLI (JADE CE) — même
        // ordre de repli que `parse_dila_xml`.
        let ecli = get("META_COMMUN", "ECLI")
            .or_else(|| get(sub_tag, "ECLI"))
            .filter(|s| !s.is_empty());

        let numbers = match fond {
            DilaFond::Cnil => get("META_CNIL", "NUMERO"),
            _ => get("META_JURI", "NUMERO"),
        }
        .as_deref()
        .map(parse_numero_composite)
        .unwrap_or_default();
        let numero_dossier = numbers.first().cloned();
        let numero_dossiers = (!numbers.is_empty()).then_some(numbers);

        let formation = get(sub_tag, "FORMATION");
        let type_recours = get(sub_tag, "TYPE_REC");
        let avocat_requerant = get(sub_tag, "AVOCATS");
        let solution = get("META_JURI", "SOLUTION");
        // PUBLI_RECUEIL (JADE) : classement Lebon A/B/C — la facette publication.
        let publication_codes = get(sub_tag, "PUBLI_RECUEIL")
            .filter(|s| !s.is_empty())
            .map(|c| vec![c])
            .unwrap_or_default();

        let sections = extract_sections_xml(full_text);
        let metadata_header = assemble_metadata_header_xml(
            jurisdiction_name.clone(),
            Some(date_dec.clone()),
            None,
            type_recours.clone(),
            solution.clone(),
            formation.clone(),
        );
        let visa_trim = build_visa_trim_xml(&sections);

        Decision {
            source_uid: source_uid.to_string(),
            member_name: source_uid.to_string(),
            ecli,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name,
            jurisdiction_type,
            jurisdiction_location: None,
            numero_dossier,
            numero_dossiers,
            numero_role: None,
            date_lecture: Some(date_dec),
            date_audience: None,
            date_mise_jour: None,
            formation,
            type_decision: None,
            type_recours,
            solution,
            publication_codes,
            avocat_requerant,
            texte_integral_raw: full_text.to_string(),
            texte_integral_clean: full_text.to_string(),
            sections,
            metadata_header,
            visa_trim,
            themes: Vec::new(),
            attacked: None,
            parse_warnings: Vec::new(),
        }
    }
}

/// Issue de [`parse_dila_doc`] : un membre DILA porte soit son **texte intégral**
/// (`BLOC_TEXTUEL/CONTENU`), soit, pour ~10 k vieux fonds JADE (CE pré-2002), une
/// **analyse seule** (`SOMMAIRE/ANA` + `SCT`, mention aux tables) sans texte. Les
/// deux donnent une [`Decision`] complète côté métadonnées ; elles diffèrent par
/// l'origine du `full_text` et par leur traitement à l'ingest (#33, ADR 0105) :
/// `Full` = upsert normal (texte canonique) ; `Analysis` = soit enrichissement
/// `source_fields` d'une décision existante (rattachement), soit décision orpheline
/// dont l'analyse EST le contenu cherchable (jamais d'écrasement d'un texte réel).
pub enum DilaDoc {
    Full(Decision),
    Analysis(Decision),
}

/// Parse un XML bulk DILA (`fond`) en [`Decision`] **à texte intégral** (ADR 0093).
/// Pendant strict de [`parse_dila_doc`] restreint au cas `Full` : erreur franche si
/// `CONTENU` est absent (un membre analyse-seule n'est PAS un texte). Conservé comme
/// porte d'entrée des consommateurs qui exigent un texte réel — l'oracle de parité
/// du banc (`lj-bench`) et les round-trips ci-dessous — pour qu'ils ne voient jamais
/// une `Decision` au `full_text` synthétisé depuis l'analyse.
pub fn parse_dila_xml(
    raw: &[u8],
    member_path: &str,
    fond: DilaFond,
) -> crate::error::Result<Decision> {
    match parse_dila_doc(raw, member_path, fond)? {
        Some(DilaDoc::Full(d)) => Ok(d),
        Some(DilaDoc::Analysis(_)) | None => Err(crate::error::CoreError::Xml(format!(
            "DILA {member_path}: BLOC_TEXTUEL/CONTENU absent"
        ))),
    }
}

/// Parse un XML bulk DILA (`fond`) en [`DilaDoc`] (ADR 0093/0105). Les octets sont
/// supposés déjà réparés au bord (lj-sources). Lit le schéma commun
/// `META/META_COMMUN`, `META/META_SPEC/META_JURI` et le sous-bloc
/// `META_JURI_{ADMIN,CONSTIT}`. Si `BLOC_TEXTUEL/CONTENU` est présent et non vide →
/// [`DilaDoc::Full`], `full_text = clean_texte(CONTENU)`. Sinon, si `SOMMAIRE`
/// (`SCT`/`ANA`) porte du texte → [`DilaDoc::Analysis`], `full_text` = `SCT` puis
/// `ANA` joints (mention aux tables + analyse), nettoyés et cherchables. Sections
/// re-détectées sur le `full_text` retenu ; `member_path` préfixe la provenance.
///
/// Erreur franche [`CoreError::Xml`] si `ID` / `DATE_DEC` absents (XML malformé,
/// frontière de validation source unique, AGENTS.md #12). `Ok(None)` si **ni**
/// `CONTENU` **ni** `SOMMAIRE` ne portent de texte : membre sans corps, rien à
/// ingérer — cas **nominal** du fond CNIL, dont ~64 % des entrées sont des fiches
/// de registre (autorisations de transfert/recherche) publiées sans texte (ADR
/// 0185). L'appelant le compte en skip, pas en erreur.
pub fn parse_dila_doc(
    raw: &[u8],
    member_path: &str,
    fond: DilaFond,
) -> crate::error::Result<Option<DilaDoc>> {
    use crate::error::CoreError;

    let root = build_tree(raw).unwrap_or_default();

    let meta_commun = root.find("META/META_COMMUN");
    let meta_juri = root.find("META/META_SPEC/META_JURI");
    let sub_tag = fond.meta_juri_sub_tag();
    let meta_sub = root.find(&format!("META/META_SPEC/{sub_tag}"));

    let id = meta_commun
        .and_then(|m| node_text(m.find_first(&["ID"])))
        .ok_or_else(|| CoreError::Xml(format!("DILA {member_path}: META_COMMUN/ID absent")))?;
    // CNIL : date dans `META_CNIL/DATE_TEXTE` (pas de `META_JURI`, ADR 0185).
    let date_dec = match fond {
        DilaFond::Cnil => meta_sub.and_then(|m| node_text(m.find_first(&["DATE_TEXTE"]))),
        _ => meta_juri.and_then(|m| node_text(m.find_first(&["DATE_DEC"]))),
    }
    .ok_or_else(|| {
        CoreError::Xml(format!(
            "DILA {member_path}: date de décision (DATE_DEC/DATE_TEXTE) absente"
        ))
    })?;

    // Texte intégral si présent ; sinon repli sur l'analyse (SCT + ANA) pour les
    // fonds analyse-seule (#33). `is_analysis` discrimine le `DilaDoc` retourné.
    // JADE/CONSTIT enveloppent le corps dans `<TEXTE>` ; CNIL non — `BLOC_TEXTUEL`
    // pend directement sous `<TEXTE_CNIL>` (ADR 0185). On essaie les deux chemins.
    let contenu = root
        .find_first(&["TEXTE/BLOC_TEXTUEL/CONTENU", "BLOC_TEXTUEL/CONTENU"])
        .and_then(XmlNode::text)
        .filter(|s| !s.trim().is_empty());
    let (texte_integral_raw, is_analysis) = match contenu {
        Some(c) => (c, false),
        None => {
            let sct = root
                .find_first(&["TEXTE/SOMMAIRE/SCT", "SOMMAIRE/SCT"])
                .and_then(XmlNode::text);
            let ana = root
                .find_first(&["TEXTE/SOMMAIRE/ANA", "SOMMAIRE/ANA"])
                .and_then(XmlNode::text);
            let analysis = [sct, ana]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if analysis.is_empty() {
                // Membre sans corps (ni CONTENU ni SOMMAIRE) : nominal pour CNIL
                // (fiches de registre sans texte, ADR 0185) → skip, pas erreur.
                return Ok(None);
            }
            (analysis, true)
        }
    };

    let texte_integral_clean = clean_texte(&texte_integral_raw);
    let sections = extract_sections_xml(&texte_integral_clean);

    let jurisdiction_name = match fond {
        DilaFond::Cnil => Some(CNIL_JURIDICTION_NOM.to_string()),
        _ => meta_juri.and_then(|m| node_text(m.find_first(&["JURIDICTION"]))),
    };
    let jurisdiction_type = match fond {
        DilaFond::Constit => Some("CONSTIT".to_string()),
        DilaFond::Cnil => Some("CNIL".to_string()),
        DilaFond::Jade => jade_jurisdiction_type(
            meta_commun
                .and_then(|m| node_text(m.find_first(&["ANCIEN_ID"])))
                .as_deref(),
            jurisdiction_name.as_deref(),
        ),
    };

    // ECLI : META_COMMUN/ECLI (CONSTIT) ou META_JURI_ADMIN/ECLI (JADE CE).
    let ecli = meta_commun
        .and_then(|m| node_text(m.find_first(&["ECLI"])))
        .or_else(|| meta_sub.and_then(|m| node_text(m.find_first(&["ECLI"]))))
        .filter(|s| !s.is_empty());

    let numero_raw = match fond {
        DilaFond::Cnil => meta_sub.and_then(|m| node_text(m.find_first(&["NUMERO"]))),
        _ => meta_juri.and_then(|m| node_text(m.find_first(&["NUMERO"]))),
    };
    let numbers = numero_raw
        .as_deref()
        .map(parse_numero_composite)
        .unwrap_or_default();
    let numero_dossier = numbers.first().cloned();
    let numero_dossiers = (!numbers.is_empty()).then_some(numbers);

    // FORMATION : sous-bloc (JADE `META_JURI_ADMIN`) ; CONSTIT n'en a pas.
    let formation = meta_sub.and_then(|m| node_text(m.find_first(&["FORMATION"])));
    let type_recours = meta_sub.and_then(|m| node_text(m.find_first(&["TYPE_REC"])));
    let avocat_requerant = meta_sub.and_then(|m| node_text(m.find_first(&["AVOCATS"])));
    let solution = meta_juri.and_then(|m| node_text(m.find_first(&["SOLUTION"])));
    // PUBLI_RECUEIL (JADE) : classement Lebon A/B/C — la facette publication.
    let publication_codes = meta_sub
        .and_then(|m| node_text(m.find_first(&["PUBLI_RECUEIL"])))
        .filter(|s| !s.is_empty())
        .map(|c| vec![c])
        .unwrap_or_default();

    let metadata_header = assemble_metadata_header_xml(
        jurisdiction_name.clone(),
        Some(date_dec.clone()),
        None,
        type_recours.clone(),
        solution.clone(),
        formation.clone(),
    );
    let visa_trim = build_visa_trim_xml(&sections);

    let decision = Decision {
        source_uid: format!("{member_path}/{id}"),
        member_name: member_path.to_string(),
        ecli,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name,
        jurisdiction_type,
        jurisdiction_location: None,
        numero_dossier,
        numero_dossiers,
        numero_role: None,
        date_lecture: Some(date_dec),
        date_audience: None,
        date_mise_jour: None,
        formation,
        type_decision: None,
        type_recours,
        solution,
        publication_codes,
        avocat_requerant,
        texte_integral_raw,
        texte_integral_clean,
        sections,
        metadata_header,
        visa_trim,
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    };
    Ok(Some(if is_analysis {
        DilaDoc::Analysis(decision)
    } else {
        DilaDoc::Full(decision)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── DILA bulk (ADR 0093) ─────────────────────────────────────────────────

    #[test]
    fn dila_numero_composite_propagates_year_prefix() {
        // Grounding décision #4 : `2026-321/322/323` → 3 numéros avec préfixe.
        assert_eq!(
            parse_numero_composite("2026-321/322/323"),
            vec!["2026-321", "2026-322", "2026-323"]
        );
        // Numéro simple → liste à un élément.
        assert_eq!(parse_numero_composite("493597"), vec!["493597"]);
        // Pas de préfixe YYYY- en tête → tokens conservés tels quels.
        assert_eq!(parse_numero_composite("96-381"), vec!["96-381"]);
    }

    #[test]
    fn dila_jade_jurisdiction_type_from_ancien_id() {
        // Préfixe ANCIEN_ID (audit dila-jade.md) : JG→CE, J0..J7→CAA, JC→TC.
        assert_eq!(
            jade_jurisdiction_type(Some("JG_L_2026_05_000000493597"), None).as_deref(),
            Some("CE")
        );
        assert_eq!(
            jade_jurisdiction_type(Some("J1_L_2026_05_00024PA01128"), None).as_deref(),
            Some("CAA")
        );
        assert_eq!(
            jade_jurisdiction_type(Some("JC_L_2026_000000"), None).as_deref(),
            Some("TC")
        );
        // Fallback sur le libellé JURIDICTION quand ANCIEN_ID absent.
        assert_eq!(
            jade_jurisdiction_type(None, Some("Tribunal des Conflits")).as_deref(),
            Some("TC")
        );
        assert_eq!(
            jade_jurisdiction_type(None, Some("Conseil d'État")).as_deref(),
            Some("CE")
        );
    }

    #[test]
    fn dila_jade_maps_fields_ecli_and_sections() {
        // Fixture JADE (audit dila-jade.md) : ECLI en META_JURI_ADMIN, ANCIEN_ID
        // JG (CE), SOLUTION vide, FORMATION/AVOCATS dans le sous-bloc.
        let xml = r#"<TEXTE_JURI_ADMIN>
<META>
  <META_COMMUN>
    <ID>CETATEXT000054148459</ID>
    <ANCIEN_ID>JG_L_2026_05_000000493597</ANCIEN_ID>
    <ORIGINE>CETAT</ORIGINE>
    <NATURE>Texte</NATURE>
  </META_COMMUN>
  <META_SPEC>
    <META_JURI>
      <TITRE>Conseil d'État, 6ème - 5ème chambres réunies, 27/05/2026, 493597</TITRE>
      <DATE_DEC>2026-05-27</DATE_DEC>
      <JURIDICTION>Conseil d'État</JURIDICTION>
      <NUMERO>493597</NUMERO>
      <SOLUTION/>
    </META_JURI>
    <META_JURI_ADMIN>
      <FORMATION>6ème - 5ème chambres réunies</FORMATION>
      <TYPE_REC>excès de pouvoir</TYPE_REC>
      <AVOCATS>SCP PIWNICA &amp; MOLINIE</AVOCATS>
      <ECLI>ECLI:FR:CECHR:2026:493597.20260527</ECLI>
    </META_JURI_ADMIN>
  </META_SPEC>
</META>
<TEXTE>
  <BLOC_TEXTUEL>
    <CONTENU>Vu : la requête.&lt;br/&gt;&lt;br/&gt;Considérant ce qui suit : motifs.&lt;br/&gt;&lt;br/&gt;DECIDE :&lt;br/&gt;&lt;br/&gt;Article 1.</CONTENU>
  </BLOC_TEXTUEL>
  <SOMMAIRE>
    <SCT>29-03-02 ENERGIE.</SCT>
    <ANA>Une analyse juridique.</ANA>
  </SOMMAIRE>
</TEXTE>
</TEXTE_JURI_ADMIN>"#
            .as_bytes();
        let d = parse_dila_xml(xml, "JADE/jade/CETATEXT000054148459.xml", DilaFond::Jade)
            .expect("parse JADE");
        assert_eq!(
            d.source_uid,
            "JADE/jade/CETATEXT000054148459.xml/CETATEXT000054148459"
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CE"));
        assert_eq!(d.jurisdiction_name.as_deref(), Some("Conseil d'État"));
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-27"));
        assert_eq!(d.numero_dossier.as_deref(), Some("493597"));
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:FR:CECHR:2026:493597.20260527")
        );
        assert_eq!(d.formation.as_deref(), Some("6ème - 5ème chambres réunies"));
        assert_eq!(d.avocat_requerant.as_deref(), Some("SCP PIWNICA & MOLINIE"));
        // SOLUTION vide JADE → None.
        assert_eq!(d.solution, None);
        // Sections re-détectées sur le full_text plat.
        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"visa"));
        assert!(kinds.contains(&"motivations"));
        assert!(kinds.contains(&"dispositif"));
        // source_fields : SCT/ANA versés verbatim + sous-bloc présent.
        let sf = build_source_fields_dila(xml, DilaFond::Jade);
        assert_eq!(
            sf.get("SCT").and_then(Value::as_str),
            Some("29-03-02 ENERGIE.")
        );
        assert_eq!(
            sf.get("ANA").and_then(Value::as_str),
            Some("Une analyse juridique.")
        );
        assert!(sf.get("META_JURI_ADMIN").is_some());
        assert!(sf.get("META_COMMUN").is_some());
    }

    #[test]
    fn dila_constit_maps_ecli_numero_composite_and_type() {
        // Fixture CONSTIT (audit dila-constit.md) : ECLI META_COMMUN, NUMERO
        // composite, SOLUTION normalisée, sous-bloc META_JURI_CONSTIT.
        let xml = r#"<TEXTE_JURI_CONSTIT>
<META>
  <META_COMMUN>
    <ID>CONSTEXT000054148611</ID>
    <ORIGINE>CONSTIT</ORIGINE>
    <NATURE>L</NATURE>
    <ECLI>ECLI:FR:CC:2026:2026.321.322.323.L</ECLI>
  </META_COMMUN>
  <META_SPEC>
    <META_JURI>
      <TITRE>Nature juridique de dispositions</TITRE>
      <DATE_DEC>2026-05-22</DATE_DEC>
      <JURIDICTION>Conseil constitutionnel</JURIDICTION>
      <NUMERO>2026-321/322/323</NUMERO>
      <SOLUTION>Réglementaire</SOLUTION>
    </META_JURI>
    <META_JURI_CONSTIT>
      <NATURE_QUALIFIEE>L</NATURE_QUALIFIEE>
      <NOR>CSCL2613769S</NOR>
      <TITRE_JO>JORF n°0120 du 23 mai 2026, texte n° 79</TITRE_JO>
    </META_JURI_CONSTIT>
  </META_SPEC>
</META>
<TEXTE>
  <BLOC_TEXTUEL>
    <CONTENU>LE CONSEIL CONSTITUTIONNEL. Considérant ce qui suit : motifs. DECIDE : Article 1.</CONTENU>
  </BLOC_TEXTUEL>
</TEXTE>
</TEXTE_JURI_CONSTIT>"#
            .as_bytes();
        let d = parse_dila_xml(xml, "CONSTIT/CONSTEXT000054148611.xml", DilaFond::Constit)
            .expect("parse CONSTIT");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CONSTIT"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Conseil constitutionnel")
        );
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:FR:CC:2026:2026.321.322.323.L")
        );
        // NUMERO composite propagé.
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(
                &[
                    "2026-321".to_string(),
                    "2026-322".to_string(),
                    "2026-323".to_string()
                ][..]
            )
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("2026-321"));
        assert_eq!(d.solution.as_deref(), Some("Réglementaire"));
        // CONSTIT n'a pas de FORMATION dans le sous-bloc.
        assert_eq!(d.formation, None);
        let sf = build_source_fields_dila(xml, DilaFond::Constit);
        let mjc = sf
            .get("META_JURI_CONSTIT")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(
            mjc.get("NATURE_QUALIFIEE").and_then(Value::as_str),
            Some("L")
        );
        assert_eq!(mjc.get("NOR").and_then(Value::as_str), Some("CSCL2613769S"));
    }

    #[test]
    fn dila_missing_required_fields_error() {
        // Erreur franche si ID / DATE_DEC absents (XML malformé, AGENTS.md #12).
        let no_id = r#"<TEXTE_JURI_CONSTIT><META><META_COMMUN/><META_SPEC><META_JURI><DATE_DEC>2026-01-01</DATE_DEC></META_JURI></META_SPEC></META><TEXTE><BLOC_TEXTUEL><CONTENU>x</CONTENU></BLOC_TEXTUEL></TEXTE></TEXTE_JURI_CONSTIT>"#.as_bytes();
        assert!(parse_dila_doc(no_id, "m", DilaFond::Constit).is_err());

        // Membre sans corps (ni CONTENU ni SOMMAIRE) mais ID/date présents → `Ok(None)`
        // (skip nominal, ADR 0185, fiches de registre CNIL) ; l'entrée stricte
        // `parse_dila_xml` (oracle parité, exige un texte) le refuse toujours.
        let empty = r#"<TEXTE_JURI_CONSTIT><META><META_COMMUN><ID>CONSTEXT1</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>2026-01-01</DATE_DEC></META_JURI></META_SPEC></META><TEXTE><BLOC_TEXTUEL/></TEXTE></TEXTE_JURI_CONSTIT>"#.as_bytes();
        assert!(matches!(
            parse_dila_doc(empty, "m", DilaFond::Constit),
            Ok(None)
        ));
        assert!(parse_dila_xml(empty, "m", DilaFond::Constit).is_err());
    }

    #[test]
    fn dila_doc_full_when_contenu_present() {
        // CONTENU présent → DilaDoc::Full (texte intégral canonique).
        let xml = r#"<TEXTE_JURI_ADMIN><META><META_COMMUN><ID>CETATEXT1</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>2026-05-27</DATE_DEC><JURIDICTION>Conseil d'État</JURIDICTION><NUMERO>493597</NUMERO></META_JURI><META_JURI_ADMIN/></META_SPEC></META><TEXTE><BLOC_TEXTUEL><CONTENU>Considérant ce qui suit : motifs.</CONTENU></BLOC_TEXTUEL><SOMMAIRE><SCT>29-03-02 ENERGIE.</SCT><ANA>Une analyse.</ANA></SOMMAIRE></TEXTE></TEXTE_JURI_ADMIN>"#.as_bytes();
        match parse_dila_doc(xml, "dila-jade", DilaFond::Jade)
            .expect("parse")
            .expect("doc présent (CONTENU non vide)")
        {
            DilaDoc::Full(d) => {
                // full_text = CONTENU (pas le SOMMAIRE), même si une analyse existe.
                assert!(d.texte_integral_clean.contains("Considérant ce qui suit"));
                assert!(!d.texte_integral_clean.contains("Une analyse"));
            }
            DilaDoc::Analysis(_) => panic!("attendu Full (CONTENU présent)"),
        }
    }

    #[test]
    fn dila_doc_analysis_when_contenu_absent() {
        // #33 : vieux fond JADE (CE pré-2002) sans BLOC_TEXTUEL — SOMMAIRE seul.
        // → DilaDoc::Analysis, full_text = SCT + ANA joints (cherchable), métadonnées
        // pleines (juridiction CE par repli sur le libellé, date, numéro).
        let xml = r#"<TEXTE_JURI_ADMIN><META><META_COMMUN><ID>CETATEXT0000001</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>1995-03-10</DATE_DEC><JURIDICTION>Conseil d'État</JURIDICTION><NUMERO>123456</NUMERO></META_JURI><META_JURI_ADMIN/></META_SPEC></META><TEXTE><SOMMAIRE><SCT>26-01 PROCEDURE.</SCT><ANA>Le Conseil d'État juge que la requête est recevable.</ANA></SOMMAIRE></TEXTE></TEXTE_JURI_ADMIN>"#.as_bytes();
        match parse_dila_doc(xml, "dila-jade", DilaFond::Jade)
            .expect("parse")
            .expect("doc présent (SOMMAIRE)")
        {
            DilaDoc::Analysis(d) => {
                assert_eq!(d.source_uid, "dila-jade/CETATEXT0000001");
                assert_eq!(d.jurisdiction_type.as_deref(), Some("CE"));
                assert_eq!(d.date_lecture.as_deref(), Some("1995-03-10"));
                assert_eq!(d.numero_dossier.as_deref(), Some("123456"));
                // full_text = SCT puis ANA, joints et cherchables.
                assert!(d.texte_integral_clean.contains("26-01 PROCEDURE."));
                assert!(d
                    .texte_integral_clean
                    .contains("Le Conseil d'État juge que la requête est recevable."));
            }
            DilaDoc::Full(_) => panic!("attendu Analysis (CONTENU absent)"),
        }
        // L'entrée stricte `parse_dila_xml` refuse une analyse-seule (oracle parité).
        assert!(parse_dila_xml(xml, "dila-jade", DilaFond::Jade).is_err());
    }

    // ── Round-trip `from_source_fields` ⟷ `parse_dila_xml` (ADR 0085, #34) ────
    //
    // Spec gate : reconstruire une `Decision` depuis `(full_text, source_fields)`
    // reproduit EXACTEMENT le parse direct du brut, hors deux champs non portés
    // par les colonnes DB (exclusions documentées, cf. oracle extract-fields-parity) :
    // `member_name` (provenance pure, fixé à `source_uid` par le chemin linéaire)
    // et `texte_integral_raw` (le brut XML `<br/>` n'est pas stocké côté DILA ;
    // seul `full_text` = clean l'est). Garantit qu'un re-embed depuis les colonnes
    // DB ne change pas les 3 entrées du chunker (`texte_integral_clean`,
    // `metadata_header`, `visa_trim`) ni les sections.

    /// Compare `from_source_fields_dila(clean, build_source_fields_dila(xml))` au
    /// parse direct, hors `member_name` + `texte_integral_raw` — gate 0 écart.
    fn assert_dila_round_trip(xml: &[u8], member_path: &str, fond: DilaFond) {
        let orig = parse_dila_xml(xml, member_path, fond).expect("parse direct");
        let source_fields = build_source_fields_dila(xml, fond);
        let rebuilt = Decision::from_source_fields_dila(
            &orig.texte_integral_clean,
            &source_fields,
            &orig.source_uid,
        );
        // Aligne les deux champs exclus avant l'égalité globale ; tout le reste
        // doit être identique au champ près.
        let mut orig_cmp = orig.clone();
        orig_cmp.member_name = rebuilt.member_name.clone();
        orig_cmp.texte_integral_raw = rebuilt.texte_integral_raw.clone();
        assert_eq!(
            orig_cmp, rebuilt,
            "round-trip DILA non identique au parse direct ({})",
            orig.source_uid
        );
        // Chunker : les 3 entrées explicitement byte-à-byte (redondant mais
        // diagnostique si l'égalité globale casse).
        assert_eq!(orig.texte_integral_clean, rebuilt.texte_integral_clean);
        assert_eq!(orig.metadata_header, rebuilt.metadata_header);
        assert_eq!(orig.visa_trim, rebuilt.visa_trim);
        assert_eq!(orig.sections, rebuilt.sections);
    }

    #[test]
    fn dila_jade_round_trips_via_source_fields() {
        let xml = r#"<TEXTE_JURI_ADMIN>
<META>
  <META_COMMUN>
    <ID>CETATEXT000054148459</ID>
    <ANCIEN_ID>JG_L_2026_05_000000493597</ANCIEN_ID>
    <ORIGINE>CETAT</ORIGINE>
    <NATURE>Texte</NATURE>
  </META_COMMUN>
  <META_SPEC>
    <META_JURI>
      <TITRE>Conseil d'État, 6ème - 5ème chambres réunies, 27/05/2026, 493597</TITRE>
      <DATE_DEC>2026-05-27</DATE_DEC>
      <JURIDICTION>Conseil d'État</JURIDICTION>
      <NUMERO>493597</NUMERO>
      <SOLUTION/>
    </META_JURI>
    <META_JURI_ADMIN>
      <FORMATION>6ème - 5ème chambres réunies</FORMATION>
      <TYPE_REC>excès de pouvoir</TYPE_REC>
      <AVOCATS>SCP PIWNICA &amp; MOLINIE</AVOCATS>
      <ECLI>ECLI:FR:CECHR:2026:493597.20260527</ECLI>
    </META_JURI_ADMIN>
  </META_SPEC>
</META>
<TEXTE>
  <BLOC_TEXTUEL>
    <CONTENU>Vu : la requête.&lt;br/&gt;&lt;br/&gt;Considérant ce qui suit : motifs.&lt;br/&gt;&lt;br/&gt;DECIDE :&lt;br/&gt;&lt;br/&gt;Article 1.</CONTENU>
  </BLOC_TEXTUEL>
  <SOMMAIRE>
    <SCT>29-03-02 ENERGIE.</SCT>
    <ANA>Une analyse juridique.</ANA>
  </SOMMAIRE>
</TEXTE>
</TEXTE_JURI_ADMIN>"#
            .as_bytes();
        // `member_path` = préfixe pivot prod : `from_source_fields_dila` déduit le
        // fond du `source_uid` (`dila-jade/<ID>`).
        assert_dila_round_trip(xml, "dila-jade", DilaFond::Jade);
    }

    #[test]
    fn dila_constit_round_trips_via_source_fields() {
        let xml = r#"<TEXTE_JURI_CONSTIT>
<META>
  <META_COMMUN>
    <ID>CONSTEXT000054148611</ID>
    <ORIGINE>CONSTIT</ORIGINE>
    <NATURE>L</NATURE>
    <ECLI>ECLI:FR:CC:2026:2026.321.322.323.L</ECLI>
  </META_COMMUN>
  <META_SPEC>
    <META_JURI>
      <TITRE>Nature juridique de dispositions</TITRE>
      <DATE_DEC>2026-05-22</DATE_DEC>
      <JURIDICTION>Conseil constitutionnel</JURIDICTION>
      <NUMERO>2026-321/322/323</NUMERO>
      <SOLUTION>Réglementaire</SOLUTION>
    </META_JURI>
    <META_JURI_CONSTIT>
      <NATURE_QUALIFIEE>L</NATURE_QUALIFIEE>
      <NOR>CSCL2613769S</NOR>
      <TITRE_JO>JORF n°0120 du 23 mai 2026, texte n° 79</TITRE_JO>
    </META_JURI_CONSTIT>
  </META_SPEC>
</META>
<TEXTE>
  <BLOC_TEXTUEL>
    <CONTENU>LE CONSEIL CONSTITUTIONNEL. Considérant ce qui suit : motifs. DECIDE : Article 1.</CONTENU>
  </BLOC_TEXTUEL>
</TEXTE>
</TEXTE_JURI_CONSTIT>"#
            .as_bytes();
        assert_dila_round_trip(xml, "dila-constit", DilaFond::Constit);
    }

    // Fixture CNIL réelle (ADR 0185, audit cnil.md) : schéma `TEXTE_CNIL` SANS
    // `META_JURI` — date en `META_CNIL/DATE_TEXTE`, numéro en `META_CNIL/NUMERO`,
    // pas d'ECLI. `NATURE_DELIB` = facette régulateur versée en source_fields.
    const CNIL_XML: &[u8] = r#"<TEXTE_CNIL>
<META>
  <META_COMMUN>
    <ID>CNILTEXT000054398352</ID>
    <ORIGINE>CNIL</ORIGINE>
    <NATURE>DECISION</NATURE>
  </META_COMMUN>
  <META_SPEC>
    <META_CNIL>
      <TITRE>DECISION n°DR-2026-036 du 25 février 2026</TITRE>
      <TITREFULL>Décision DR-2026-036 du 25 février 2026 autorisant un traitement</TITREFULL>
      <NUMERO>DR-2026-036</NUMERO>
      <NOR/>
      <NATURE_DELIB>Autorisation de recherche</NATURE_DELIB>
      <DATE_TEXTE>2026-02-25</DATE_TEXTE>
      <DATE_PUBLI>2026-07-09</DATE_PUBLI>
      <ETAT_JURIDIQUE>VIGUEUR</ETAT_JURIDIQUE>
    </META_CNIL>
  </META_SPEC>
</META>
<BLOC_TEXTUEL>
  <CONTENU>La Commission nationale de l'informatique et des libertés, Vu le règlement (UE) 2016/679 ; Considérant que : motifs. DECIDE : Article 1.</CONTENU>
</BLOC_TEXTUEL>
</TEXTE_CNIL>"#
        .as_bytes();

    #[test]
    fn dila_cnil_maps_fields_from_meta_cnil() {
        let d = parse_dila_xml(CNIL_XML, "dila-cnil", DilaFond::Cnil).expect("parse CNIL");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CNIL"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Commission nationale de l'informatique et des libertés")
        );
        // Date lue dans META_CNIL/DATE_TEXTE (pas de META_JURI/DATE_DEC).
        assert_eq!(d.date_lecture.as_deref(), Some("2026-02-25"));
        // Numéro lu dans META_CNIL/NUMERO ; format régulateur (pas composite).
        assert_eq!(d.numero_dossier.as_deref(), Some("DR-2026-036"));
        // Pas d'ECLI côté CNIL.
        assert_eq!(d.ecli, None);
        // NATURE_DELIB (facette) versée verbatim en source_fields.
        let sf = build_source_fields_dila(CNIL_XML, DilaFond::Cnil);
        let mc = sf.get("META_CNIL").and_then(Value::as_object).unwrap();
        assert_eq!(
            mc.get("NATURE_DELIB").and_then(Value::as_str),
            Some("Autorisation de recherche")
        );
    }

    #[test]
    fn dila_cnil_round_trips_via_source_fields() {
        assert_dila_round_trip(CNIL_XML, "dila-cnil", DilaFond::Cnil);
    }
}
