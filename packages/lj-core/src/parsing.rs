//! Parsers tolérants vers le modèle `Decision`
//! (port de `parsing/decision_input.py` XML + `parsing/judilibre_parser.py`).
//!
//! XML via `quick-xml` (tolérant, mode recover). JSON Judilibre via `serde_json`.
//! Aucun I/O : les bytes/valeurs sont fournis par l'appelant (`lj-sources`).

use crate::decision::{Decision, DecisionSection, VISA_TRIM_MAX_CHARS};
use crate::normalizer::clean_texte;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::Value;
use std::sync::LazyLock;

use regex::Regex;

mod adde;
mod ariane;
mod cnda;
mod dila;
mod european;
pub use adde::{build_adde_source_fields, parse_adde_title, AddeCitation};
pub use ariane::{
    analyse_body, build_ariane_source_fields, parse_ajce_html, parse_dce_html, AjceAnalysis,
    AjceEntry, AjceRubrique, DceParsed,
};
use cnda::source_uid_is_cnda;
pub use cnda::{clean_ocr_markdown, parse_cnda, reflow_cnda_pdf_text, CndaParsed};
use dila::source_fields_is_dila;
pub use dila::{build_source_fields_dila, parse_dila_doc, parse_dila_xml, DilaDoc, DilaFond};
use european::source_uid_is_html_europe;
pub use european::{parse_cedh, parse_cjue};

// ─────────────────────────────────────────────────────────────────────────────
// XML opendata (TA / CAA / CE) — port fidèle de decision_input.py
// ─────────────────────────────────────────────────────────────────────────────

// Réparation byte-level : `<Texte_Integral></p>` orphelin → `<Texte_Integral>`.
const BROKEN_TEXTE_INTEGRAL_OPEN: &[u8] = b"<Texte_Integral></p>";
const FIXED_TEXTE_INTEGRAL_OPEN: &[u8] = b"<Texte_Integral>";

/// Préfixes uid : D<X> (décisions), OR<X> (ordonnances). Port de `_TYPE_BY_PREFIX`.
const TYPE_BY_PREFIX: &[(&str, &str)] = &[
    ("DTA", "TA"),
    ("ORTA", "TA"),
    ("DCA", "CAA"),
    ("ORCA", "CAA"),
    ("DCE", "CE"),
    ("ORCE", "CE"),
];

// Patterns `_extract_sections` (ordre préservé : kind, regex). IGNORECASE
// systématique (`re.IGNORECASE` côté Python). Le dispositif est aussi MULTILINE.
static SECTION_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "procedure",
            Regex::new(
                r"(?i)\b(?:Vu la procédure suivante|Procédure contentieuse antérieure|Procédure devant [^:\n]{1,80})\s*:",
            )
            .unwrap(),
        ),
        ("visa", Regex::new(r"(?i)\bVu\s*:\s*").unwrap()),
        ("visa", Regex::new(r"(?i)\bVu les autres pièces du dossier\b\s*;?").unwrap()),
        ("motivations", Regex::new(r"(?i)\bConsidérant ce qui suit\b\s*:?\s*").unwrap()),
        (
            "dispositif",
            Regex::new(
                r"(?mi)^(?:D\s*[ÉE]\s*C\s*I\s*D\s*E|O\s*R\s*D\s*O\s*N\s*N\s*E|A\s*R\s*R\s*[ÊE]\s*T\s*E)\s*:?\s*$",
            )
            .unwrap(),
        ),
    ]
});

/// Nom de fichier (dernier segment après `/`) — `Path(...).name`.
fn path_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Port de `_classify_uid` : type de juridiction depuis le préfixe du nom uid.
fn classify_uid(source_uid: &str) -> Option<String> {
    let uid_name = path_name(source_uid).to_uppercase();
    TYPE_BY_PREFIX
        .iter()
        .find(|(prefix, _)| uid_name.starts_with(&format!("{prefix}_")))
        .map(|(_, jur_type)| (*jur_type).to_string())
}

/// Arbre XML minimal : balise locale + enfants + texte descendant agrégé
/// (`itertext()`). Reconstruit le sous-ensemble de lxml utilisé par le parser.
#[derive(Debug, Default)]
pub struct XmlNode {
    pub tag: String,
    pub children: Vec<XmlNode>,
    /// Tous les bouts de texte de ce nœud ET de ses descendants, dans l'ordre.
    text_parts: Vec<String>,
    /// Attributs directs du nœud : `(nom local, valeur)`, ordre document.
    attrs: Vec<(String, String)>,
}

impl XmlNode {
    /// `etree.find(path)` : suit un chemin `A/B/C` d'enfants directs, premier match.
    pub fn find(&self, path: &str) -> Option<&XmlNode> {
        let mut current = self;
        for segment in path.split('/') {
            current = current.children.iter().find(|c| c.tag == segment)?;
        }
        Some(current)
    }

    /// `_first` : premier des chemins candidats qui résout.
    fn find_first(&self, paths: &[&str]) -> Option<&XmlNode> {
        paths.iter().find_map(|p| self.find(p))
    }

    /// `_text` : `itertext()` concaténé puis `.strip()` ; `None` si vide.
    pub fn text(&self) -> Option<String> {
        let joined: String = self.text_parts.concat();
        let trimmed = joined.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// Valeur d'un attribut direct par nom local (`@cid`), première occurrence.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `_text` appliqué à un `Option<&XmlNode>` (gère le `None` amont).
pub fn node_text(node: Option<&XmlNode>) -> Option<String> {
    node.and_then(XmlNode::text)
}

/// Nom local d'une balise : `ns:Tag` → `Tag`.
fn local_name(qname: &[u8]) -> String {
    let s = String::from_utf8_lossy(qname);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// Construit l'arbre minimal depuis les events quick-xml (tolérant : pas de
/// vérification des noms de fermeture, on dépile aussi à l'EOF). Les `text_parts`
/// remontent vers tous les ancêtres pour reproduire `itertext()`.
/// Lit les attributs d'un élément en `(nom local, valeur unescapée)`. Les valeurs
/// d'attributs LEGI utiles (`@cid` = LEGITEXT) sont ASCII ; `from_utf8_lossy`
/// suffit, comme pour les fragments de texte ailleurs dans `build_tree`.
fn read_attrs(e: &quick_xml::events::BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .filter_map(std::result::Result::ok)
        .map(|a| {
            let key = local_name(a.key.as_ref());
            let raw = String::from_utf8_lossy(&a.value);
            let val = quick_xml::escape::unescape(&raw)
                .map(|u| u.into_owned())
                .unwrap_or_else(|_| raw.into_owned());
            (key, val)
        })
        .collect()
}

pub fn build_tree(raw: &[u8]) -> Option<XmlNode> {
    let mut reader = Reader::from_reader(raw);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root: Option<XmlNode> = None;
    let mut buf = Vec::new();

    let attach = |node: XmlNode, stack: &mut Vec<XmlNode>, root: &mut Option<XmlNode>| match stack
        .last_mut()
    {
        Some(parent) => parent.children.push(node),
        None => *root = Some(node),
    };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => stack.push(XmlNode {
                tag: local_name(e.name().as_ref()),
                attrs: read_attrs(&e),
                ..Default::default()
            }),
            Ok(Event::Empty(e)) => {
                let node = XmlNode {
                    tag: local_name(e.name().as_ref()),
                    attrs: read_attrs(&e),
                    ..Default::default()
                };
                attach(node, &mut stack, &mut root);
            }
            Ok(Event::Text(e)) => {
                // quick-xml 0.39 a retiré BytesText::unescape : decode (encoding-aware)
                // puis unescape des entites XML (&amp; ...), fidele a l'ancien comportement.
                let txt = match e.decode() {
                    Ok(d) => quick_xml::escape::unescape(&d)
                        .map(|u| u.into_owned())
                        .unwrap_or_else(|_| d.into_owned()),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                if !txt.is_empty() {
                    for node in stack.iter_mut() {
                        node.text_parts.push(txt.clone());
                    }
                }
            }
            // quick-xml 0.40 emet les references d'entites (`&amp;`, `&#160;`,
            // `&#39;` ...) presentes dans le contenu textuel comme des events
            // `GeneralRef` distincts, hors des `Text` voisins. Les ignorer
            // perdait `&` (cabinets « X & Associes »), apostrophes et NBSP —
            // divergence vs le parsing Python qui resout toutes les entites.
            Ok(Event::GeneralRef(e)) => {
                let txt = match e.decode() {
                    Ok(content) => quick_xml::escape::unescape(&format!("&{content};"))
                        .map(|u| u.into_owned())
                        .unwrap_or_else(|_| format!("&{content};")),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                if !txt.is_empty() {
                    for node in stack.iter_mut() {
                        node.text_parts.push(txt.clone());
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    attach(node, &mut stack, &mut root);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }

    // XML tronqué : dépile les balises restées ouvertes vers la racine.
    while let Some(node) = stack.pop() {
        attach(node, &mut stack, &mut root);
    }
    root
}

/// Port de `_build_metadata_header` (XML) : 3 lignes, lignes vides omises.
fn build_metadata_header_xml(root: &XmlNode) -> String {
    let dossier = root.find("Dossier");
    let audience = root.find("Audience");

    assemble_metadata_header_xml(
        dossier.and_then(|d| node_text(d.find_first(&["Nom_Juridiction"]))),
        dossier.and_then(|d| node_text(d.find_first(&["Date_Lecture"]))),
        audience.and_then(|a| node_text(a.find_first(&["Date_Audience"]))),
        dossier.and_then(|d| node_text(d.find_first(&["Type_Recours"]))),
        dossier.and_then(|d| node_text(d.find_first(&["Solution"]))),
        audience.and_then(|a| node_text(a.find_first(&["Formation_Jugement"]))),
    )
}

/// Assemble les 3 lignes du header XML depuis les 6 scalaires lus. Partagé par
/// `build_metadata_header_xml` (depuis l'arbre XML) et
/// `build_metadata_header_xml_from_fields` (depuis `source_fields`) : une seule
/// mise en forme, donc reconstruction garantie identique (ADR 0085).
pub(crate) fn assemble_metadata_header_xml(
    nom_jur: Option<String>,
    date_lecture: Option<String>,
    date_audience: Option<String>,
    type_recours: Option<String>,
    solution: Option<String>,
    formation: Option<String>,
) -> String {
    let mut header_lines: Vec<String> = Vec::new();

    let mut line1: Vec<String> = Vec::new();
    if let Some(nom) = nom_jur {
        line1.push(nom);
    }
    if let Some(d) = date_lecture.or(date_audience) {
        line1.push(d);
    }
    if !line1.is_empty() {
        header_lines.push(line1.join(" | "));
    }

    let mut line2: Vec<String> = Vec::new();
    if let Some(tr) = type_recours {
        line2.push(format!("Recours : {tr}"));
    }
    if let Some(s) = solution {
        line2.push(format!("Solution : {s}"));
    }
    if !line2.is_empty() {
        header_lines.push(line2.join(" | "));
    }

    if let Some(f) = formation {
        header_lines.push(format!("Formation : {f}"));
    }

    header_lines.join("\n")
}

/// `metadata_header` XML reconstruit depuis `source_fields` (cf.
/// `build_source_fields_xml`). Lit les mêmes 6 scalaires que
/// `build_metadata_header_xml`, mais dans le JSONB stocké `{ "Dossier": {…},
/// "Audience": {…} }` — la sortie est identique au parse direct (gate ADR 0085).
pub fn build_metadata_header_xml_from_fields(source_fields: &Value) -> String {
    let get = |parent: &str, key: &str| -> Option<String> {
        source_fields
            .get(parent)
            .and_then(|node| node.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    assemble_metadata_header_xml(
        get("Dossier", "Nom_Juridiction"),
        get("Dossier", "Date_Lecture"),
        get("Audience", "Date_Audience"),
        get("Dossier", "Type_Recours"),
        get("Dossier", "Solution"),
        get("Audience", "Formation_Jugement"),
    )
}

/// Port de `_build_visa_trim` (XML) : préfixe `preamble`/`procedure`/`visa`,
/// tronqué paragraph-aware à `VISA_TRIM_MAX_CHARS`.
pub(crate) fn build_visa_trim_xml(sections: &[DecisionSection]) -> String {
    let parts: Vec<&str> = sections
        .iter()
        .filter(|s| matches!(s.kind.as_str(), "preamble" | "procedure" | "visa"))
        .map(|s| s.text.as_str())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    let visa = parts.join("\n\n");
    if char_len(&visa) <= VISA_TRIM_MAX_CHARS {
        return visa;
    }
    let truncated = char_take(&visa, VISA_TRIM_MAX_CHARS);
    match char_rfind_double_newline(&truncated) {
        Some(last_para) if last_para > 0 => char_take(&truncated, last_para),
        _ => truncated,
    }
}

/// Port de `_extract_sections` (XML). Offsets en **codepoints** (comme Python).
pub(crate) fn extract_sections_xml(cleaned_text: &str) -> Vec<DecisionSection> {
    if cleaned_text.is_empty() {
        return Vec::new();
    }

    // (start_char, kind, label).
    let mut markers: Vec<(usize, &'static str, String)> = Vec::new();
    for (kind, re) in SECTION_PATTERNS.iter() {
        if let Some(m) = re.find(cleaned_text) {
            let start_char = byte_to_char_index(cleaned_text, m.start());
            markers.push((start_char, kind, m.as_str().trim().to_string()));
        }
    }
    if markers.is_empty() {
        return Vec::new();
    }

    // Tri stable par position (Python `markers.sort(key=item[0])`).
    markers.sort_by_key(|m| m.0);

    // Dédup : un seul marqueur par kind, et jamais deux au même offset.
    let mut deduped: Vec<(usize, &'static str, String)> = Vec::new();
    let mut seen_kinds: Vec<&'static str> = Vec::new();
    let mut last_start: Option<usize> = None;
    for (start, kind, label) in markers {
        if seen_kinds.contains(&kind) || Some(start) == last_start {
            continue;
        }
        deduped.push((start, kind, label));
        seen_kinds.push(kind);
        last_start = Some(start);
    }

    let total = char_len(cleaned_text);
    let mut sections: Vec<DecisionSection> = Vec::new();

    if deduped[0].0 > 0 {
        let intro_text = char_slice(cleaned_text, 0, deduped[0].0);
        let intro_text = intro_text.trim();
        if !intro_text.is_empty() {
            sections.push(DecisionSection {
                label: "Préambule".to_string(),
                kind: "preamble".to_string(),
                start_char: 0,
                end_char: deduped[0].0,
                text: intro_text.to_string(),
            });
        }
    }

    for (idx, (start, kind, label)) in deduped.iter().enumerate() {
        let end = if idx + 1 < deduped.len() {
            deduped[idx + 1].0
        } else {
            total
        };
        let section_text = char_slice(cleaned_text, *start, end);
        let section_text = section_text.trim();
        if section_text.is_empty() {
            continue;
        }
        sections.push(DecisionSection {
            label: label.clone(),
            kind: (*kind).to_string(),
            start_char: *start,
            end_char: end,
            text: section_text.to_string(),
        });
    }
    sections
}

/// Recherche d'un sous-slice (`bytes.find`).
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Remplace toutes les occurrences d'un sous-slice (`bytes.replace`).
fn replace_subslice(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if i + from.len() <= haystack.len() && &haystack[i..i + from.len()] == from {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Parse un XML opendata (TA/CAA/CE) en `Decision`. Ne lève pas : consigne les
/// anomalies dans `parse_warnings`. `archive_name` préfixe le `source_uid`.
/// Port fidèle de `decision_input.parse`.
pub fn parse_xml(raw: &[u8], member_name: &str, archive_name: Option<&str>) -> Decision {
    let mut warnings: Vec<String> = Vec::new();

    let repaired_buf;
    let raw: &[u8] = if find_subslice(raw, BROKEN_TEXTE_INTEGRAL_OPEN).is_some() {
        warnings.push("xml_repair:texte_integral_orphan_closing_p".to_string());
        repaired_buf = replace_subslice(raw, BROKEN_TEXTE_INTEGRAL_OPEN, FIXED_TEXTE_INTEGRAL_OPEN);
        &repaired_buf
    } else {
        raw
    };

    let root = build_tree(raw).unwrap_or_default();

    let ident = node_text(root.find("Donnees_Techniques/Identification"));
    let source_uid = if let Some(archive) = archive_name {
        format!("{archive}/{member_name}")
    } else {
        let fallback_uid = member_name.strip_suffix(".xml").unwrap_or(member_name);
        if member_name.contains('/') {
            fallback_uid.to_string()
        } else {
            let base = ident
                .clone()
                .unwrap_or_else(|| path_name(member_name).to_string());
            base.strip_suffix(".xml").unwrap_or(&base).to_string()
        }
    };
    let jur_type = classify_uid(&source_uid);

    let dossier = root.find("Dossier");
    let audience = root.find("Audience");
    let decision = root.find("Decision");

    let date_lecture = dossier.and_then(|d| node_text(d.find_first(&["Date_Lecture"])));
    let date_audience = audience.and_then(|a| node_text(a.find_first(&["Date_Audience"])));
    let texte_integral_raw = decision
        .and_then(|d| node_text(d.find_first(&["Texte_Integral"])))
        .unwrap_or_default();
    let texte_integral_clean = clean_texte(&texte_integral_raw);
    let sections = extract_sections_xml(&texte_integral_clean);
    let metadata_header = build_metadata_header_xml(&root);
    let visa_trim = build_visa_trim_xml(&sections);

    let publication_codes = dossier
        .and_then(|d| node_text(d.find_first(&["Code_Publication"])))
        .map(|c| vec![c])
        .unwrap_or_default();

    Decision {
        source_uid,
        member_name: member_name.to_string(),
        // L'XML opendata ne porte pas d'ECLI (ADR 0080).
        ecli: None,
        jurisdiction_source_code: dossier
            .and_then(|d| node_text(d.find_first(&["Code_Juridiction"]))),
        chamber: None,
        nac: None,
        jurisdiction_name: dossier.and_then(|d| node_text(d.find_first(&["Nom_Juridiction"]))),
        jurisdiction_type: jur_type,
        jurisdiction_location: None,
        numero_dossier: dossier.and_then(|d| node_text(d.find_first(&["Numero_Dossier"]))),
        numero_dossiers: None,
        numero_role: audience.and_then(|a| node_text(a.find_first(&["Numero_Role"]))),
        date_lecture: date_lecture.clone().or_else(|| date_audience.clone()),
        date_audience,
        date_mise_jour: node_text(root.find("Donnees_Techniques/Date_Mise_Jour")),
        formation: audience.and_then(|a| node_text(a.find_first(&["Formation_Jugement"]))),
        type_decision: dossier.and_then(|d| node_text(d.find_first(&["Type_Decision"]))),
        type_recours: dossier.and_then(|d| node_text(d.find_first(&["Type_Recours"]))),
        solution: dossier.and_then(|d| node_text(d.find_first(&["Solution"]))),
        publication_codes,
        avocat_requerant: dossier.and_then(|d| node_text(d.find_first(&["Avocat_Requerant"]))),
        texte_integral_raw,
        texte_integral_clean,
        sections,
        metadata_header,
        visa_trim,
        themes: Vec::new(),
        attacked: None,
        parse_warnings: warnings,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Judilibre (port fidèle de judilibre_parser.py)
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinelle pour `start_char` / `end_char` d'une section synthétique sans
/// offset dans le texte (visa Judilibre). Python utilise `-1` ; côté Rust les
/// offsets sont `usize`, on encode l'absence d'offset par `usize::MAX`.
pub const SECTION_NO_OFFSET: usize = usize::MAX;

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

// `PAR\s+CES\s+MOTIFS`, IGNORECASE.
static PAR_CES_MOTIFS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)PAR\s+CES\s+MOTIFS").unwrap());

// Verbe de dispositif en tête de ligne (multiline), casse-sensible (majuscules).
static DISPOSITIF_VERB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:LA\s+COUR[^\n:]*:\s*)?(?:REJETTE|CASSE|ANNULE|CONFIRME|INFIRME|CONDAMNE|D[ÉE]BOUTE|D[ÉE]CLARE|ORDONNE|PRONONCE|RENVOIE|ACCUEILLE|RABAT)\b",
    )
    .unwrap()
});

const DISPOSITIF_VERB_TAIL_RATIO: f64 = 0.65;

/// (`zone_key`, `kind`, `label`).
const ZONE_KIND_MAP: &[(&str, &str, &str)] = &[
    ("introduction", "preamble", "Introduction"),
    ("expose", "expose", "Exposé du litige"),
    ("moyens", "moyens", "Moyens"),
    ("motivations", "motivations", "Motivations"),
    ("dispositif", "dispositif", "Dispositif"),
];

// ── Helpers char-aware (les offsets Judilibre indexent par codepoint) ────────

/// Nombre de codepoints dans `s`.
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Slice `[start, end)` en indices de **codepoints** (comme `s[start:end]` Python).
pub(crate) fn char_slice(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Prend les `n` premiers codepoints (comme `s[:n]` Python).
fn char_take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Index (codepoint) de la première occurrence de `needle` dans `haystack` à
/// partir du codepoint `from`, ou `None`. Réplique `str.find(needle, from)`.
fn char_find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(char_len(haystack)));
    }
    let hay: Vec<char> = haystack.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    if pat.len() > hay.len() {
        return None;
    }
    let last = hay.len() - pat.len();
    let mut i = from.min(hay.len());
    while i <= last {
        if hay[i..i + pat.len()] == pat[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `str.rfind("\n\n")` en index de codepoint.
fn char_rfind_double_newline(s: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let mut i = chars.len() - 2;
    loop {
        if chars[i] == '\n' && chars[i + 1] == '\n' {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// Position d'un fragment nettoyé dans `clean_text` (recherche monotone à partir
/// de `cursor`, repli sur recherche globale). Port de `_clean_offset`.
fn clean_offset(clean_text: &str, frag_clean: &str, cursor: usize) -> usize {
    let probe = char_take(frag_clean, 60);
    if let Some(pos) = char_find_from(clean_text, &probe, cursor) {
        return pos;
    }
    if let Some(pos) = char_find_from(clean_text, &probe, 0) {
        return pos;
    }
    cursor
}

/// Retire les balises HTML et trim. Port de `_strip_html` (`if not value` →
/// `None` et chaîne vide traités identiquement).
fn strip_html(value: Option<&str>) -> String {
    match value.filter(|v| !v.is_empty()) {
        None => String::new(),
        Some(v) => TAG_RE.replace_all(v, "").trim().to_string(),
    }
}

/// Index de début du dispositif via marqueurs, ou `None`. Port de
/// `_find_dispositif_start`. Renvoie un index de **codepoint**.
fn find_dispositif_start(text: &str) -> Option<usize> {
    if let Some(m) = PAR_CES_MOTIFS_RE.find(text) {
        return Some(byte_to_char_index(text, m.start()));
    }
    let threshold = (char_len(text) as f64 * DISPOSITIF_VERB_TAIL_RATIO) as usize;
    for m in DISPOSITIF_VERB_RE.find_iter(text) {
        let start = byte_to_char_index(text, m.start());
        if start >= threshold {
            return Some(start);
        }
    }
    None
}

/// Convertit un offset **byte** (rendu par `regex`) en offset **codepoint**.
pub(crate) fn byte_to_char_index(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx].chars().count()
}

/// Vocabulaire Judilibre — port de `judilibre_vocab.py`.
pub(crate) mod vocab {
    fn lookup<'a>(table: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
        let lc = key.to_lowercase();
        table.iter().find(|(k, _)| *k == lc).map(|(_, v)| *v)
    }

    const JURISDICTION_LABELS: &[(&str, &str)] = &[
        ("cc", "Cour de cassation"),
        ("ca", "Cour d'appel"),
        ("tj", "Tribunal judiciaire"),
        ("tcom", "Tribunal de commerce"),
        ("constit", "Conseil constitutionnel"),
        ("tc", "Tribunal des conflits"),
        ("cnil", "CNIL"),
        ("cedh", "Cour européenne des droits de l'homme"),
        ("cjue", "Cour de justice de l'Union européenne"),
        ("cnda", "Cour nationale du droit d'asile"),
        // Hors nomenclature Judilibre : porté par les payloads reconstruits du
        // backfill ArianeWeb DCE (ADR 0219).
        ("ce", "Conseil d'État"),
    ];

    const JURISDICTION_TYPES: &[(&str, &str)] = &[
        ("cc", "CC"),
        ("ca", "CA"),
        ("tj", "TJ"),
        ("tcom", "TCOM"),
        ("constit", "CONSTIT"),
        ("tc", "TC"),
        ("cnil", "CNIL"),
        ("cedh", "CEDH"),
        ("cjue", "CJUE"),
        ("cnda", "CNDA"),
        // Hors nomenclature Judilibre (backfill ArianeWeb DCE, ADR 0219).
        ("ce", "CE"),
    ];

    const CHAMBER_LABELS: &[(&str, &str)] = &[
        ("civ1", "Première chambre civile"),
        ("civ2", "Deuxième chambre civile"),
        ("civ3", "Troisième chambre civile"),
        ("soc", "Chambre sociale"),
        ("comm", "Chambre commerciale"),
        ("cr", "Chambre criminelle"),
        ("mi", "Chambre mixte"),
        ("pl", "Assemblée plénière"),
        ("ord", "Ordonnance du Premier président"),
        ("ordo", "Ordonnance du Premier président"),
        ("creun", "Chambre réunies"),
        ("allciv", "Toutes chambres civiles"),
        ("other", "Autre formation"),
    ];

    const FORMATION_LABELS: &[(&str, &str)] = &[
        ("fs", "Formation de section"),
        ("fp", "Formation plénière"),
        ("f", "Formation restreinte"),
        ("fm", "Formation mixte"),
        ("frh", "Formation restreinte hors RNSM"),
        ("frr", "Formation restreinte RNSM"),
    ];

    const TYPE_LABELS: &[(&str, &str)] = &[
        ("arret", "Arrêt"),
        ("ordonnance", "Ordonnance"),
        ("avis", "Avis"),
        ("qpc", "QPC"),
        ("saisie", "Saisie"),
        ("autre", "Autre"),
        ("other", "Autre"),
    ];

    const SOLUTION_LABELS: &[(&str, &str)] = &[
        ("rejet", "Rejet"),
        ("cassation", "Cassation"),
        ("cassation_partielle", "Cassation partielle"),
        ("annulation", "Annulation"),
        ("irrecevabilite", "Irrecevabilité"),
        ("non-admission", "Non-admission"),
        ("non-lieu", "Non-lieu"),
        ("desistement", "Désistement"),
        ("qpc_renvoi", "QPC — Renvoi"),
        ("qpc_non-lieu", "QPC — Non-lieu"),
        ("autre", "Autre"),
        ("other", "Autre"),
    ];

    /// `JURISDICTION_LABELS.get(code.lower(), code)`.
    pub fn jurisdiction_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(
            lookup(JURISDICTION_LABELS, code)
                .unwrap_or(code)
                .to_string(),
        )
    }

    /// `JURISDICTION_TYPES.get(code.lower())` — `None` si inconnu.
    pub fn jurisdiction_type(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        lookup(JURISDICTION_TYPES, code).map(str::to_string)
    }

    /// `CHAMBER_LABELS.get(code.lower(), code)`.
    pub fn chamber_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(lookup(CHAMBER_LABELS, code).unwrap_or(code).to_string())
    }

    /// `FORMATION_LABELS.get(code.lower(), code.upper())`.
    pub fn formation_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(
            lookup(FORMATION_LABELS, code)
                .map(str::to_string)
                .unwrap_or_else(|| code.to_uppercase()),
        )
    }

    /// `TYPE_LABELS.get(code.lower(), code)`.
    pub fn type_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(lookup(TYPE_LABELS, code).unwrap_or(code).to_string())
    }

    /// `SOLUTION_LABELS.get(code.lower(), code)`.
    pub fn solution_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(lookup(SOLUTION_LABELS, code).unwrap_or(code).to_string())
    }
}

// ── Accès JSON ───────────────────────────────────────────────────────────────

/// `payload.get(key)` rendu comme `&str` si présent et de type chaîne.
fn get_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

// ── Construction des sections ────────────────────────────────────────────────

struct Frag {
    start: usize,
    end: usize,
    kind: String,
    label: String,
    text: String,
}

/// Assemble les sections canoniques depuis `zones` + `visa`. Port fidèle de
/// `_build_sections`. Les offsets `zones` indexent `payload["text"]` brut.
fn build_sections(raw_text: &str, clean_text: &str, payload: &Value) -> Vec<DecisionSection> {
    if clean_text.is_empty() {
        return Vec::new();
    }

    let zones = payload.get("zones");

    // 1. Aplatis tous les fragments de zone, triés par position (texte brut).
    let mut raw_frags: Vec<(usize, usize, &str, &str)> = Vec::new();
    for (zone_key, kind, label) in ZONE_KIND_MAP {
        let spans = zones
            .and_then(|z| z.get(zone_key))
            .and_then(Value::as_array);
        if let Some(spans) = spans {
            for span in spans {
                let start = span.get("start").and_then(Value::as_u64);
                let end = span.get("end").and_then(Value::as_u64);
                if let (Some(start), Some(end)) = (start, end) {
                    raw_frags.push((start as usize, end as usize, kind, label));
                }
            }
        }
    }
    // tri stable sur la position de départ (comme Python `sort(key=f[0])`).
    raw_frags.sort_by_key(|f| f.0);

    // 2. Slice le brut, nettoie par fragment, recale en espace nettoyé.
    let mut frags: Vec<Frag> = Vec::new();
    let mut cursor = 0usize;
    for (raw_start, raw_end, kind, label) in raw_frags {
        let frag_clean = clean_texte(&char_slice(raw_text, raw_start, raw_end))
            .trim()
            .to_string();
        if frag_clean.is_empty() {
            continue;
        }
        let start = clean_offset(clean_text, &frag_clean, cursor);
        let end = start + char_len(&frag_clean);
        cursor = end;
        frags.push(Frag {
            start,
            end,
            kind: kind.to_string(),
            label: label.to_string(),
            text: frag_clean,
        });
    }

    // 3. Fragment « motivations » orphelin EN TÊTE replié dans l'introduction.
    let body_anchor = frags
        .iter()
        .filter(|f| f.kind == "moyens" || f.kind == "expose")
        .map(|f| f.start)
        .min();
    if let Some(body_anchor) = body_anchor {
        let motiv_count = frags.iter().filter(|f| f.kind == "motivations").count();
        let any_after = frags
            .iter()
            .any(|f| f.kind == "motivations" && f.start >= body_anchor);
        if motiv_count >= 2 && any_after {
            for f in frags.iter_mut() {
                if f.kind == "motivations" && f.start < body_anchor {
                    f.kind = "preamble".to_string();
                    f.label = "Introduction".to_string();
                }
            }
        }
    }

    let mut out: Vec<DecisionSection> = frags
        .into_iter()
        .map(|f| DecisionSection {
            label: f.label,
            kind: f.kind,
            start_char: f.start,
            end_char: f.end,
            text: f.text,
        })
        .collect();

    // 4. Visa synthétique (refs légales HTML, sans offset dans le texte).
    if let Some(visa_titles) = payload.get("visa").and_then(Value::as_array) {
        if !visa_titles.is_empty() {
            let visa_text = visa_titles
                .iter()
                .map(|item| strip_html(item.get("title").and_then(Value::as_str)))
                .collect::<Vec<_>>()
                .join("\n");
            let visa_text = visa_text.trim().to_string();
            if !visa_text.is_empty() {
                let preamble_idx = out.iter().position(|s| s.kind == "preamble");
                let insertion_idx = match preamble_idx {
                    Some(i) => i + 1,
                    None => 0, // Python: preamble_idx = -1 → insertion_idx = 0.
                };
                out.insert(
                    insertion_idx,
                    DecisionSection {
                        label: "Visa".to_string(),
                        kind: "visa".to_string(),
                        start_char: SECTION_NO_OFFSET,
                        end_char: SECTION_NO_OFFSET,
                        text: visa_text,
                    },
                );
            }
        }
    }

    out
}

/// Ajoute une section `dispositif` par marqueurs si les zones n'en ont pas.
/// Port fidèle de `_augment_dispositif_fallback`.
fn augment_dispositif_fallback(sections: Vec<DecisionSection>, text: &str) -> Vec<DecisionSection> {
    if text.is_empty() || sections.iter().any(|s| s.kind == "dispositif") {
        return sections;
    }
    let start = match find_dispositif_start(text) {
        Some(s) => s,
        None => return sections,
    };
    let dispositif_text = char_slice(text, start, char_len(text)).trim().to_string();
    if dispositif_text.is_empty() {
        return sections;
    }

    let mut out: Vec<DecisionSection> = Vec::with_capacity(sections.len() + 1);
    for section in sections {
        // `0 <= start_char < start < end_char` — la borne synthétique du visa
        // (SECTION_NO_OFFSET) ne peut pas satisfaire `start_char < start` car
        // start ≤ len(text) ; on garde donc la section telle quelle, comme Python.
        if section.start_char != SECTION_NO_OFFSET
            && section.start_char < start
            && start < section.end_char
        {
            let trimmed = char_slice(text, section.start_char, start)
                .trim()
                .to_string();
            if !trimmed.is_empty() {
                out.push(DecisionSection {
                    end_char: start,
                    text: trimmed,
                    ..section
                });
            }
        } else {
            out.push(section);
        }
    }
    out.push(DecisionSection {
        label: "Dispositif".to_string(),
        kind: "dispositif".to_string(),
        start_char: start,
        end_char: char_len(text),
        text: dispositif_text,
    });
    out
}

/// Header 3 lignes (ADR 0018). Port fidèle de `_build_metadata_header`.
fn build_metadata_header(payload: &Value) -> String {
    let jur = vocab::jurisdiction_label(get_str(payload, "jurisdiction"));
    let chamber = vocab::chamber_label(get_str(payload, "chamber"));
    let date_lecture = get_str(payload, "decision_date").filter(|s| !s.is_empty());
    let type_recours = vocab::type_label(get_str(payload, "type"));
    let solution = vocab::solution_label(get_str(payload, "solution"));
    let formation = vocab::formation_label(get_str(payload, "formation"));

    let mut line1: Vec<String> = Vec::new();
    let jur_chamber = [jur, chamber]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if !jur_chamber.is_empty() {
        line1.push(jur_chamber);
    }
    if let Some(d) = date_lecture {
        line1.push(d.to_string());
    }

    let mut line2: Vec<String> = Vec::new();
    if let Some(t) = type_recours.filter(|s| !s.is_empty()) {
        line2.push(format!("Recours : {t}"));
    }
    if let Some(s) = solution.filter(|s| !s.is_empty()) {
        line2.push(format!("Solution : {s}"));
    }

    let mut lines: Vec<String> = Vec::new();
    if !line1.is_empty() {
        lines.push(line1.join(" | "));
    }
    if !line2.is_empty() {
        lines.push(line2.join(" | "));
    }
    if let Some(f) = formation.filter(|s| !s.is_empty()) {
        lines.push(format!("Formation : {f}"));
    }

    lines.join("\n")
}

/// visa_trim Judilibre : préfixe `expose + moyens` paragraph-aware. Port fidèle
/// de `_build_visa_trim`.
fn build_visa_trim(sections: &[DecisionSection]) -> String {
    let parts: Vec<&str> = sections
        .iter()
        .filter(|s| s.kind == "expose" || s.kind == "moyens")
        .map(|s| s.text.as_str())
        .collect();
    if parts.is_empty() {
        return String::new();
    }

    let visa = parts.join("\n\n");
    if char_len(&visa) <= VISA_TRIM_MAX_CHARS {
        return visa;
    }

    let truncated = char_take(&visa, VISA_TRIM_MAX_CHARS);
    match char_rfind_double_newline(&truncated) {
        Some(last_para) if last_para > 0 => char_take(&truncated, last_para),
        _ => truncated,
    }
}

/// Parse un payload JSON Judilibre (CC/CA/TJ/TCOM) en `Decision`.
///
/// Port fidèle de `judilibre_parser.parse`. `payload` est un objet `/decision`
/// ou un item `/export` (même schéma). Panique si `payload` n'a pas d'`id` —
/// réplique du `ValueError` Python (frontière de validation source unique).
/// Thèmes Judilibre (`payload["themes"]`) : liste verbatim (matière → chaîne
/// de mots-clés), entrées vides écartées.
fn parse_themes(payload: &Value) -> Vec<String> {
    payload
        .get("themes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_judilibre(payload: &Value, member_name: Option<&str>) -> Decision {
    let decision_id = get_str(payload, "id").filter(|s| !s.is_empty());
    let decision_id = decision_id.unwrap_or_else(|| panic!("payload Judilibre sans 'id'"));

    let raw_text = get_str(payload, "text").unwrap_or("");
    let text = clean_texte(raw_text);
    let sections = build_sections(raw_text, &text, payload);
    let sections = augment_dispositif_fallback(sections, &text);

    // `numbers` : liste propre, dédoublonnée en gardant l'ordre. Le scalaire
    // `number` est corrompu (cf. commentaire Python).
    let mut clean_numbers: Vec<String> = Vec::new();
    if let Some(arr) = payload.get("numbers").and_then(Value::as_array) {
        for n in arr {
            if let Some(s) = n.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() && !clean_numbers.iter().any(|x| x == trimmed) {
                    clean_numbers.push(trimmed.to_string());
                }
            }
        }
    }

    let number_scalar = get_str(payload, "number").map(str::to_string);
    let numero_dossier = clean_numbers.first().cloned().or(number_scalar);
    let numero_dossiers = if clean_numbers.is_empty() {
        None
    } else {
        Some(clean_numbers)
    };

    let visa_trim = build_visa_trim(&sections);
    let metadata_header = build_metadata_header(payload);

    Decision {
        source_uid: format!("judilibre/{decision_id}"),
        member_name: member_name
            .map(str::to_string)
            .unwrap_or_else(|| decision_id.to_string()),
        ecli: get_str(payload, "ecli")
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        jurisdiction_source_code: None,
        chamber: get_str(payload, "chamber").map(str::to_string),
        nac: get_str(payload, "nac")
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        jurisdiction_name: vocab::jurisdiction_label(get_str(payload, "jurisdiction")),
        jurisdiction_type: vocab::jurisdiction_type(get_str(payload, "jurisdiction")),
        jurisdiction_location: get_str(payload, "location").map(str::to_string),
        numero_dossier,
        numero_dossiers,
        numero_role: None,
        date_lecture: get_str(payload, "decision_date").map(str::to_string),
        date_audience: None,
        date_mise_jour: get_str(payload, "update_date").map(str::to_string),
        formation: vocab::formation_label(get_str(payload, "formation")),
        type_decision: None,
        type_recours: vocab::type_label(get_str(payload, "type")),
        solution: vocab::solution_label(get_str(payload, "solution")),
        publication_codes: payload
            .get("publication")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        avocat_requerant: None,
        texte_integral_raw: raw_text.to_string(),
        texte_integral_clean: text,
        sections,
        metadata_header,
        visa_trim,
        themes: parse_themes(payload),
        attacked: None,
        parse_warnings: Vec::new(),
    }
}

/// Construit `source_fields` (ADR 0085) depuis le payload JSON source et les
/// sections déjà rebasées sur le texte nettoyé (`decision.sections`).
///
/// Décomposition quasi-bijective `source_payload ⟷ (full_text, source_fields)` :
/// on retire `text` (stocké à part dans `full_text`) et on remplace `zones`
/// (offsets sur le texte brut) par `sections` — `[{kind, start, end}]`, offsets
/// rebasés sur `full_text` (codepoints du texte nettoyé). C'est l'unique
/// transformation (invariant d'offsets) ; le reste du payload (`visa`, `themes`,
/// `summary`, `nac`, `ecli`…) est conservé verbatim.
///
/// Les sections synthétiques sans offset dans le texte (visa Judilibre,
/// `SECTION_NO_OFFSET`) sont **exclues** : leur texte n'est pas une tranche de
/// `full_text` (refs HTML), elles se reconstruisent au rendu depuis
/// `source_fields["visa"]`.
///
/// Sources sans payload JSON structuré (XML opendata : `<Dossier>` déjà en
/// colonnes, sections redétectables sur `full_text`) → passer un `payload`
/// non-objet rend `Value::Null`.
pub fn build_source_fields(payload: &Value, sections: &[DecisionSection]) -> Value {
    let Some(obj) = payload.as_object() else {
        return Value::Null;
    };
    let mut out = obj.clone();
    out.remove("text");
    out.remove("zones");

    let rebased: Vec<Value> = sections
        .iter()
        .filter(|s| s.start_char != SECTION_NO_OFFSET)
        .map(|s| {
            serde_json::json!({
                "kind": s.kind,
                "start": s.start_char,
                "end": s.end_char,
            })
        })
        .collect();
    if !rebased.is_empty() {
        out.insert("sections".to_string(), Value::Array(rebased));
    }

    Value::Object(out)
}

/// `true` si `source_fields` provient d'un payload XML opendata : présence d'un
/// nœud `<Dossier>`/`<Audience>` (clés spécifiques opendata, qu'un payload
/// Judilibre JSON n'a jamais). Discriminant interne du dispatch
/// [`Decision::from_source_fields`] (ADR 0085), déduit de la structure de
/// `source_fields`.
fn source_fields_is_xml(source_fields: &Value) -> bool {
    source_fields.get("Dossier").is_some() || source_fields.get("Audience").is_some()
}

impl Decision {
    /// Convertisseur canonique `(full_text, source_fields) → Decision` (ADR 0085),
    /// pendant exact des `build_source_fields*` (`xml`/`dila`/HTML
    /// CEDH-CJUE/CNDA). Chemin d'extraction LINÉAIRE unique : utilisé à l'ingest
    /// comme en aval (re-extract, affichage). Dispatch sur le **préfixe du
    /// `source_uid`** d'abord (discriminant stable en DB, ADR 0094/0096) pour les
    /// fonds scrapés — `cedh/`/`cjue/` ⇒ HTML européen ; `cnda/` ⇒ CNDA —, puis sur
    /// la **forme de `source_fields`** : `<Dossier>`/`<Audience>` ⇒ XML opendata
    /// (TA/CAA/CE) ; `META_COMMUN`+`META_JURI` ⇒ DILA bulk (JADE/CONSTIT, fond
    /// déduit du `source_uid` `dila-jade`/`dila-constit`) ; sinon JSON Judilibre
    /// (CC/CA/TJ/TCOM). `source_uid` est la provenance (sert de `member_name` et
    /// de base à `classify_uid` côté XML, au fond côté DILA).
    pub fn from_source_fields(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        if source_uid_is_html_europe(source_uid) {
            Decision::from_source_fields_html_europe(full_text, source_fields, source_uid)
        } else if source_uid_is_cnda(source_uid) {
            Decision::from_source_fields_cnda(full_text, source_fields, source_uid)
        } else if source_fields_is_xml(source_fields) {
            Decision::from_source_fields_xml(full_text, source_fields, source_uid)
        } else if source_fields_is_dila(source_fields) {
            Decision::from_source_fields_dila(full_text, source_fields, source_uid)
        } else {
            Decision::from_source_fields_json(full_text, source_fields, source_uid)
        }
    }

    /// Branche XML opendata de [`Decision::from_source_fields`]. Mappe les
    /// scalaires `<Dossier>`/`<Audience>` (conservés verbatim par
    /// [`build_source_fields_xml`]) vers `Decision` comme [`parse_xml`], et
    /// recalcule sections/`metadata_header`/`visa_trim` sur `full_text` (offsets
    /// recomputés, aucun rebasage). `raw == clean == full_text` ; `date_mise_jour`
    /// n'est pas porté en `source_fields` (donc `None`).
    fn from_source_fields_xml(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        let get = |parent: &str, key: &str| -> Option<String> {
            source_fields
                .get(parent)
                .and_then(|node| node.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };

        let date_lecture = get("Dossier", "Date_Lecture");
        let date_audience = get("Audience", "Date_Audience");
        let publication_codes = get("Dossier", "Code_Publication")
            .map(|c| vec![c])
            .unwrap_or_default();
        let sections = extract_sections_xml(full_text);
        let visa_trim = build_visa_trim_xml(&sections);

        Decision {
            source_uid: source_uid.to_string(),
            member_name: source_uid.to_string(),
            // L'XML opendata ne porte pas d'ECLI (ADR 0080).
            ecli: None,
            jurisdiction_source_code: get("Dossier", "Code_Juridiction"),
            chamber: None,
            nac: None,
            jurisdiction_name: get("Dossier", "Nom_Juridiction"),
            jurisdiction_type: classify_uid(source_uid),
            jurisdiction_location: None,
            numero_dossier: get("Dossier", "Numero_Dossier"),
            numero_dossiers: None,
            numero_role: get("Audience", "Numero_Role"),
            date_lecture: date_lecture.or_else(|| date_audience.clone()),
            date_audience,
            date_mise_jour: None,
            formation: get("Audience", "Formation_Jugement"),
            type_decision: get("Dossier", "Type_Decision"),
            type_recours: get("Dossier", "Type_Recours"),
            solution: get("Dossier", "Solution"),
            publication_codes,
            avocat_requerant: get("Dossier", "Avocat_Requerant"),
            texte_integral_raw: full_text.to_string(),
            texte_integral_clean: full_text.to_string(),
            sections,
            metadata_header: build_metadata_header_xml_from_fields(source_fields),
            visa_trim,
            themes: Vec::new(),
            attacked: None,
            parse_warnings: Vec::new(),
        }
    }

    /// Branche JSON Judilibre de [`Decision::from_source_fields`]. Reproduit
    /// exactement le parse direct : reconstruit les `zones` depuis
    /// `source_fields["sections"]` (inverse de [`ZONE_KIND_MAP`]) puis applique
    /// [`build_sections`] + [`augment_dispositif_fallback`] sur
    /// `(full_text, payload-like)`. Le visa synthétique se reforme depuis
    /// `source_fields["visa"]` (conservé verbatim). `raw == clean == full_text`.
    fn from_source_fields_json(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        let payload = json_payload_from_source_fields(full_text, source_fields);

        let sections = build_sections(full_text, full_text, &payload);
        let sections = augment_dispositif_fallback(sections, full_text);

        // `numbers` : liste propre, dédoublonnée en gardant l'ordre. Le scalaire
        // `number` est corrompu (cf. parse_judilibre).
        let mut clean_numbers: Vec<String> = Vec::new();
        if let Some(arr) = payload.get("numbers").and_then(Value::as_array) {
            for n in arr {
                if let Some(s) = n.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() && !clean_numbers.iter().any(|x| x == trimmed) {
                        clean_numbers.push(trimmed.to_string());
                    }
                }
            }
        }
        let number_scalar = get_str(&payload, "number").map(str::to_string);
        let numero_dossier = clean_numbers.first().cloned().or(number_scalar);
        let numero_dossiers = if clean_numbers.is_empty() {
            None
        } else {
            Some(clean_numbers)
        };

        let visa_trim = build_visa_trim(&sections);
        let metadata_header = build_metadata_header(&payload);

        Decision {
            source_uid: source_uid.to_string(),
            member_name: source_uid.to_string(),
            ecli: get_str(&payload, "ecli")
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            jurisdiction_source_code: None,
            chamber: get_str(&payload, "chamber").map(str::to_string),
            nac: get_str(&payload, "nac")
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            jurisdiction_name: vocab::jurisdiction_label(get_str(&payload, "jurisdiction")),
            jurisdiction_type: vocab::jurisdiction_type(get_str(&payload, "jurisdiction")),
            jurisdiction_location: get_str(&payload, "location").map(str::to_string),
            numero_dossier,
            numero_dossiers,
            numero_role: None,
            date_lecture: get_str(&payload, "decision_date").map(str::to_string),
            date_audience: None,
            date_mise_jour: get_str(&payload, "update_date").map(str::to_string),
            formation: vocab::formation_label(get_str(&payload, "formation")),
            type_decision: None,
            type_recours: vocab::type_label(get_str(&payload, "type")),
            solution: vocab::solution_label(get_str(&payload, "solution")),
            publication_codes: payload
                .get("publication")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            avocat_requerant: None,
            texte_integral_raw: full_text.to_string(),
            texte_integral_clean: full_text.to_string(),
            sections,
            metadata_header,
            visa_trim,
            themes: parse_themes(&payload),
            attacked: parse_attacked(&payload),
            parse_warnings: Vec::new(),
        }
    }
}

/// `payload["contested"]` Judilibre -> [`crate::decision::AttackedRef`]
/// (ADR 0161) : matiere brute du lien de chronologie. `None` si le champ est
/// absent, nul, ou vide de tout discriminant.
fn parse_attacked(payload: &Value) -> Option<crate::decision::AttackedRef> {
    let c = payload.get("contested")?;
    if !c.is_object() {
        return None;
    }
    let s = |k: &str| {
        c.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let r = crate::decision::AttackedRef {
        jurisdiction: s("jurisdiction").or_else(|| s("title")),
        number: s("number"),
        date: s("date"),
    };
    if r.jurisdiction.is_none() && r.number.is_none() && r.date.is_none() {
        return None;
    }
    Some(r)
}

/// Reconstruit le payload-like JSON Judilibre (entrée de [`build_sections`] /
/// [`build_metadata_header`]) depuis `(full_text, source_fields)` : ré-insère
/// `text = full_text` et reconstruit `zones` depuis `source_fields["sections"]`
/// (inverse de [`ZONE_KIND_MAP`]). Les offsets `sections` sont en codepoints du
/// texte nettoyé ; comme `full_text` EST ce texte, [`build_sections`] les
/// ré-applique sans décalage (`clean_texte` idempotent). Le reste du payload
/// (`visa`, `publication`, `ecli`…) est conservé verbatim.
fn json_payload_from_source_fields(full_text: &str, source_fields: &Value) -> Value {
    let Some(obj) = source_fields.as_object() else {
        return Value::Null;
    };
    let mut out = obj.clone();
    let sections = out.remove("sections");
    out.insert("text".to_string(), Value::String(full_text.to_string()));

    if let Some(sections) = sections.as_ref().and_then(Value::as_array) {
        let mut zones = serde_json::Map::new();
        for sec in sections {
            let (Some(kind), Some(start), Some(end)) = (
                sec.get("kind").and_then(Value::as_str),
                sec.get("start").and_then(Value::as_u64),
                sec.get("end").and_then(Value::as_u64),
            ) else {
                continue;
            };
            let Some((zone_key, _, _)) = ZONE_KIND_MAP.iter().find(|(_, k, _)| *k == kind) else {
                continue;
            };
            let span = serde_json::json!({ "start": start, "end": end });
            match zones
                .entry(zone_key.to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
            {
                Value::Array(a) => a.push(span),
                _ => unreachable!("entry insère toujours un Array"),
            }
        }
        if !zones.is_empty() {
            out.insert("zones".to_string(), Value::Object(zones));
        }
    }
    Value::Object(out)
}

/// Scalaires directs (feuilles) d'un nœud XML : `{ tag: texte }` pour chaque
/// enfant sans enfant. Première occurrence retenue par tag — comme `find_first`,
/// qui prend le premier match en ordre document.
pub(crate) fn xml_scalar_children(node: &XmlNode) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for child in &node.children {
        if child.children.is_empty() {
            if let Some(text) = child.text() {
                out.entry(child.tag.clone()).or_insert(Value::String(text));
            }
        }
    }
    out
}

/// `source_fields` pour un payload XML opendata (ADR 0085) : tous les scalaires
/// (feuilles) des nœuds `<Dossier>` et `<Audience>`, groupés
/// `{ "Dossier": {…}, "Audience": {…} }`.
///
/// Pendant XML du `build_source_fields` JSON. Le texte intégral part dans
/// `full_text` ; les sections et `visa_trim` se recalculent depuis `full_text`
/// (`extract_sections_xml`), donc rien à rebaser ici. `metadata_header` (XML) ne
/// lit que ces scalaires — d'où la conservation verbatim de **tous** les
/// scalaires `Dossier`/`Audience` (et pas seulement les 6 lus aujourd'hui) :
/// quasi-bijection, le payload XML est reconstructible hors texte intégral.
pub fn build_source_fields_xml(raw: &[u8]) -> Value {
    let root = build_tree(raw).unwrap_or_default();
    let mut obj = serde_json::Map::new();
    for parent in ["Dossier", "Audience"] {
        if let Some(node) = root.find(parent) {
            let scalars = xml_scalar_children(node);
            if !scalars.is_empty() {
                obj.insert(parent.to_string(), Value::Object(scalars));
            }
        }
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests;
