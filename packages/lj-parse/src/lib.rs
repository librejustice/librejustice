//! Parsers tolérants vers le modèle `Decision`.
//!
//! XML via `quick-xml` (tolérant, mode recover). JSON Judilibre via `serde_json`.
//! Aucun I/O : les bytes/valeurs sont fournis par l'appelant (`lj-sources`).

use lj_core::decision::{Decision, DecisionSection};
use lj_core::normalizer::clean_texte;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::Value;
use std::sync::LazyLock;

use regex::Regex;

mod a2aj;
mod adala;
mod adala_ar;
mod adlc;
mod amf;
mod anbenin;
mod ariane;
mod bwb;
mod cc_site;
mod cce;
mod ce_list;
mod cnda;
mod cnlegis;
mod codex;
mod comeu;
mod consiliaweb;
mod consolidated;
mod contribution;
mod court_sites;
mod court_summary;
mod court_summary_ocr;
mod ddd;
mod dglex;
mod dila;
mod dila_judi;
mod doctrine_oai;
mod ejo;
mod elaws;
mod entscheidsuche;
mod european;
mod fedlex;
pub mod formex;
mod gallilex;
mod hal;
mod juportal;
mod juradm_site;
mod juricaf;
mod justel;
mod justice_laws;
mod justice_lu;
mod lead;
mod legalis;
mod legilux;
mod legimonaco;
mod lexfind;
mod lexwork;
mod lims;
mod opencaselaw;
mod pdf_articles;
mod peachjam;
pub mod questions;
mod rechtspraak;
mod reflow;
mod scc_csc;
mod service_public;
mod vd;
mod wallex;
mod wiki_names;
pub use a2aj::{
    a2aj_court, a2aj_names, a2aj_source_uid, parse_a2aj, A2ajCourt, A2ajDecision, A2ajLanguage,
    A2ajRefused, A2AJ_COURTS,
};
pub use adala::{
    adala_consolidated, adala_folder, adala_identity, adala_promulgation, AdalaFile, AdalaFolder,
    AdalaIdentity,
};
pub use adala_ar::{
    adala_arabic_articles, adala_arabic_body, adala_arabic_consolidated, adala_arabic_day,
    adala_arabic_identity, adala_arabic_promulgation, adala_arabic_text,
};
pub use adlc::{adlc_is_decision, adlc_number, adlc_source_fields, adlc_text};
pub use amf::{amf_decided, amf_named_until, amf_source_fields};
pub use anbenin::{anbenin_laws, anbenin_pdfs, AnbeninLaw};
pub use ariane::{
    analyse_body, build_ariane_source_fields, parse_ajce_html, parse_dce_html, AjceAnalysis,
    AjceEntry, AjceRubrique, DceParsed,
};
pub use bwb::{
    bwb_abbreviations, bwb_manifest, bwb_sitemap, bwb_toestand, BwbListed, BwbState, BwbToestand,
};
pub use cc_site::{cc_site_decision, CcSiteDecision, CcSitePage, CcSiteRefused};
pub use cce::{
    cce_names, cce_source_uid, parse_cce, CceArret, CceLanguage, CceParsed, CceRefused, CCE_CODE,
    CCE_LABEL, CCE_LANGUAGES, CCE_PLACE, CCE_TYPE,
};
pub use ce_list::{
    ce_list_names, is_pseudonymised, parse_ce_list_anchors, parse_ce_list_page, split_joint_name,
    CeListAnchor, CeListEntry, CeListName,
};
use cnda::source_uid_is_cnda;
pub use cnda::{
    clean_ocr_markdown, cnda_solution_uid_from_source_fields, parse_cnda, reflow_cnda_pdf_text,
    CndaParsed,
};
pub use cnlegis::{cnlegis_court, cnlegis_nature, cnlegis_page, CnlegisPage, CnlegisText};
pub use codex::{codex_state, CodexState, CodexVersion};
pub use comeu::{comeu_court_code, comeu_first, COMEU_ADOPTION_DATE, COMEU_COURTS};
pub use consiliaweb::{consiliaweb_date, consiliaweb_number, consiliaweb_source_fields};
pub use consolidated::ConsolidatedArticle;
pub use contribution::contribution_source_fields;
use contribution::source_uid_is_contribution;
pub use court_sites::{
    african_court_advisories, african_court_advisory_pdf, african_court_listing,
    cassation_ci_listing, cc_bj_listing, cc_ci_listing, cc_dj_listing, cc_ma_listing, cc_ma_page,
    ccja_listing, cndj_ci_listing, ejo_ruling, garbled_layer, hcc_mg_listing, holds_a_decision,
    html_text, jurisprudence_mg_listing, jurisprudence_mg_page, parse_site_decision, site_court,
    site_court_of_uid, SiteBody, SiteCourt, SiteEntry, SiteListing, SiteParsed, CCJA_PAGE,
    CC_MA_PAGE, SITE_COURTS,
};
pub use court_summary::{
    cassation_summary_for, cassation_summary_numbers_arrets, cedh_legal_summary,
    cedh_legal_summary_pending, cjue_court_summary, cjue_summary_without_head,
};
pub use court_summary_ocr::{cjue_case_number, cjue_court_summary_ocr, OcrBlock, OcrPage};
pub use ddd::{
    ddd_concerns_minor, ddd_jurisprudence_court, ddd_jurisprudence_source_uid, ddd_source_fields,
};
pub use dglex::{dglex_articles, dglex_listing, DglexList, DglexListed};
use dila::source_fields_is_dila;
pub use dila::{
    build_source_fields_dila, cnil_doctrine, parse_dila_doc, parse_dila_xml, CnilDoctrine, DilaDoc,
    DilaFond, CNIL_DOCTRINE_NATURES,
};
pub use dila_judi::judicial_record_uids;
pub use doctrine_oai::{
    build_doctrine_source_fields, cairn_label_title, cairn_note, cited_decisions, openedition_note,
    page_dc_title, parse_oai_page, prairial_note, prairial_page_url, robots_allows, same_note,
    Citation, DoctrineNote, OaiPage, OaiRecord,
};
pub use ejo::{
    ejo_council_ruling, ejo_effects, ejo_general, ejo_individual, ejo_issues, ejo_reference_key,
    ejo_terms, ejo_texts, ejo_title_reference, journal_articles, journal_body, journal_number_key,
    journal_text_articles, journal_text_body, EjoEffect, EjoIssue, EjoNature, EjoText,
};
pub use elaws::{elaws_articles, elaws_sitemap, elaws_title, ElawsKind, ElawsListed};
pub use entscheidsuche::{
    court_names, entscheidsuche_scan, parse_entscheidsuche, states_day, swiss_court,
    EntscheidsucheParsed, SwissCourt, ENTSCHEIDSUCHE_PARSE_VERSION, SWISS_COURTS,
};
use european::source_uid_is_html_europe;
pub use european::{
    cedh_case_name, celex_is_opinion, celex_to_case_number, cjue_advocate_general, cjue_case_name,
    cjue_title_case_name, eurlex_act_text, europe_names, parse_cedh, parse_cjue,
};
pub use fedlex::fedlex_articles;
pub use gallilex::{gallilex_articles, gallilex_page, GallilexPage};
pub use hal::{build_hal_source_fields, extract_dockets, extract_law_numbers, HalDeposit};
pub use juportal::{
    belgian_court, ecli_court_and_kind, is_juportal_page, juportal_feed, juportal_names,
    juportal_page_lang, juportal_taken, parse_juportal, BelgianCourt, JuportalParsed,
    BELGIAN_COURTS, BELGIAN_PLACE,
};
pub use juradm_site::{
    juradm_court, juradm_decision, juradm_site_source_fields, parse_juradm_head, JuradmCourt,
    JuradmHead, JuradmRefused, JURADM_COURTS,
};
pub use juricaf::{
    archived_text, juricaf_article, ohadata_decision, parse_juricaf, JuricafParsed, Recovered,
};
pub use justel::{
    justel_consolidated, justel_list, justel_page, JustelArticle, JustelConsolidated, JustelList,
    JustelListed, JustelNoText, JustelPage, JustelUntil,
};
pub use justice_laws::{
    justice_laws_articles, justice_laws_index, JusticeLawsEntry, JusticeLawsInstrument,
    JusticeLawsKind,
};
pub use justice_lu::{
    lu_court, parse_justice_lu, JusticeLuParsed, LuCourt, LuMember, LU_COURTS, LU_PLACE,
};
use lead::augment_lead;
pub use legalis::{
    legalis_court, legalis_date, legalis_decision, legalis_source_uid, LegalisCourt,
    LegalisDecision, LegalisPage, LegalisRefused,
};
pub use legilux::legilux_articles;
pub use legimonaco::{
    monaco_court, parse_legimonaco, LegimonacoParsed, MonacoCourt, MONACO, MONACO_COURTS,
};
pub use lexfind::{lexfind_articles, LexfindLayout};
pub use lexwork::lexwork_articles;
pub use lims::{lims_document, LimsDocument};
pub use opencaselaw::{
    obwalden_names, parse_opencaselaw, OpenCaseLawParsed, OpenCaseLawRefused, OBWALDEN_COURTS,
};
pub use peachjam::{peachjam_page, peachjam_sitemap, PeachjamListed, PeachjamPage};
pub use rechtspraak::{
    dutch_court, parse_rechtspraak, rechtspraak_checksummed, rechtspraak_court_segment,
    rechtspraak_dockets, rechtspraak_listing, rechtspraak_names, rechtspraak_taken, DutchCourt,
    RechtspraakListing, RechtspraakParsed, DUTCH_COURTS, DUTCH_PLACE,
};
pub use reflow::reflow_page_text;
pub use scc_csc::{
    scc_decision, scc_page_url, scc_source_uid, SccDecision, SccPage, SccRefused, SCC_CODE,
    SCC_LABEL, SCC_PLACE, SCC_TYPE,
};
pub use service_public::{
    parse_service_public_sheet, ArticleSpan, ServicePublicSheet, SheetReference,
};
pub use vd::vd_articles;
pub use wallex::{wallex_page, WallexPage, WallexVersion};
pub use wiki_names::{wiki_list_entries, wiki_page_entry, wiki_usage_name, WikiEntry};

// ─────────────────────────────────────────────────────────────────────────────
// XML opendata (TA / CAA / CE)
// ─────────────────────────────────────────────────────────────────────────────

// Réparation byte-level : `<Texte_Integral></p>` orphelin → `<Texte_Integral>`.
const BROKEN_TEXTE_INTEGRAL_OPEN: &[u8] = b"<Texte_Integral></p>";
const FIXED_TEXTE_INTEGRAL_OPEN: &[u8] = b"<Texte_Integral>";

/// Préfixes uid : D<X> (décisions), OR<X> (ordonnances).
const TYPE_BY_PREFIX: &[(&str, &str)] = &[
    ("DTA", "TA"),
    ("ORTA", "TA"),
    ("DCA", "CAA"),
    ("ORCA", "CAA"),
    ("DCE", "CE"),
    ("ORCE", "CE"),
];

// Patterns de sections (ordre préservé : kind, regex). IGNORECASE
// systématique. Le dispositif est aussi MULTILINE.
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
        // Conseil constitutionnel: its own visa, which no administrative decision
        // carries.
        ("visa", Regex::new(r"(?i)\bAu vu des textes suivants\s*:").unwrap()),
        // The Conseil d'État before 1986, printed in capitals without accents.
        (
            "visa",
            Regex::new(r"(?m)^[ \t]*VU\s+(?:LA|LE|LES|L')\s").unwrap(),
        ),
        // A text the decision applies, opening a line or a clause, whatever its case.
        (
            "visa",
            Regex::new(
                r"(?mi)(?:^[ \t]*|;[ \t]*)(Vu\s+(?:le|la|les|l')\s*(?:code|loi|décret|decret|constitution|convention|traité|traite|directive|règlement|reglement)\b)",
            )
            .unwrap(),
        ),
        ("motivations", Regex::new(r"(?i)\bConsidérant ce qui suit\b\s*:?\s*").unwrap()),
        // Conseil constitutionnel again, whose recent drafting closes its visa and
        // opens its dispositif in the middle of a line: the boundary is taken on
        // the numbered paragraph that follows, and on the heading itself.
        (
            "motivations",
            Regex::new(r"(?i)S['’]EST FONDÉ SUR CE QUI SUIT\s*:\s*\n\s*(\d+\.)").unwrap(),
        ),
        (
            "dispositif",
            Regex::new(
                r"(?mi)^(?:LE CONSEIL CONSTITUTIONNEL\s+)?(?:D\s*[ÉE]\s*C\s*I\s*D\s*E|O\s*R\s*D\s*O\s*N\s*N\s*E|A\s*R\s*R\s*[ÊE]\s*T\s*E)\s*:?\s*$",
            )
            .unwrap(),
        ),
        (
            "dispositif",
            Regex::new(r"(?mi)^[ \t]*(LE CONSEIL CONSTITUTIONNEL\s+D\s*[ÉE]\s*C\s*I\s*D\s*E)\s*:").unwrap(),
        ),
        (
            "dispositif",
            Regex::new(r"(?mi)^[ \t]*D[ÉE]CIDE\s*:?\s*ART(?:ICLE|\.)").unwrap(),
        ),
    ]
});

/// The CNIL's drafting, over the administrative markers: its visas open a line
/// on « Vu la loi », its reasons on what it examined or observes, its operative
/// part on what it grants or advises.
static CNIL_SECTION_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let mut patterns = SECTION_PATTERNS.clone();
    patterns.extend([
        (
            "visa",
            Regex::new(r"(?m)^[ \t]*Vu\s+(?:la|le|les|l['’])\s*\S").unwrap(),
        ),
        (
            "expose",
            Regex::new(r"(?m)^[ \t]*(?:[IVX]+\s*\.?\s*[\x{2013}-]?\s*)?(?:Faits et procédure|La saisine|Constate\b)")
                .unwrap(),
        ),
        (
            "motivations",
            Regex::new(
                r"(?m)^[ \t]*(?:(?:[IVX]+\s*\.?\s*[\x{2013}-]?\s*)?(?:Motifs de la décision|L['’]avis de la (?:CNIL|Commission))\b|Formule les observations suivantes|[ÉE]met l['’]avis suivant|A adopté la délibération suivante|Après en avoir délibéré|Sur (?:la|le|les|l['’])\s*(?:finalités?|données|destinataires|durées?|droits|information|sécurité|mesures|responsable|catégories|base légale|licéité|conformité|manquements?|recevabilité)\b|La Commission (?:considère|rappelle|estime|relève|observe|prend acte|note|constate)\b)",
            )
            .unwrap(),
        ),
        (
            "dispositif",
            Regex::new(
                r"(?m)^[ \t]*(?:Par ces motifs|PAR CES MOTIFS|Autorise\b|Met en demeure\b|Décide\s*:|Prononce\b|Délivre\b|Dans ces conditions, la Commission (?:autorise|émet|décide)\b|[ÉE]met un avis (?:favorable|défavorable)\b)",
            )
            .unwrap(),
        ),
    ]);
    patterns
});

/// Motifs of the drafting that predates « Considérant ce qui suit », read only
/// when that marker is absent: a decision carrying it may quote a « considérant
/// qu' » earlier, in the procedure it recites. The opening boundary is required,
/// « en considérant que » being how a party's argument is recited. Group 1 is
/// the marker, the match itself reaching back over the punctuation.
static MOTIFS_ANCIENS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;:.]\s+)(Consid[ée]rant\s+qu(?:['’]|e\b)|CONS\.\s+QU)").unwrap()
});

fn path_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Type de juridiction depuis le préfixe du nom uid.
fn classify_uid(source_uid: &str) -> Option<String> {
    let uid_name = path_name(source_uid).to_uppercase();
    TYPE_BY_PREFIX
        .iter()
        .find(|(prefix, _)| uid_name.starts_with(&format!("{prefix}_")))
        .map(|(_, jur_type)| (*jur_type).to_string())
}

/// Arbre XML minimal : balise locale + enfants + texte descendant agrégé.
#[derive(Debug, Default)]
pub struct XmlNode {
    pub tag: String,
    pub children: Vec<XmlNode>,
    /// The text of this node and of its descendants, and the HTML breaks between
    /// them, in document order.
    text_parts: Vec<TextPart>,
    /// Attributs directs du nœud : `(nom local, valeur)`, ordre document.
    attrs: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
enum TextPart {
    Text(String),
    /// Text inside a table cell, where the source's line breaks are layout.
    CellText(String),
    /// Whitespace between the cells and rows of a table.
    TableSpace(String),
    /// A `<br/>`.
    Break,
    /// The edge of a paragraph-level element.
    Block,
    /// The edge of a table row.
    Row,
    /// The start of a table cell.
    Cell,
}

/// What an HTML element leaves in the text at its edges.
fn edge_part(tag: &str) -> Option<TextPart> {
    match tag.to_ascii_lowercase().as_str() {
        "br" => Some(TextPart::Break),
        "tr" => Some(TextPart::Row),
        "td" | "th" => Some(TextPart::Cell),
        "p" | "div" | "blockquote" | "table" | "ul" | "ol" | "li" | "h1" | "h2" | "h3" | "h4"
        | "h5" | "h6" => Some(TextPart::Block),
        _ => None,
    }
}

/// The breaks met between two runs of text.
#[derive(Default)]
struct Gap {
    breaks: usize,
    block: bool,
    row: bool,
    cell: bool,
}

impl Gap {
    /// Appends a run of text to `out`, behind the separator the gap calls for
    /// when both sides carry text. A line break the source already has counts as
    /// one asked for.
    fn push(&mut self, out: &mut String, mut text: &str) {
        if text.trim().is_empty() {
            out.push_str(text);
            return;
        }
        if !out.trim().is_empty() {
            let body = out.trim_end().len();
            if self.cell && !self.row {
                out.truncate(body);
                out.push_str(" | ");
                text = text.trim_start();
            } else {
                let lead = text.len() - text.trim_start().len();
                let present =
                    out[body..].matches('\n').count() + text[..lead].matches('\n').count();
                let wanted = if self.row {
                    present.max(1)
                } else if self.block {
                    present.max(self.breaks).max(1)
                } else {
                    present.max(self.breaks)
                };
                for _ in present..wanted {
                    out.push('\n');
                }
            }
        }
        out.push_str(text);
        *self = Gap::default();
    }
}

impl XmlNode {
    /// Suit un chemin `A/B/C` d'enfants directs, premier match.
    pub fn find(&self, path: &str) -> Option<&XmlNode> {
        let mut current = self;
        for segment in path.split('/') {
            current = current.children.iter().find(|c| c.tag == segment)?;
        }
        Some(current)
    }

    fn find_first(&self, paths: &[&str]) -> Option<&XmlNode> {
        paths.iter().find_map(|p| self.find(p))
    }

    /// Texte descendant concaténé puis trimé ; `None` si vide.
    pub fn text(&self) -> Option<String> {
        let mut joined = String::new();
        for part in &self.text_parts {
            if let TextPart::Text(t) | TextPart::CellText(t) | TextPart::TableSpace(t) = part {
                joined.push_str(t);
            }
        }
        let trimmed = joined.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// The descendant text read as HTML, trimmed, `None` when empty: a `<br/>`,
    /// the edge of a paragraph-level element and the edge of a table row each
    /// start a line, and a table cell is set off from the one before it by ` | `.
    pub fn html_text(&self) -> Option<String> {
        let mut out = String::new();
        let mut gap = Gap::default();
        for part in &self.text_parts {
            match part {
                TextPart::Break => gap.breaks += 1,
                TextPart::Block => gap.block = true,
                TextPart::Row => gap.row = true,
                TextPart::Cell => gap.cell = true,
                TextPart::TableSpace(_) => {}
                TextPart::Text(t) => gap.push(&mut out, t),
                TextPart::CellText(t) => gap.push(&mut out, &t.replace(['\r', '\n'], " ")),
            }
        }
        let trimmed = out.trim();
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

pub fn node_text(node: Option<&XmlNode>) -> Option<String> {
    node.and_then(XmlNode::text)
}

/// Nom local d'une balise : `ns:Tag` → `Tag`.
fn local_name(qname: &[u8]) -> String {
    let s = String::from_utf8_lossy(qname);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// Lit les attributs d'un élément en `(nom local, valeur unescapée)`.
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

fn push_part(stack: &mut [XmlNode], part: TextPart) {
    for node in stack.iter_mut() {
        node.text_parts.push(part.clone());
    }
}

fn push_text(stack: &mut [XmlNode], txt: String) {
    const CELLS: &[&str] = &["td", "th"];
    const TABLE: &[&str] = &["table", "thead", "tbody", "tfoot", "tr", "td", "th"];
    let is = |tag: &str, set: &[&str]| set.iter().any(|t| tag.eq_ignore_ascii_case(t));
    let part = match stack.iter().rev().find(|n| is(&n.tag, TABLE)) {
        Some(n) if is(&n.tag, CELLS) => TextPart::CellText(txt),
        Some(_) if txt.trim().is_empty() => TextPart::TableSpace(txt),
        _ => TextPart::Text(txt),
    };
    push_part(stack, part);
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
            Ok(Event::Start(e)) => {
                let tag = local_name(e.name().as_ref());
                if let Some(part) = edge_part(&tag) {
                    push_part(&mut stack, part);
                }
                stack.push(XmlNode {
                    tag,
                    attrs: read_attrs(&e),
                    ..Default::default()
                });
            }
            Ok(Event::Empty(e)) => {
                let tag = local_name(e.name().as_ref());
                if let Some(part) = edge_part(&tag) {
                    push_part(&mut stack, part);
                }
                let node = XmlNode {
                    tag,
                    attrs: read_attrs(&e),
                    ..Default::default()
                };
                attach(node, &mut stack, &mut root);
            }
            Ok(Event::Text(e)) => {
                let txt = match e.decode() {
                    Ok(d) => quick_xml::escape::unescape(&d)
                        .map(|u| u.into_owned())
                        .unwrap_or_else(|_| d.into_owned()),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                if !txt.is_empty() {
                    push_text(&mut stack, txt);
                }
            }
            // quick-xml émet les références d'entités (`&amp;`, `&#160;`, `&#39;` …)
            // présentes dans le contenu textuel comme des events `GeneralRef` distincts,
            // hors des `Text` voisins.
            Ok(Event::GeneralRef(e)) => {
                let txt = match e.decode() {
                    Ok(content) => quick_xml::escape::unescape(&format!("&{content};"))
                        .map(|u| u.into_owned())
                        .unwrap_or_else(|_| format!("&{content};")),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                if !txt.is_empty() {
                    push_text(&mut stack, txt);
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    if let Some(part @ (TextPart::Block | TextPart::Row)) = edge_part(&node.tag) {
                        push_part(&mut stack, part);
                    }
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

/// En-tête de métadonnées (XML) : 3 lignes, lignes vides omises.
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
/// `build_metadata_header_xml_from_fields` (depuis `source_fields`) : une seule mise
/// en forme, donc reconstruction garantie identique.
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
/// "Audience": {…} }` — la sortie est identique au parse direct.
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

/// Découpe le texte XML nettoyé en sections. Offsets en **codepoints**.
pub(crate) fn extract_sections_xml(cleaned_text: &str) -> Vec<DecisionSection> {
    sections_from_markers(cleaned_text, &SECTION_PATTERNS, true)
}

pub(crate) fn extract_sections_cnil(cleaned_text: &str) -> Vec<DecisionSection> {
    sections_from_markers(cleaned_text, &CNIL_SECTION_PATTERNS, true)
}

/// The marker of a match: group 1 when the pattern has one, so that a pattern
/// needing the surrounding lines to tell a heading from a table of contents
/// consumes them without carrying them into the boundary.
fn marker<'t>(re: &Regex, text: &'t str) -> Option<regex::Match<'t>> {
    let caps = re.captures(text)?;
    caps.get(1).or_else(|| caps.get(0))
}

/// Splits `cleaned_text` on the first match of each pattern. Offsets in
/// **codepoints**. `motifs_anciens` reads the older French motifs marker when
/// the pattern set found none.
fn sections_from_markers(
    cleaned_text: &str,
    patterns: &[(&'static str, Regex)],
    motifs_anciens: bool,
) -> Vec<DecisionSection> {
    if cleaned_text.is_empty() {
        return Vec::new();
    }

    // (start_char, line_start_char, kind, label).
    let mut markers: Vec<(usize, usize, &'static str, String)> = Vec::new();
    let push = |markers: &mut Vec<_>, at: usize, kind: &'static str, label: &str| {
        let line_start = cleaned_text[..at].rfind('\n').map_or(0, |idx| idx + 1);
        markers.push((
            byte_to_char_index(cleaned_text, at),
            byte_to_char_index(cleaned_text, line_start),
            kind,
            label.trim().to_string(),
        ));
    };
    for (kind, re) in patterns.iter() {
        if let Some(m) = marker(re, cleaned_text) {
            push(&mut markers, m.start(), kind, m.as_str());
        }
    }
    if motifs_anciens && !markers.iter().any(|(_, _, kind, _)| *kind == "motivations") {
        if let Some(m) = marker(&MOTIFS_ANCIENS_RE, cleaned_text) {
            push(&mut markers, m.start(), "motivations", m.as_str());
        }
    }
    if markers.is_empty() {
        return Vec::new();
    }

    markers.sort_by_key(|m| m.0);

    // Dedup: one marker a kind, and never two at the same offset. A
    // boundary falls back to the start of its line, without crossing the previous
    // one: the renderer puts a whole paragraph in the section covering its first
    // character, so a boundary set inside a line opens no section at all. Markers
    // of the recent drafting already open their line, those of the older stock
    // seldom do.
    let mut deduped: Vec<(usize, &'static str, String)> = Vec::new();
    let mut seen_kinds: Vec<&'static str> = Vec::new();
    let mut last_start: Option<usize> = None;
    for (start, line_start, kind, label) in markers {
        let start = match last_start {
            Some(last) if line_start > last => line_start,
            None if line_start > 0 => line_start,
            _ => start,
        };
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

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

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

    let publication_codes = dossier
        .and_then(|d| node_text(d.find_first(&["Code_Publication"])))
        .map(|c| vec![c])
        .unwrap_or_default();

    Decision {
        source_uid,
        member_name: member_name.to_string(),
        // L'XML opendata ne porte pas d'ECLI.
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
        listed_counsel: Vec::new(),
        texte_integral_raw,
        texte_integral_clean,
        sections,
        metadata_header,
        themes: Vec::new(),
        attacked: None,
        parse_warnings: warnings,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Judilibre
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinelle pour `start_char` / `end_char` d'une section synthétique sans
/// offset dans le texte (visa Judilibre). Les offsets sont `usize` : on encode
/// l'absence d'offset par `usize::MAX`.
pub const SECTION_NO_OFFSET: usize = usize::MAX;

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

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

/// (`zone_key`, `kind`, `label`). `chapeau` is no Judilibre zone: carved at
/// ingest, it is stored with the zones and rebuilt as one.
const ZONE_KIND_MAP: &[(&str, &str, &str)] = &[
    ("introduction", "preamble", "Introduction"),
    ("expose", "expose", "Exposé du litige"),
    ("moyens", "moyens", "Moyens"),
    ("motivations", "motivations", "Motivations"),
    ("chapeau", "chapeau", "Chapeau"),
    ("dispositif", "dispositif", "Dispositif"),
];

// ── Helpers char-aware (les offsets Judilibre indexent par codepoint) ────────

/// Nombre de codepoints dans `s`.
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Slice `[start, end)` en indices de **codepoints**.
pub(crate) fn char_slice(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn char_take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Index (codepoint) de la première occurrence de `needle` dans `haystack` à
/// partir du codepoint `from`, ou `None`.
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

/// Position d'un fragment nettoyé dans `clean_text` (recherche monotone à partir
/// de `cursor`, repli sur recherche globale).
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

/// Retire les balises HTML et trim (`None` et chaîne vide traités
/// identiquement).
fn strip_html(value: Option<&str>) -> String {
    match value.filter(|v| !v.is_empty()) {
        None => String::new(),
        Some(v) => TAG_RE.replace_all(v, "").trim().to_string(),
    }
}

/// Index de début du dispositif via marqueurs, ou `None`. Renvoie un index de
/// **codepoint**.
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

/// Vocabulaire Judilibre.
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
        ("cph", "Conseil de prud'hommes"),
        ("constit", "Conseil constitutionnel"),
        ("tc", "Tribunal des conflits"),
        ("cnil", "CNIL"),
        ("cedh", "Cour européenne des droits de l'homme"),
        ("cjue", "Cour de justice de l'Union européenne"),
        ("cnda", "Cour nationale du droit d'asile"),
        // Hors nomenclature Judilibre : porté par les payloads reconstruits du
        // backfill ArianeWeb DCE.
        ("ce", "Conseil d'État"),
    ];

    const JURISDICTION_TYPES: &[(&str, &str)] = &[
        ("cc", "CC"),
        ("ca", "CA"),
        ("tj", "TJ"),
        ("tcom", "TCOM"),
        ("cph", "CPH"),
        ("constit", "CONSTIT"),
        ("tc", "TC"),
        ("cnil", "CNIL"),
        ("cedh", "CEDH"),
        ("cjue", "CJUE"),
        ("cnda", "CNDA"),
        // Hors nomenclature Judilibre (backfill ArianeWeb DCE).
        ("ce", "CE"),
    ];

    const CASSATION_CHAMBER_LABELS: &[(&str, &str)] = &[
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

    /// Libellé de juridiction ; le code brut si inconnu.
    pub fn jurisdiction_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(
            lookup(JURISDICTION_LABELS, code)
                .unwrap_or(code)
                .to_string(),
        )
    }

    /// Type de juridiction ; `None` si le code est inconnu.
    pub fn jurisdiction_type(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        lookup(JURISDICTION_TYPES, code).map(str::to_string)
    }

    /// Libellé de chambre ; le code brut si inconnu.
    pub fn chamber_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(
            lookup(CASSATION_CHAMBER_LABELS, code)
                .unwrap_or(code)
                .to_string(),
        )
    }

    /// Libellé de formation ; le code en majuscules si inconnu.
    pub fn formation_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(
            lookup(FORMATION_LABELS, code)
                .map(str::to_string)
                .unwrap_or_else(|| code.to_uppercase()),
        )
    }

    /// Libellé de type ; le code brut si inconnu.
    pub fn type_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(lookup(TYPE_LABELS, code).unwrap_or(code).to_string())
    }

    /// Libellé de solution ; le code brut si inconnu.
    pub fn solution_label(code: Option<&str>) -> Option<String> {
        let code = code.filter(|c| !c.is_empty())?;
        Some(lookup(SOLUTION_LABELS, code).unwrap_or(code).to_string())
    }
}

// ── Accès JSON ───────────────────────────────────────────────────────────────

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

/// Assemble les sections canoniques depuis `zones` + `visa`. Les offsets
/// `zones` indexent `payload["text"]` brut.
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
    if let Some(anchor) = orphan_motivations_bound(frags.iter().map(|f| (f.start, f.kind.as_str())))
    {
        for f in frags.iter_mut() {
            if f.kind == "motivations" && f.start < anchor {
                f.kind = "preamble".to_string();
                f.label = "Introduction".to_string();
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
                    None => 0,
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

/// Sections of a decision in Judilibre's shape: its zones, completed by the
/// markers of its drafting, which for a Conseil d'État payload are all it has.
fn judilibre_sections(raw_text: &str, text: &str, payload: &Value) -> Vec<DecisionSection> {
    if get_str(payload, "jurisdiction") == Some("ce") {
        return extract_sections_xml(text);
    }
    let sections = build_sections(raw_text, text, payload);
    let sections = augment_dispositif_fallback(sections, text);
    let sections = augment_lead(sections, text, is_cassation(payload));
    augment_annexes(augment_chapeau(sections, text), text)
}

fn is_cassation(payload: &Value) -> bool {
    get_str(payload, "jurisdiction") == Some("cc")
}

/// The first moyens or exposé, ahead of which a motivations fragment belongs to
/// the introduction, when another motivations fragment follows it.
pub(crate) fn orphan_motivations_bound<'a>(
    frags: impl Iterator<Item = (usize, &'a str)> + Clone,
) -> Option<usize> {
    let anchor = frags
        .clone()
        .filter(|(_, k)| *k == "moyens" || *k == "expose")
        .map(|(start, _)| start)
        .min()?;
    let motivations: Vec<usize> = frags
        .filter(|(_, k)| *k == "motivations")
        .map(|(start, _)| start)
        .collect();
    (motivations.len() >= 2 && motivations.iter().any(|&start| start >= anchor)).then_some(anchor)
}

/// Ajoute une section `dispositif` par marqueurs si les zones n'en ont pas.
fn augment_dispositif_fallback(sections: Vec<DecisionSection>, text: &str) -> Vec<DecisionSection> {
    if text.is_empty() || sections.iter().any(|s| s.kind == "dispositif") {
        return sections;
    }
    let bound = lj_core::annexes::start(text).unwrap_or_else(|| char_len(text));
    let Some(found) = find_dispositif_start(&char_slice(text, 0, bound)) else {
        return sections;
    };
    let span = char_slice(text, found, bound);
    let dispositif_text = span.trim().to_string();
    if dispositif_text.is_empty() {
        return sections;
    }
    let start = found + span.chars().take_while(|c| c.is_whitespace()).count();
    let end = start + char_len(&dispositif_text);

    let mut out: Vec<DecisionSection> = Vec::with_capacity(sections.len() + 1);
    for section in sections {
        // `0 <= start_char < start < end_char` — la borne synthétique du visa
        // (SECTION_NO_OFFSET) ne peut pas satisfaire `start_char < start` puisque
        // start ≤ len(text) ; la section est gardée telle quelle.
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
        end_char: end,
        text: dispositif_text,
    });
    out
}

/// Sets the moyens annexés apart: a section crossing their heading ends on it,
/// one starting past it goes, and an `annexes` section runs to the end.
fn augment_annexes(sections: Vec<DecisionSection>, text: &str) -> Vec<DecisionSection> {
    let Some(start) = lj_core::annexes::start(text) else {
        return sections;
    };
    let mut out: Vec<DecisionSection> = Vec::with_capacity(sections.len() + 1);
    for section in sections {
        if section.start_char == SECTION_NO_OFFSET || section.end_char <= start {
            out.push(section);
        } else if section.start_char < start {
            let kept = char_slice(text, section.start_char, start);
            let kept = kept.trim_end();
            if !kept.is_empty() {
                out.push(DecisionSection {
                    end_char: section.start_char + char_len(kept),
                    text: kept.to_string(),
                    ..section
                });
            }
        }
    }
    out.push(DecisionSection {
        label: "Moyens annexés".to_string(),
        kind: "annexes".to_string(),
        start_char: start,
        end_char: char_len(text),
        text: char_slice(text, start, char_len(text)).trim().to_string(),
    });
    out
}

/// Visa of a text, alone on its line, which opens the rule the Court states.
static CHAPEAU_VISA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*Vu\s+(?:l['’]article|les articles|l['’]|le\s|la\s)[^\n]{0,250}:[ \t]*$")
        .unwrap()
});

/// Numbered paragraph of the recent drafting, group 1 on its opening words.
static CHAPEAU_PARA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*\d+\.[ \t]+(.+)").unwrap());

/// The paragraph where the Court leaves the rule for what the attacked decision
/// did. It closes the chapeau, and opening on it means there is none.
static CHAPEAU_GRIEF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:Pour\s|En statuant|En se déterminant|En l['’]état|En procédant|En prononçant|En retenant|En jugeant|En déclarant|En condamnant|En rejetant|En ordonnant|En accueillant|En refusant|En disant|En fixant|En application|Après avoir|Or,|L['’]arrêt\b|Le jugement\b|La cour d['’]appel a\b|Le tribunal.{0,30} a\b|La cassation\b)|fait grief (?:à l['’]arrêt|au jugement)",
    )
    .unwrap()
});

/// Sections of a judicial decision whose source zoned none of its text.
pub(crate) fn unzoned_judicial_sections(text: &str, cassation: bool) -> Vec<DecisionSection> {
    let sections = augment_dispositif_fallback(Vec::new(), text);
    let sections = augment_lead(sections, text, cassation);
    augment_annexes(augment_chapeau(sections, text), text)
}

/// Carves the chapeau out of the motivations. In the recent cassation drafting a
/// `Vu <texte> :` line opens the rule, the numbered paragraphs that follow carry
/// it, and the one turning to the grief closes it. A chapeau no grief closes is
/// left alone rather than run to the end of the motivations.
fn augment_chapeau(sections: Vec<DecisionSection>, text: &str) -> Vec<DecisionSection> {
    if text.is_empty() {
        return sections;
    }
    let mut out: Vec<DecisionSection> = Vec::with_capacity(sections.len());
    for section in sections {
        if section.kind != "motivations" || section.start_char == SECTION_NO_OFFSET {
            out.push(section);
            continue;
        }
        out.extend(split_chapeau(section));
    }
    out
}

/// Splits one `motivations` section on the chapeaux it carries. Offsets in
/// **codepoints**, rebased on the section's own start.
fn split_chapeau(section: DecisionSection) -> Vec<DecisionSection> {
    let body = section.text.as_str();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for visa in CHAPEAU_VISA_RE.find_iter(body) {
        let mut paragraphs = 0usize;
        let mut close = None;
        let mut at = visa.end();
        for line in body[visa.end()..].split_inclusive('\n') {
            let start = at;
            at += line.len();
            if line.trim().is_empty() {
                continue;
            }
            // A heading closes the chapeau as surely as the grief does. A source
            // line wrapped mid-sentence opens in lower case, and closes nothing.
            let Some(head) = CHAPEAU_PARA_RE
                .captures(line)
                .map(|caps| caps.get(1).expect("groupe 1").as_str())
            else {
                if line.trim_start().starts_with(char::is_lowercase) {
                    continue;
                }
                close = Some(start);
                break;
            };

            let head: String = head.chars().take(200).collect();
            if CHAPEAU_GRIEF_RE.is_match(&head) {
                close = Some(start);
                break;
            }
            paragraphs += 1;
        }
        if let (Some(close), 1..) = (close, paragraphs) {
            cuts.push((visa.start(), close));
        }
    }
    if cuts.is_empty() {
        return vec![section];
    }

    let mut out: Vec<DecisionSection> = Vec::with_capacity(cuts.len() * 2 + 1);
    // Bounded on the trimmed text, as `build_sections` bounds a stored zone.
    let mut push = |start: usize, end: usize, chapeau: bool| {
        let piece = &body[start..end];
        let text = piece.trim();
        if text.is_empty() {
            return;
        }
        let (kind, label) = if chapeau {
            ("chapeau", "Chapeau")
        } else {
            ("motivations", section.label.as_str())
        };
        let start_char = section.start_char
            + byte_to_char_index(body, start + piece.len() - piece.trim_start().len());
        out.push(DecisionSection {
            label: label.to_string(),
            kind: kind.to_string(),
            start_char,
            end_char: start_char + char_len(text),
            text: text.to_string(),
        });
    };
    let mut cursor = 0usize;
    for (start, end) in cuts {
        push(cursor, start, false);
        push(start, end, true);
        cursor = end;
    }
    push(cursor, body.len(), false);
    out
}

/// Header 3 lignes.
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

/// Thèmes Judilibre (`payload["themes"]`) : liste verbatim (matière → chaîne de
/// mots-clés), entrées vides écartées.
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
    let sections = judilibre_sections(raw_text, &text, payload);

    // `numbers` : liste propre, dédoublonnée en gardant l'ordre — le scalaire
    // `number` de la source n'est pas fiable.
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
        listed_counsel: Vec::new(),
        texte_integral_raw: raw_text.to_string(),
        texte_integral_clean: text,
        sections,
        metadata_header,
        themes: parse_themes(payload),
        attacked: None,
        parse_warnings: Vec::new(),
    }
}

/// Construit `source_fields` depuis le payload JSON source et les sections déjà
/// rebasées sur le texte nettoyé (`decision.sections`).
///
/// Décomposition quasi-bijective `source_payload ⟷ (full_text, source_fields)` :
/// on retire `text` (stocké à part dans `full_text`) et on remplace `zones`
/// (offsets sur le texte brut) par `sections` — `[{kind, start, end}]`, offsets
/// rebasés sur `full_text` (codepoints du texte nettoyé). C'est l'unique
/// transformation ; le reste du payload (`visa`, `themes`, `summary`, `nac`,
/// `ecli`…) est conservé verbatim.
///
/// Les sections synthétiques sans offset dans le texte (visa Judilibre,
/// `SECTION_NO_OFFSET`) sont **exclues** : leur texte n'est pas une tranche de
/// `full_text` (refs HTML), elles se reconstruisent au rendu depuis
/// `source_fields["visa"]`.
///
/// The moyens annexés are left out too: they are carved from their heading at
/// every read.
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
        .filter(|s| ZONE_KIND_MAP.iter().any(|(_, kind, _)| *kind == s.kind))
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
/// nœud `<Dossier>`/`<Audience>`, clés qu'un payload Judilibre JSON n'a jamais.
/// Discriminant interne du dispatch [`Decision::from_source_fields`].
fn source_fields_is_xml(source_fields: &Value) -> bool {
    source_fields.get("Dossier").is_some() || source_fields.get("Audience").is_some()
}

/// Rebuilds a [`Decision`] from what storage keeps of it, `(full_text, source_fields)`.
pub trait FromSourceFields {
    fn from_source_fields(full_text: &str, source_fields: &Value, source_uid: &str) -> Self;
}

impl FromSourceFields for Decision {
    /// The canonical decision of `(full_text, source_fields)`, counterpart of the
    /// `build_source_fields*`, and the one path the ingest, the re-extraction
    /// and the display read a decision through. A French text in capitals is read
    /// recased, which is the text the ingest stores. Dispatches on the `source_uid`
    /// prefix for the scraped fonds (`cedh/`, `cjue/`, `cnda/`, `ddd/`, `ddd-jurisprudence/`, `amf/`, `adlc/`, `comeu/`, `consiliaweb/`, `juricaf/`, `entscheidsuche/`, `legalis/`, `legimonaco/`, `juportal/`, `rechtspraak/`, `justice-lu/`, `juradm-site/`, `scc-csc/`, `cce/`, `opencaselaw/`, `a2aj/`, a court site's source such as `cc-bj/`) and the contributions, then on
    /// the shape of `source_fields`: `Dossier`/`Audience` for the opendata XML,
    /// `META_COMMUN` and `META_JURI` for the DILA bulk, the Judilibre JSON otherwise.
    fn from_source_fields(full_text: &str, source_fields: &Value, source_uid: &str) -> Decision {
        let french = source_fields
            .get("lang")
            .and_then(Value::as_str)
            .is_none_or(|code| code.eq_ignore_ascii_case("fra"));
        let recased = french
            .then(|| lj_core::truecase::recased_text(full_text))
            .flatten();
        let full_text = recased.as_deref().unwrap_or(full_text);
        if source_uid_is_html_europe(source_uid) {
            european::from_source_fields_html_europe(full_text, source_fields, source_uid)
        } else if source_uid_is_cnda(source_uid) {
            cnda::from_source_fields_cnda(full_text, source_fields, source_uid)
        } else if ddd::source_uid_is_ddd(source_uid) {
            ddd::from_source_fields_ddd(full_text, source_fields, source_uid)
        } else if ddd::source_uid_is_ddd_jurisprudence(source_uid) {
            ddd::from_source_fields_ddd_jurisprudence(full_text, source_fields, source_uid)
        } else if amf::source_uid_is_amf(source_uid) {
            amf::from_source_fields_amf(full_text, source_fields, source_uid)
        } else if adlc::source_uid_is_adlc(source_uid) {
            adlc::from_source_fields_adlc(full_text, source_fields, source_uid)
        } else if comeu::source_uid_is_comeu(source_uid) {
            comeu::from_source_fields_comeu(full_text, source_fields, source_uid)
        } else if consiliaweb::source_uid_is_consiliaweb(source_uid) {
            consiliaweb::from_source_fields_consiliaweb(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("juricaf/") {
            juricaf::from_source_fields_juricaf(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("entscheidsuche/") {
            entscheidsuche::from_source_fields_entscheidsuche(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("legalis/") {
            legalis::from_source_fields_legalis(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("legimonaco/") {
            legimonaco::from_source_fields_legimonaco(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("juportal/") {
            juportal::from_source_fields_juportal(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("rechtspraak/") {
            rechtspraak::from_source_fields_rechtspraak(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("justice-lu/") {
            justice_lu::from_source_fields_justice_lu(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("juradm-site/") {
            juradm_site::from_source_fields_juradm_site(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("scc-csc/") {
            scc_csc::from_source_fields_scc(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("cce/") {
            cce::from_source_fields_cce(full_text, source_fields, source_uid)
        } else if court_sites::site_court_of_uid(source_uid).is_some() {
            court_sites::from_source_fields_site(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("opencaselaw/") {
            opencaselaw::from_source_fields_opencaselaw(full_text, source_fields, source_uid)
        } else if source_uid.starts_with("a2aj/") {
            a2aj::from_source_fields_a2aj(full_text, source_fields, source_uid)
        } else if source_uid_is_contribution(source_uid) {
            contribution::from_source_fields_contribution(full_text, source_fields, source_uid)
        } else if source_fields_is_xml(source_fields) {
            from_source_fields_xml(full_text, source_fields, source_uid)
        } else if source_fields_is_dila(source_fields) {
            dila::from_source_fields_dila(full_text, source_fields, source_uid)
        } else {
            from_source_fields_json(full_text, source_fields, source_uid)
        }
    }
}

/// Opendata XML branch of [`Decision::from_source_fields`]. Maps the
/// `<Dossier>`/`<Audience>` scalars (kept verbatim by
/// [`build_source_fields_xml`]) onto `Decision` as [`parse_xml`] does, and
/// recomputes the sections and `metadata_header` on `full_text` (offsets
/// recomputed, no rebasing). `raw == clean == full_text`; `date_mise_jour` is
/// not carried in `source_fields` (hence `None`).
fn from_source_fields_xml(full_text: &str, source_fields: &Value, source_uid: &str) -> Decision {
    let restored = lj_core::lettered_par::restored_par(full_text);
    let full_text = restored.as_deref().unwrap_or(full_text);
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

    Decision {
        source_uid: source_uid.to_string(),
        member_name: source_uid.to_string(),
        // Opendata XML carries no ECLI.
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
        listed_counsel: Vec::new(),
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections,
        metadata_header: build_metadata_header_xml_from_fields(source_fields),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    }
}

/// Judilibre JSON branch of [`Decision::from_source_fields`]. Reproduces the
/// direct parse exactly: rebuilds the `zones` from `source_fields["sections"]`
/// (inverse of [`ZONE_KIND_MAP`]) then applies [`build_sections`] and the
/// fallbacks on `(full_text, payload-like)`. The synthetic
/// visa is rebuilt from `source_fields["visa"]` (kept verbatim).
/// `raw == clean == full_text`.
fn from_source_fields_json(full_text: &str, source_fields: &Value, source_uid: &str) -> Decision {
    let payload = json_payload_from_source_fields(full_text, source_fields);

    let sections = judilibre_sections(full_text, full_text, &payload);

    // `numbers`: a clean list, deduplicated in order; the source's `number`
    // scalar is not reliable.
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
        listed_counsel: Vec::new(),
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections,
        metadata_header,
        themes: parse_themes(&payload),
        attacked: parse_attacked(&payload),
        parse_warnings: Vec::new(),
    }
}

/// `payload["contested"]` Judilibre -> [`lj_core::decision::AttackedRef`] : matiere
/// brute du lien de chronologie. `None` si le champ est absent, nul, ou vide de
/// tout discriminant.
fn parse_attacked(payload: &Value) -> Option<lj_core::decision::AttackedRef> {
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
    let r = lj_core::decision::AttackedRef {
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

/// Formatting tags inside a field's text: they carry no metadata, so a node
/// holding only these reads as its text.
const INLINE_TAGS: &[&str] = &[
    "br", "p", "i", "b", "u", "em", "strong", "sup", "sub", "span", "a", "BR", "P", "I", "B", "U",
    "EM", "STRONG", "SUP", "SUB", "SPAN", "A",
];

/// A whole XML subtree as JSON, nothing dropped but empty values: a leaf is its
/// text, a node is an object of its children (a repeated tag becomes an
/// array), a non-empty attribute is `@name`, and the text beside attributes is
/// `#text`. `None` when the subtree carries nothing.
pub(crate) fn xml_tree_json(node: &XmlNode) -> Option<Value> {
    let attrs = node.attrs.iter().filter(|(_, v)| !v.trim().is_empty());
    let structural = node
        .children
        .iter()
        .any(|c| !INLINE_TAGS.contains(&c.tag.as_str()));
    let mut obj = serde_json::Map::new();
    for (k, v) in attrs {
        obj.insert(format!("@{k}"), Value::String(v.trim().to_string()));
    }
    if !structural {
        let text = node.text();
        if obj.is_empty() {
            return text.map(Value::String);
        }
        if let Some(t) = text {
            obj.insert("#text".to_string(), Value::String(t));
        }
        return Some(Value::Object(obj));
    }
    for child in &node.children {
        if let Some(v) = xml_tree_json(child) {
            push_json_child(&mut obj, &child.tag, v);
        }
    }
    (!obj.is_empty()).then_some(Value::Object(obj))
}

/// Adds `v` under `tag`, a second occurrence turning the value into an array.
pub(crate) fn push_json_child(obj: &mut serde_json::Map<String, Value>, tag: &str, v: Value) {
    match obj.get_mut(tag) {
        None => {
            obj.insert(tag.to_string(), v);
        }
        Some(Value::Array(items)) => items.push(v),
        Some(first) => {
            let first = first.take();
            obj.insert(tag.to_string(), Value::Array(vec![first, v]));
        }
    }
}

/// The text a [`xml_tree_json`] value holds: the string itself, the first of a
/// repeated tag, or the `#text` beside attributes.
pub(crate) fn json_text(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        Value::Array(items) => items.first().and_then(json_text),
        Value::Object(o) => o.get("#text").and_then(Value::as_str),
        _ => None,
    }
}

/// Every text a [`xml_tree_json`] value holds under one tag, in document order.
pub(crate) fn json_texts(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(json_text)
            .map(str::to_string)
            .collect(),
        Some(v) => json_text(v).map(str::to_string).into_iter().collect(),
        None => Vec::new(),
    }
}

/// `source_fields` pour un payload XML opendata : tous les scalaires (feuilles)
/// des nœuds `<Dossier>` et `<Audience>`, groupés `{ "Dossier": {…},
/// "Audience": {…} }`.
///
/// The text goes to `full_text`, from which `extract_sections_xml` computes the
/// sections again, so nothing is rebased here. `metadata_header` reads these
/// scalars alone, and every one of them is kept, not only the 6 read today: the
/// XML payload stays rebuildable but for its text.
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
