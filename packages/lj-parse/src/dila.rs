use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// DILA bulk (JADE / CONSTIT / CNIL) — schémas `<TEXTE_JURI_{ADMIN,CONSTIT}>` et
// `<TEXTE_CNIL>`. Octets déjà réparés au bord lj-sources (de-escape
// `&amp;nbsp;`, mojibake par sous-arbre) ; le parser reste PUR.
// ─────────────────────────────────────────────────────────────────────────────

/// The CNIL deliberations that lay down a general rule or position: the norms
/// hold them, as the CNIL's doctrine.
pub const CNIL_DOCTRINE_NATURES: &[&str] = &[
    "Référentiel/Règlement type/Norme",
    "Recommandation/Lignes directrices",
];

fn is_cnil_doctrine(meta_cnil: &XmlNode) -> bool {
    node_text(meta_cnil.find_first(&["NATURE_DELIB"]))
        .is_some_and(|n| CNIL_DOCTRINE_NATURES.contains(&n.trim()))
}

/// A CNIL deliberation of [`CNIL_DOCTRINE_NATURES`], as the norms take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CnilDoctrine {
    /// `CNILTEXT…`, its DILA identifier.
    pub id: String,
    pub title: String,
    pub number: Option<String>,
    pub date_texte: String,
    pub date_publi: Option<String>,
    pub nor: Option<String>,
    /// `ETAT_JURIDIQUE`: `VIGUEUR`, `MODIFIE`, `ABROGE`, `ANNULE`, `PERIME`.
    pub status: String,
    pub body: String,
}

/// The deliberation a `TEXTE_CNIL` member holds when its nature is one of
/// [`CNIL_DOCTRINE_NATURES`] and it has a body.
pub fn cnil_doctrine(raw: &[u8]) -> Option<CnilDoctrine> {
    let root = build_tree(raw)?;
    let meta = root.find("META/META_SPEC/META_CNIL")?;
    if !is_cnil_doctrine(meta) {
        return None;
    }
    let field = |tag: &str| node_text(meta.find_first(&[tag])).filter(|v| !v.trim().is_empty());
    let body = root
        .find_first(&["BLOC_TEXTUEL/CONTENU", "TEXTE/BLOC_TEXTUEL/CONTENU"])
        .and_then(XmlNode::html_text)
        .map(|t| clean_texte(&t))
        .filter(|t| !t.trim().is_empty())?;
    Some(CnilDoctrine {
        id: node_text(root.find("META/META_COMMUN")?.find_first(&["ID"]))?,
        title: field("TITREFULL").or_else(|| field("TITRE"))?,
        number: field("NUMERO"),
        date_texte: field("DATE_TEXTE")?,
        date_publi: field("DATE_PUBLI"),
        nor: field("NOR"),
        status: field("ETAT_JURIDIQUE").unwrap_or_else(|| "VIGUEUR".to_string()),
        body,
    })
}

/// Fonds bulk DILA. Sélectionne le sous-bloc `META_JURI_*` et le routage de
/// juridiction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DilaFond {
    /// Jurisprudence administrative (CE / CAA / TC), sous-bloc `META_JURI_ADMIN`.
    Jade,
    /// Conseil constitutionnel, sous-bloc `META_JURI_CONSTIT`.
    Constit,
    /// Délibérations/décisions de la CNIL, sous-bloc `META_CNIL`. Schéma
    /// `TEXTE_CNIL` sans `META_JURI` : date en `META_CNIL/DATE_TEXTE`, numéro en
    /// `META_CNIL/NUMERO`, pas d'ECLI. `jurisdiction_type = CNIL`.
    Cnil,
    /// Cour de cassation judgments published in the Bulletin, sub-block `META_JURI_JUDI`.
    Cass,
    /// Unpublished Cour de cassation judgments, sub-block `META_JURI_JUDI`.
    Inca,
    /// A selection of appeal and first-degree judicial decisions, sub-block `META_JURI_JUDI`.
    Capp,
}

impl DilaFond {
    /// Tag du sous-bloc juridiction-spécifique (`META/META_SPEC/<…>`).
    fn meta_juri_sub_tag(self) -> &'static str {
        match self {
            DilaFond::Jade => "META_JURI_ADMIN",
            DilaFond::Constit => "META_JURI_CONSTIT",
            DilaFond::Cnil => "META_CNIL",
            DilaFond::Cass | DilaFond::Inca | DilaFond::Capp => "META_JURI_JUDI",
        }
    }

    fn is_judicial(self) -> bool {
        matches!(self, DilaFond::Cass | DilaFond::Inca | DilaFond::Capp)
    }
}

/// Libellé de l'émetteur CNIL : le schéma `META_CNIL` ne porte pas de
/// `JURIDICTION`, on le pose ici pour nourrir le `metadata_header`.
const CNIL_JURIDICTION_NOM: &str = "Commission nationale de l'informatique et des libertés";

/// `jurisdiction_type` JADE depuis le préfixe `ANCIEN_ID` (`JG`→CE, `J0..J7`→CAA,
/// `JC`→TC), fallback sur le libellé `JURIDICTION`.
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
/// 2026-322, 2026-323]`).
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

/// `source_fields` of a DILA record: the whole tree outside the text, nested,
/// repeated and attributed elements included ([`xml_tree_json`]). The blocks
/// of `META` and `META_SPEC` sit at the top (`META_COMMUN`, `META_JURI`, the
/// fund's sub-block), beside every block of `TEXTE` but `BLOC_TEXTUEL`, whose
/// text goes to `full_text`, and `LIENS`.
pub fn build_source_fields_dila(raw: &[u8]) -> Value {
    let root = build_tree(raw).unwrap_or_default();
    let mut obj = serde_json::Map::new();
    let mut put = |node: &XmlNode| {
        if let Some(v) = xml_tree_json(node) {
            push_json_child(&mut obj, &node.tag, v);
        }
    };
    for child in &root.children {
        match child.tag.as_str() {
            "META" => {
                for m in &child.children {
                    if m.tag == "META_SPEC" {
                        m.children.iter().for_each(&mut put);
                    } else {
                        put(m);
                    }
                }
            }
            "TEXTE" => child
                .children
                .iter()
                .filter(|c| c.tag != "BLOC_TEXTUEL")
                .for_each(&mut put),
            "BLOC_TEXTUEL" => {}
            _ => put(child),
        }
    }
    Value::Object(obj)
}

/// `true` si `source_fields` provient d'un payload DILA bulk : présence des
/// nœuds `META_COMMUN` + `META_JURI` (calqués par [`build_source_fields_dila`],
/// qu'aucun payload XML opendata `<Dossier>` ni JSON Judilibre ne porte).
/// Discriminant de famille du dispatch [`Decision::from_source_fields`].
pub(crate) fn source_fields_is_dila(source_fields: &Value) -> bool {
    source_fields.get("META_COMMUN").is_some()
        && (source_fields.get("META_JURI").is_some() || source_fields.get("META_CNIL").is_some())
}

/// Fond DILA déduit du `source_uid` (`dila-jade/<ID>` / `dila-constit/<ID>`).
/// C'est le discriminant le plus sûr — il colle au `source_prefix` posé à
/// l'ingest, donc stable en DB. Repli sur la forme du `source_fields` (sous-bloc
/// `META_JURI_CONSTIT` ⇒ Constit) si le préfixe est absent, puis défaut Jade.
fn dila_fond_from_uid(source_uid: &str, source_fields: &Value) -> DilaFond {
    for (prefix, fond) in [
        ("dila-cass/", DilaFond::Cass),
        ("dila-inca/", DilaFond::Inca),
        ("dila-capp/", DilaFond::Capp),
    ] {
        if source_uid.starts_with(prefix) {
            return fond;
        }
    }
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

fn dila_sections(text: &str, fond: DilaFond) -> Vec<DecisionSection> {
    match fond {
        DilaFond::Cnil => extract_sections_cnil(text),
        _ => extract_sections_xml(text),
    }
}

/// DILA bulk branch of [`Decision::from_source_fields`]: rebuilds a `Decision`
/// **identical** to [`parse_dila_xml`] from `(full_text, source_fields)`,
/// without touching the raw payload. Exact counterpart of
/// [`build_source_fields_dila`]: it reads the same
/// `META_COMMUN`/`META_JURI`/sub-block scalars and recomputes
/// sections and `metadata_header` on `full_text`
/// (= `texte_integral_clean`). `raw == clean == full_text`; `date_mise_jour` is
/// not carried (hence `None`). The fond (Jade/Constit) follows from the
/// `source_uid` ([`dila_fond_from_uid`]).
pub(crate) fn from_source_fields_dila(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let fond = dila_fond_from_uid(source_uid, source_fields);
    if fond.is_judicial() {
        return dila_judi::judicial_decision(full_text, source_fields, source_uid, fond);
    }
    let sub_tag = fond.meta_juri_sub_tag();

    // First occurrence of a tag of a block, like `find_first` on the tree.
    let get = |block: &str, key: &str| -> Option<String> {
        source_fields
            .get(block)
            .and_then(|node| node.get(key))
            .and_then(json_text)
            .map(str::to_string)
    };

    // CNIL: date and number in `META_CNIL` (no `META_JURI`).
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
        DilaFond::Cass | DilaFond::Inca | DilaFond::Capp => unreachable!("judicial fund"),
    };

    // ECLI: META_COMMUN/ECLI (CONSTIT) or sub-block/ECLI (JADE CE), in the
    // same fallback order as `parse_dila_xml`.
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
    // PUBLI_RECUEIL (JADE): Lebon A/B/C ranking, the publication facet.
    let publication_codes = get(sub_tag, "PUBLI_RECUEIL")
        .filter(|s| !s.is_empty())
        .map(|c| vec![c])
        .unwrap_or_default();

    let sections = dila_sections(full_text, fond);
    let metadata_header = assemble_metadata_header_xml(
        jurisdiction_name.clone(),
        Some(date_dec.clone()),
        None,
        type_recours.clone(),
        solution.clone(),
        formation.clone(),
    );

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
        listed_counsel: Vec::new(),
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections,
        metadata_header,
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    }
}

/// Issue de [`parse_dila_doc`] : un membre DILA porte soit son **texte intégral**
/// (`BLOC_TEXTUEL/CONTENU`), soit une **analyse seule** (`SOMMAIRE/ANA` + `SCT`,
/// mention aux tables) sans texte — vieux fonds JADE, CE pré-2002. Les deux
/// donnent une [`Decision`] complète côté métadonnées ; elles diffèrent par
/// l'origine du `full_text` et par leur traitement à l'ingest : `Full` = upsert
/// normal (texte canonique) ; `Analysis` = soit enrichissement `source_fields`
/// d'une décision existante (rattachement), soit décision orpheline dont
/// l'analyse EST le contenu cherchable, jamais d'écrasement d'un texte réel.
pub enum DilaDoc {
    Full(Decision),
    Analysis(Decision),
}

/// Parse un XML bulk DILA (`fond`) en [`Decision`] **à texte intégral**. Pendant
/// strict de [`parse_dila_doc`] restreint au cas `Full` : erreur franche si
/// `CONTENU` est absent (un membre analyse-seule n'est PAS un texte). Porte
/// d'entrée des consommateurs qui exigent un texte réel, pour qu'ils ne voient
/// jamais une `Decision` au `full_text` synthétisé depuis l'analyse.
pub fn parse_dila_xml(
    raw: &[u8],
    member_path: &str,
    fond: DilaFond,
) -> lj_core::error::Result<Decision> {
    let mut docs = parse_dila_doc(raw, member_path, fond)?;
    match (docs.pop(), docs.is_empty()) {
        (Some(DilaDoc::Full(d)), true) => Ok(d),
        _ => Err(lj_core::error::CoreError::Xml(format!(
            "DILA {member_path}: not one record with BLOC_TEXTUEL/CONTENU"
        ))),
    }
}

/// Parse un XML bulk DILA (`fond`) en [`DilaDoc`]. Les octets sont supposés déjà
/// réparés au bord (lj-sources). Lit le schéma commun `META/META_COMMUN`,
/// `META/META_SPEC/META_JURI` et le sous-bloc `META_JURI_{ADMIN,CONSTIT}`. Si
/// `BLOC_TEXTUEL/CONTENU` est présent et non vide → [`DilaDoc::Full`],
/// `full_text = clean_texte(CONTENU)`. Sinon, si `SOMMAIRE` (`SCT`/`ANA`) porte
/// du texte → [`DilaDoc::Analysis`], `full_text` = `SCT` puis `ANA` joints
/// (mention aux tables + analyse), nettoyés et cherchables. Sections re-détectées
/// sur le `full_text` retenu ; `member_path` préfixe la provenance.
///
/// Erreur franche [`CoreError::Xml`] si `ID` / `DATE_DEC` absents (XML malformé,
/// frontière de validation source unique). Vide si **ni** `CONTENU` **ni**
/// `SOMMAIRE` ne portent de texte : membre sans corps, rien à ingérer — cas
/// **nominal** du fond CNIL, dont une large part des entrées sont des fiches de
/// registre (autorisations de transfert/recherche) publiées sans texte.
/// L'appelant le compte en skip, pas en erreur. The register entries of 2010 and
/// 2011 hold the President's salutation « Le Président, » alone, and skip too. A judicial record cut in
/// sections gives one document per section.
pub fn parse_dila_doc(
    raw: &[u8],
    member_path: &str,
    fond: DilaFond,
) -> lj_core::error::Result<Vec<DilaDoc>> {
    use lj_core::error::CoreError;

    if fond.is_judicial() {
        return dila_judi::parse_judicial(raw, member_path, fond);
    }

    let root = build_tree(raw).unwrap_or_default();

    let meta_commun = root.find("META/META_COMMUN");
    let meta_juri = root.find("META/META_SPEC/META_JURI");
    let sub_tag = fond.meta_juri_sub_tag();
    let meta_sub = root.find(&format!("META/META_SPEC/{sub_tag}"));

    let id = meta_commun
        .and_then(|m| node_text(m.find_first(&["ID"])))
        .ok_or_else(|| CoreError::Xml(format!("DILA {member_path}: META_COMMUN/ID absent")))?;
    if fond == DilaFond::Cnil && meta_sub.is_some_and(is_cnil_doctrine) {
        return Ok(Vec::new());
    }
    // CNIL : date dans `META_CNIL/DATE_TEXTE` (pas de `META_JURI`).
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
    // fonds analyse-seule. `is_analysis` discrimine le `DilaDoc` retourné.
    // JADE/CONSTIT enveloppent le corps dans `<TEXTE>` ; CNIL non — `BLOC_TEXTUEL`
    // pend directement sous `<TEXTE_CNIL>`. On essaie les deux chemins.
    let contenu = root
        .find_first(&["TEXTE/BLOC_TEXTUEL/CONTENU", "BLOC_TEXTUEL/CONTENU"])
        .and_then(XmlNode::html_text)
        .filter(|s| !matches!(s.trim(), "" | "Le Président,"));
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
                // Membre sans corps (ni CONTENU ni SOMMAIRE) : nominal pour CNIL (fiches de
                // registre sans texte) → skip, pas erreur.
                return Ok(Vec::new());
            }
            (analysis, true)
        }
    };

    let texte_integral_clean = clean_texte(&texte_integral_raw);
    let sections = dila_sections(&texte_integral_clean, fond);

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
        DilaFond::Cass | DilaFond::Inca | DilaFond::Capp => unreachable!("judicial fund"),
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
        listed_counsel: Vec::new(),
        texte_integral_raw,
        texte_integral_clean,
        sections,
        metadata_header,
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    };
    Ok(vec![if is_analysis {
        DilaDoc::Analysis(decision)
    } else {
        DilaDoc::Full(decision)
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── DILA bulk ────────────────────────────────────────────────────────────

    #[test]
    fn dila_numero_composite_propagates_year_prefix() {
        assert_eq!(
            parse_numero_composite("2026-321/322/323"),
            vec!["2026-321", "2026-322", "2026-323"]
        );
        assert_eq!(parse_numero_composite("493597"), vec!["493597"]);
        assert_eq!(parse_numero_composite("96-381"), vec!["96-381"]);
    }

    #[test]
    fn dila_jade_jurisdiction_type_from_ancien_id() {
        // Préfixe ANCIEN_ID : JG→CE, J0..J7→CAA, JC→TC.
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
        // Fixture JADE : ECLI en META_JURI_ADMIN, ANCIEN_ID JG (CE), SOLUTION vide,
        // FORMATION/AVOCATS dans le sous-bloc.
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
        assert_eq!(d.solution, None);
        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"visa"));
        assert!(kinds.contains(&"motivations"));
        assert!(kinds.contains(&"dispositif"));
        let sf = build_source_fields_dila(xml);
        assert_eq!(
            sf.pointer("/SOMMAIRE/SCT").and_then(Value::as_str),
            Some("29-03-02 ENERGIE.")
        );
        assert_eq!(
            sf.pointer("/SOMMAIRE/ANA").and_then(Value::as_str),
            Some("Une analyse juridique.")
        );
        assert!(sf.get("META_JURI_ADMIN").is_some());
        assert!(sf.get("META_COMMUN").is_some());
    }

    #[test]
    fn dila_constit_maps_ecli_numero_composite_and_type() {
        // Fixture CONSTIT : ECLI META_COMMUN, NUMERO composite, SOLUTION normalisée,
        // sous-bloc META_JURI_CONSTIT.
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
        assert_eq!(d.formation, None);
        let sf = build_source_fields_dila(xml);
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
        let no_id = r#"<TEXTE_JURI_CONSTIT><META><META_COMMUN/><META_SPEC><META_JURI><DATE_DEC>2026-01-01</DATE_DEC></META_JURI></META_SPEC></META><TEXTE><BLOC_TEXTUEL><CONTENU>x</CONTENU></BLOC_TEXTUEL></TEXTE></TEXTE_JURI_CONSTIT>"#.as_bytes();
        assert!(parse_dila_doc(no_id, "m", DilaFond::Constit).is_err());

        // A member without a body (no CONTENU, no SOMMAIRE) but with its ID and date
        // gives nothing, the CNIL register entries' nominal skip; the strict
        // `parse_dila_xml`, which wants a text, still refuses it.
        let empty = r#"<TEXTE_JURI_CONSTIT><META><META_COMMUN><ID>CONSTEXT1</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>2026-01-01</DATE_DEC></META_JURI></META_SPEC></META><TEXTE><BLOC_TEXTUEL/></TEXTE></TEXTE_JURI_CONSTIT>"#.as_bytes();
        assert!(parse_dila_doc(empty, "m", DilaFond::Constit)
            .unwrap()
            .is_empty());
        assert!(parse_dila_xml(empty, "m", DilaFond::Constit).is_err());

        let salutation = "<TEXTE_CNIL><META><META_COMMUN><ID>CNILTEXT000022502192</ID></META_COMMUN><META_SPEC><META_CNIL><DATE_TEXTE>2010-12-31</DATE_TEXTE></META_CNIL></META_SPEC></META><BLOC_TEXTUEL><CONTENU>Le Président,<br/></CONTENU></BLOC_TEXTUEL></TEXTE_CNIL>".as_bytes();
        assert!(parse_dila_doc(salutation, "m", DilaFond::Cnil)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dila_doc_full_when_contenu_present() {
        let xml = r#"<TEXTE_JURI_ADMIN><META><META_COMMUN><ID>CETATEXT1</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>2026-05-27</DATE_DEC><JURIDICTION>Conseil d'État</JURIDICTION><NUMERO>493597</NUMERO></META_JURI><META_JURI_ADMIN/></META_SPEC></META><TEXTE><BLOC_TEXTUEL><CONTENU>Considérant ce qui suit : motifs.</CONTENU></BLOC_TEXTUEL><SOMMAIRE><SCT>29-03-02 ENERGIE.</SCT><ANA>Une analyse.</ANA></SOMMAIRE></TEXTE></TEXTE_JURI_ADMIN>"#.as_bytes();
        match parse_dila_doc(xml, "dila-jade", DilaFond::Jade)
            .expect("parse")
            .pop()
            .expect("doc présent (CONTENU non vide)")
        {
            DilaDoc::Full(d) => {
                assert!(d.texte_integral_clean.contains("Considérant ce qui suit"));
                assert!(!d.texte_integral_clean.contains("Une analyse"));
            }
            DilaDoc::Analysis(_) => panic!("attendu Full (CONTENU présent)"),
        }
    }

    #[test]
    fn dila_doc_analysis_when_contenu_absent() {
        // Vieux fond JADE (CE pré-2002) sans BLOC_TEXTUEL — SOMMAIRE seul →
        // DilaDoc::Analysis, full_text = SCT + ANA joints (cherchable), métadonnées
        // pleines (juridiction CE par repli sur le libellé, date, numéro).
        let xml = r#"<TEXTE_JURI_ADMIN><META><META_COMMUN><ID>CETATEXT0000001</ID></META_COMMUN><META_SPEC><META_JURI><DATE_DEC>1995-03-10</DATE_DEC><JURIDICTION>Conseil d'État</JURIDICTION><NUMERO>123456</NUMERO></META_JURI><META_JURI_ADMIN/></META_SPEC></META><TEXTE><SOMMAIRE><SCT>26-01 PROCEDURE.</SCT><ANA>Le Conseil d'État juge que la requête est recevable.</ANA></SOMMAIRE></TEXTE></TEXTE_JURI_ADMIN>"#.as_bytes();
        match parse_dila_doc(xml, "dila-jade", DilaFond::Jade)
            .expect("parse")
            .pop()
            .expect("doc présent (SOMMAIRE)")
        {
            DilaDoc::Analysis(d) => {
                assert_eq!(d.source_uid, "dila-jade/CETATEXT0000001");
                assert_eq!(d.jurisdiction_type.as_deref(), Some("CE"));
                assert_eq!(d.date_lecture.as_deref(), Some("1995-03-10"));
                assert_eq!(d.numero_dossier.as_deref(), Some("123456"));
                assert!(d.texte_integral_clean.contains("26-01 PROCEDURE."));
                assert!(d
                    .texte_integral_clean
                    .contains("Le Conseil d'État juge que la requête est recevable."));
            }
            DilaDoc::Full(_) => panic!("attendu Analysis (CONTENU absent)"),
        }
        assert!(parse_dila_xml(xml, "dila-jade", DilaFond::Jade).is_err());
    }

    // ── Round-trip `from_source_fields` ⟷ `parse_dila_xml` ────────────────────
    //
    // A `Decision` rebuilt from `(full_text, source_fields)` equals the direct parse
    // but for `member_name` and `texte_integral_raw`, which the columns do not carry.

    /// Rebuilt from its source fields, the `Decision` equals the direct parse.
    fn assert_dila_round_trip(xml: &[u8], member_path: &str, fond: DilaFond) {
        let orig = parse_dila_xml(xml, member_path, fond).expect("parse direct");
        let source_fields = build_source_fields_dila(xml);
        let rebuilt =
            from_source_fields_dila(&orig.texte_integral_clean, &source_fields, &orig.source_uid);
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
        assert_eq!(orig.texte_integral_clean, rebuilt.texte_integral_clean);
        assert_eq!(orig.metadata_header, rebuilt.metadata_header);
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
        // `from_source_fields_dila` reads the fond from the `dila-jade/<ID>` uid.
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

    // Fixture CNIL réelle : schéma `TEXTE_CNIL` SANS `META_JURI` — date en
    // `META_CNIL/DATE_TEXTE`, numéro en `META_CNIL/NUMERO`, pas d'ECLI.
    // `NATURE_DELIB` = facette régulateur versée en source_fields.
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
        assert_eq!(d.date_lecture.as_deref(), Some("2026-02-25"));
        assert_eq!(d.numero_dossier.as_deref(), Some("DR-2026-036"));
        assert_eq!(d.ecli, None);
        let sf = build_source_fields_dila(CNIL_XML);
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
