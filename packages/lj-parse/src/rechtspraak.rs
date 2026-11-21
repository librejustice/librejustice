//! A Rechtspraak open data document (`open-rechtspraak` XML) into a [`Decision`] and its
//! `source_fields`, and the ECLIs a search answer lists.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

use crate::consolidated::{Child, Lines, Tree};

/// A Dutch court the load takes, by the court segment of its ECLIs.
#[derive(Debug)]
pub struct DutchCourt {
    pub ecli: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
    /// Its name in Dutch.
    pub name: &'static str,
}

pub const DUTCH_PLACE: &str = "geo:NL";

const fn court(
    ecli: &'static str,
    jurisdiction_type: &'static str,
    code: &'static str,
    label: &'static str,
    name: &'static str,
) -> DutchCourt {
    DutchCourt {
        ecli,
        jurisdiction_type,
        code,
        label,
        name,
    }
}

const ADMIN: &str = "COUR_ADMINISTRATIVE";
const APPEL: &str = "COUR_APPEL";
const INSTANCE: &str = "TRIBUNAL_PREMIERE_INSTANCE";

/// The courts of the European part of the Kingdom whose decisions Rechtspraak publishes. A court
/// renamed in 2013 keeps one code under both its segments.
pub const DUTCH_COURTS: [DutchCourt; 45] = [
    court(
        "HR",
        "COUR_SUPREME",
        "nl_cour_supreme",
        "Cour suprême (Pays-Bas)",
        "Hoge Raad",
    ),
    court(
        "RVS",
        ADMIN,
        "nl_cour_administrative_raad_van_state",
        "Conseil d'État (Pays-Bas)",
        "Raad van State",
    ),
    court(
        "CRVB",
        ADMIN,
        "nl_cour_administrative_centrale_raad_van_beroep",
        "Conseil central d'appel (Pays-Bas)",
        "Centrale Raad van Beroep",
    ),
    court(
        "CBB",
        ADMIN,
        "nl_cour_administrative_college_beroep_bedrijfsleven",
        "Collège d'appel pour l'économie (Pays-Bas)",
        "College van Beroep voor het bedrijfsleven",
    ),
    court(
        "GHAMS",
        APPEL,
        "nl_cour_appel_amsterdam",
        "Cour d'appel d'Amsterdam (Pays-Bas)",
        "Gerechtshof Amsterdam",
    ),
    court(
        "GHARL",
        APPEL,
        "nl_cour_appel_arnhem_leeuwarden",
        "Cour d'appel d'Arnhem-Leeuwarden (Pays-Bas)",
        "Gerechtshof Arnhem-Leeuwarden",
    ),
    court(
        "GHDHA",
        APPEL,
        "nl_cour_appel_den_haag",
        "Cour d'appel de La Haye (Pays-Bas)",
        "Gerechtshof Den Haag",
    ),
    court(
        "GHSGR",
        APPEL,
        "nl_cour_appel_den_haag",
        "Cour d'appel de La Haye (Pays-Bas)",
        "Gerechtshof Den Haag",
    ),
    court(
        "GHSHE",
        APPEL,
        "nl_cour_appel_s_hertogenbosch",
        "Cour d'appel de Bois-le-Duc (Pays-Bas)",
        "Gerechtshof 's-Hertogenbosch",
    ),
    court(
        "GHARN",
        APPEL,
        "nl_cour_appel_arnhem",
        "Cour d'appel d'Arnhem (Pays-Bas)",
        "Gerechtshof Arnhem",
    ),
    court(
        "GHLEE",
        APPEL,
        "nl_cour_appel_leeuwarden",
        "Cour d'appel de Leeuwarden (Pays-Bas)",
        "Gerechtshof Leeuwarden",
    ),
    court(
        "RBAMS",
        INSTANCE,
        "nl_tribunal_premiere_instance_amsterdam",
        "Tribunal d'Amsterdam (Pays-Bas)",
        "Rechtbank Amsterdam",
    ),
    court(
        "RBDHA",
        INSTANCE,
        "nl_tribunal_premiere_instance_den_haag",
        "Tribunal de La Haye (Pays-Bas)",
        "Rechtbank Den Haag",
    ),
    court(
        "RBSGR",
        INSTANCE,
        "nl_tribunal_premiere_instance_den_haag",
        "Tribunal de La Haye (Pays-Bas)",
        "Rechtbank Den Haag",
    ),
    court(
        "RBGEL",
        INSTANCE,
        "nl_tribunal_premiere_instance_gelderland",
        "Tribunal de Gueldre (Pays-Bas)",
        "Rechtbank Gelderland",
    ),
    court(
        "RBLIM",
        INSTANCE,
        "nl_tribunal_premiere_instance_limburg",
        "Tribunal du Limbourg (Pays-Bas)",
        "Rechtbank Limburg",
    ),
    court(
        "RBMNE",
        INSTANCE,
        "nl_tribunal_premiere_instance_midden_nederland",
        "Tribunal des Pays-Bas centraux (Pays-Bas)",
        "Rechtbank Midden-Nederland",
    ),
    court(
        "RBNHO",
        INSTANCE,
        "nl_tribunal_premiere_instance_noord_holland",
        "Tribunal de Hollande-Septentrionale (Pays-Bas)",
        "Rechtbank Noord-Holland",
    ),
    court(
        "RBNNE",
        INSTANCE,
        "nl_tribunal_premiere_instance_noord_nederland",
        "Tribunal du Nord des Pays-Bas (Pays-Bas)",
        "Rechtbank Noord-Nederland",
    ),
    court(
        "RBOBR",
        INSTANCE,
        "nl_tribunal_premiere_instance_oost_brabant",
        "Tribunal du Brabant oriental (Pays-Bas)",
        "Rechtbank Oost-Brabant",
    ),
    court(
        "RBOVE",
        INSTANCE,
        "nl_tribunal_premiere_instance_overijssel",
        "Tribunal d'Overijssel (Pays-Bas)",
        "Rechtbank Overijssel",
    ),
    court(
        "RBROT",
        INSTANCE,
        "nl_tribunal_premiere_instance_rotterdam",
        "Tribunal de Rotterdam (Pays-Bas)",
        "Rechtbank Rotterdam",
    ),
    court(
        "RBZWB",
        INSTANCE,
        "nl_tribunal_premiere_instance_zeeland_west_brabant",
        "Tribunal de Zélande-Brabant occidental (Pays-Bas)",
        "Rechtbank Zeeland-West-Brabant",
    ),
    court(
        "RBONE",
        INSTANCE,
        "nl_tribunal_premiere_instance_oost_nederland",
        "Tribunal de l'Est des Pays-Bas (Pays-Bas)",
        "Rechtbank Oost-Nederland",
    ),
    court(
        "RBSHE",
        INSTANCE,
        "nl_tribunal_premiere_instance_s_hertogenbosch",
        "Tribunal de Bois-le-Duc (Pays-Bas)",
        "Rechtbank 's-Hertogenbosch",
    ),
    court(
        "RBALK",
        INSTANCE,
        "nl_tribunal_premiere_instance_alkmaar",
        "Tribunal d'Alkmaar (Pays-Bas)",
        "Rechtbank Alkmaar",
    ),
    court(
        "RBALM",
        INSTANCE,
        "nl_tribunal_premiere_instance_almelo",
        "Tribunal d'Almelo (Pays-Bas)",
        "Rechtbank Almelo",
    ),
    court(
        "RBARN",
        INSTANCE,
        "nl_tribunal_premiere_instance_arnhem",
        "Tribunal d'Arnhem (Pays-Bas)",
        "Rechtbank Arnhem",
    ),
    court(
        "RBASS",
        INSTANCE,
        "nl_tribunal_premiere_instance_assen",
        "Tribunal d'Assen (Pays-Bas)",
        "Rechtbank Assen",
    ),
    court(
        "RBBRE",
        INSTANCE,
        "nl_tribunal_premiere_instance_breda",
        "Tribunal de Breda (Pays-Bas)",
        "Rechtbank Breda",
    ),
    court(
        "RBDOR",
        INSTANCE,
        "nl_tribunal_premiere_instance_dordrecht",
        "Tribunal de Dordrecht (Pays-Bas)",
        "Rechtbank Dordrecht",
    ),
    court(
        "RBGRO",
        INSTANCE,
        "nl_tribunal_premiere_instance_groningen",
        "Tribunal de Groningue (Pays-Bas)",
        "Rechtbank Groningen",
    ),
    court(
        "RBHAA",
        INSTANCE,
        "nl_tribunal_premiere_instance_haarlem",
        "Tribunal de Haarlem (Pays-Bas)",
        "Rechtbank Haarlem",
    ),
    court(
        "RBLEE",
        INSTANCE,
        "nl_tribunal_premiere_instance_leeuwarden",
        "Tribunal de Leeuwarden (Pays-Bas)",
        "Rechtbank Leeuwarden",
    ),
    court(
        "RBMAA",
        INSTANCE,
        "nl_tribunal_premiere_instance_maastricht",
        "Tribunal de Maastricht (Pays-Bas)",
        "Rechtbank Maastricht",
    ),
    court(
        "RBMID",
        INSTANCE,
        "nl_tribunal_premiere_instance_middelburg",
        "Tribunal de Middelbourg (Pays-Bas)",
        "Rechtbank Middelburg",
    ),
    court(
        "RBROE",
        INSTANCE,
        "nl_tribunal_premiere_instance_roermond",
        "Tribunal de Ruremonde (Pays-Bas)",
        "Rechtbank Roermond",
    ),
    court(
        "RBUTR",
        INSTANCE,
        "nl_tribunal_premiere_instance_utrecht",
        "Tribunal d'Utrecht (Pays-Bas)",
        "Rechtbank Utrecht",
    ),
    court(
        "RBZUT",
        INSTANCE,
        "nl_tribunal_premiere_instance_zutphen",
        "Tribunal de Zutphen (Pays-Bas)",
        "Rechtbank Zutphen",
    ),
    court(
        "RBZWO",
        INSTANCE,
        "nl_tribunal_premiere_instance_zwolle",
        "Tribunal de Zwolle (Pays-Bas)",
        "Rechtbank Zwolle",
    ),
    court(
        "RBZLY",
        INSTANCE,
        "nl_tribunal_premiere_instance_zwolle_lelystad",
        "Tribunal de Zwolle-Lelystad (Pays-Bas)",
        "Rechtbank Zwolle-Lelystad",
    ),
    court(
        "RBHRV",
        INSTANCE,
        "nl_tribunal_premiere_instance_heerenveen",
        "Tribunal de Heerenveen (Pays-Bas)",
        "Rechtbank Heerenveen",
    ),
    court(
        "RBTIE",
        INSTANCE,
        "nl_tribunal_premiere_instance_tiel",
        "Tribunal de Tiel (Pays-Bas)",
        "Rechtbank Tiel",
    ),
    court(
        "RBWIN",
        INSTANCE,
        "nl_tribunal_premiere_instance_winschoten",
        "Tribunal de Winschoten (Pays-Bas)",
        "Rechtbank Winschoten",
    ),
    court(
        "RBZIE",
        INSTANCE,
        "nl_tribunal_premiere_instance_zierikzee",
        "Tribunal de Zierikzee (Pays-Bas)",
        "Rechtbank Zierikzee",
    ),
];

pub fn dutch_court(ecli_court: &str) -> Option<&'static DutchCourt> {
    DUTCH_COURTS.iter().find(|c| c.ecli == ecli_court)
}

/// Each court's Dutch name, once per code.
pub fn rechtspraak_names() -> Vec<(&'static str, Value)> {
    let mut names: Vec<(&'static str, Value)> = Vec::new();
    for court in &DUTCH_COURTS {
        if !names.iter().any(|(code, _)| *code == court.code) {
            names.push((
                court.code,
                json!({ "nl": { "name": format!("{} (Nederland)", court.name) } }),
            ));
        }
    }
    names
}

/// The court segment of a Dutch ECLI: `ECLI:NL:HR:2024:1000` is `HR`.
pub fn rechtspraak_court_segment(ecli: &str) -> Option<&str> {
    let mut parts = ecli.strip_prefix("ECLI:NL:")?.split(':');
    let court = parts.next()?;
    let year = parts.next()?;
    let number = parts.next()?;
    (!court.is_empty() && year.len() == 4 && !number.is_empty() && parts.next().is_none())
        .then_some(court)
}

/// An ECLI of a court of [`DUTCH_COURTS`].
pub fn rechtspraak_taken(ecli: &str) -> bool {
    rechtspraak_court_segment(ecli).is_some_and(|court| dutch_court(court).is_some())
}

/// What a search answer lists: the count it found, then each ECLI with the time, UTC, its document
/// or metadata last changed.
#[derive(Debug)]
pub struct RechtspraakListing {
    pub total: usize,
    pub entries: Vec<(String, String)>,
}

fn fail(why: String) -> CoreError {
    CoreError::Xml(format!("rechtspraak: {why}"))
}

/// The children of `idx` whose local name is `tag`, whatever their namespace prefix.
fn kids<'a>(tree: &'a Tree, idx: usize, tag: &'a str) -> impl Iterator<Item = usize> + 'a {
    tree.nodes[idx]
        .children
        .iter()
        .filter_map(move |c| match c {
            Child::Node(i) if tree.nodes[*i].tag == tag => Some(*i),
            _ => None,
        })
}

fn kid(tree: &Tree, idx: usize, tag: &str) -> Option<usize> {
    kids(tree, idx, tag).next()
}

/// The text under `idx`, whitespace folded.
fn text(tree: &Tree, idx: usize) -> String {
    let mut out = String::new();
    gather(tree, idx, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn gather(tree: &Tree, idx: usize, out: &mut String) {
    for c in &tree.nodes[idx].children {
        match c {
            Child::Text(t) => out.push_str(t),
            Child::Node(i) => gather(tree, *i, out),
        }
    }
}

/// Parses a search answer of `/uitspraken/zoeken`.
pub fn rechtspraak_listing(xml: &str) -> Result<RechtspraakListing> {
    let tree = Tree::parse(xml).map_err(|e| fail(format!("listing: {e}")))?;
    let feed = kid(&tree, 0, "feed").ok_or_else(|| fail("listing: no Atom feed".into()))?;
    let subtitle = kid(&tree, feed, "subtitle")
        .map(|i| text(&tree, i))
        .unwrap_or_default();
    let total = subtitle
        .rsplit(':')
        .next()
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| fail(format!("listing: no count in {subtitle:?}")))?;
    let mut entries = Vec::new();
    for entry in kids(&tree, feed, "entry") {
        let id = kid(&tree, entry, "id").map(|i| text(&tree, i));
        let updated = kid(&tree, entry, "updated").map(|i| text(&tree, i));
        match (id, updated) {
            (Some(id), Some(updated)) if id.starts_with("ECLI:") => entries.push((id, updated)),
            _ => {
                return Err(fail(
                    "listing: an entry without its ECLI or its time".into(),
                ))
            }
        }
    }
    Ok(RechtspraakListing { total, entries })
}

#[derive(Debug)]
pub struct RechtspraakParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static DutchCourt,
}

/// The non-blank characters a decision text holds at least.
const DECISION_FLOOR: usize = 200;

/// The case numbers `zaaknummer` values hold, `C/09/669557 / FA RK 24-5090` holding two and
/// `NL25.19118 en NL25.19119` two, a range by its two ends.
pub fn rechtspraak_dockets(zaaknummers: &[String]) -> Vec<String> {
    const SEPARATORS: [&str; 7] = [" en ", ", ", ";", " + ", " / ", " \\ ", " t/m "];
    let mut dockets: Vec<String> = Vec::new();
    for raw in zaaknummers {
        let mut parts = vec![raw.split_whitespace().collect::<Vec<_>>().join(" ")];
        for sep in SEPARATORS {
            parts = parts
                .iter()
                .flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>())
                .collect();
        }
        for part in parts {
            let part = part.trim();
            let part = ["Parketnummer:", "Parketnr:"]
                .iter()
                .find_map(|prefix| part.strip_prefix(prefix))
                .unwrap_or(part)
                .trim();
            if !part.is_empty() && !dockets.iter().any(|d| d == part) {
                dockets.push(part.to_string());
            }
        }
    }
    dockets
}

/// `nld` or `eng`, whichever's function words the text holds more of.
fn language(text: &str) -> &'static str {
    const DUTCH: [&str; 10] = [
        "de", "het", "een", "van", "dat", "niet", "voor", "zijn", "wordt", "deze",
    ];
    const ENGLISH: [&str; 10] = [
        "the", "of", "and", "that", "not", "for", "this", "which", "with", "has",
    ];
    let (mut dutch, mut english) = (0usize, 0usize);
    for word in text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .take(5_000)
    {
        let word = word.to_lowercase();
        if DUTCH.contains(&word.as_str()) {
            dutch += 1;
        } else if ENGLISH.contains(&word.as_str()) {
            english += 1;
        }
    }
    if english > dutch {
        "eng"
    } else {
        "nld"
    }
}

/// A body one block per line: a number heads the line of the block it numbers, a table cell is a
/// line, a footnote call is its label in brackets, and the footnotes close the text.
fn body_lines(tree: &Tree, idx: usize) -> String {
    let mut labels = HashMap::new();
    for (i, node) in tree.nodes.iter().enumerate() {
        if node.tag == "footnote" {
            if let (Some(id), Some(label)) = (tree.attr(i, "id"), tree.attr(i, "label")) {
                labels.insert(id.to_string(), label.to_string());
            }
        }
    }
    let mut lines = Lines::new(|line| line);
    lay_out(tree, idx, &labels, &mut lines);
    lines.finish().join("\n")
}

fn lay_out(tree: &Tree, idx: usize, labels: &HashMap<String, String>, lines: &mut Lines) {
    for c in &tree.nodes[idx].children {
        let i = match c {
            Child::Text(t) => {
                lines.push(t);
                continue;
            }
            Child::Node(i) => *i,
        };
        match tree.nodes[i].tag.as_str() {
            "nr" => lines.mark(&text(tree, i)),
            "footnote-ref" => {
                if let Some(label) = tree.attr(i, "linkend").and_then(|id| labels.get(id)) {
                    lines.glue();
                    lines.push(&format!("[{label}]"));
                }
            }
            "footnote" => {
                lines.line_break();
                if let Some(label) = tree.attr(i, "label") {
                    lines.mark(&format!("[{label}]"));
                }
                lay_out(tree, i, labels, lines);
                lines.line_break();
            }
            "inlinemediaobject" | "imageobject" | "imagedata" | "colspec" => {}
            "para" | "title" | "bridgehead" | "entry" | "listitem" => {
                lay_out(tree, i, labels, lines);
                lines.line_break();
            }
            _ => lay_out(tree, i, labels, lines),
        }
    }
}

/// The document's own description: the RDF block's `Description` with no `about`.
fn description(tree: &Tree, root: usize) -> Option<usize> {
    let rdf = kid(tree, root, "RDF")?;
    kids(tree, rdf, "Description").find(|&d| tree.attr(d, "about").is_none())
}

/// The title of the document's web rendering, `ECLI:NL:HR:2024:1000 Hoge Raad , 02-07-2024 /
/// 22/01514`.
fn web_title(tree: &Tree, root: usize) -> Option<String> {
    let rdf = kid(tree, root, "RDF")?;
    kids(tree, rdf, "Description")
        .filter_map(|d| kid(tree, d, "title"))
        .map(|t| text(tree, t))
        .find(|t| !t.is_empty())
}

fn decision_of(court: &DutchCourt, source_fields: &Value, text: String) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let ecli = field("ecli").expect("a rechtspraak decision states its ECLI");
    let dockets: Vec<String> = source_fields
        .get("dockets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Decision {
        source_uid: format!("rechtspraak/{ecli}"),
        member_name: ecli.to_string(),
        ecli: Some(ecli.to_string()),
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: dockets.first().cloned(),
        numero_dossiers: (!dockets.is_empty()).then_some(dockets),
        numero_role: None,
        date_lecture: field("date").map(str::to_string),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

/// Parses a document of `/uitspraken/content`. A conclusion, a document of a court the load does
/// not take, and one without a decision text are refused.
pub fn parse_rechtspraak(xml: &str) -> Result<RechtspraakParsed> {
    let tree = Tree::parse(xml).map_err(|e| fail(format!("document: {e}")))?;
    let root =
        kid(&tree, 0, "open-rechtspraak").ok_or_else(|| fail("no open-rechtspraak root".into()))?;
    let desc = description(&tree, root).ok_or_else(|| fail("no RDF description".into()))?;
    let one = |tag: &str| {
        kid(&tree, desc, tag)
            .map(|i| text(&tree, i))
            .filter(|s| !s.is_empty())
    };
    let all = |tag: &str| -> Vec<String> {
        kids(&tree, desc, tag)
            .map(|i| text(&tree, i))
            .filter(|s| !s.is_empty())
            .collect()
    };
    let ecli = one("identifier").ok_or_else(|| fail("no identifier".into()))?;
    let segment =
        rechtspraak_court_segment(&ecli).ok_or_else(|| fail(format!("ECLI {ecli} unread")))?;
    let court =
        dutch_court(segment).ok_or_else(|| fail(format!("{ecli}: court {segment} not taken")))?;
    let kind = one("type").unwrap_or_default();
    if kind != "Uitspraak" {
        return Err(fail(format!("{ecli}: a {kind:?}, not a ruling")));
    }
    let date = one("date").ok_or_else(|| fail(format!("{ecli}: no decision date")))?;
    if date.parse::<jiff::civil::Date>().is_err() {
        return Err(fail(format!("{ecli}: decision date {date:?}")));
    }
    let body = kid(&tree, root, "uitspraak").ok_or_else(|| fail(format!("{ecli}: no text")))?;
    let body_text = body_lines(&tree, body);
    if body_text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(fail(format!("{ecli}: no decision text")));
    }
    let abstract_ = kid(&tree, root, "inhoudsindicatie")
        .map(|i| body_lines(&tree, i))
        .filter(|a| a.chars().any(char::is_alphanumeric));
    let labelled = |tag: &str| -> Vec<Value> {
        kids(&tree, desc, tag)
            .map(|i| {
                json!({
                    "id": tree.attr(i, "resourceIdentifier"),
                    "label": text(&tree, i),
                })
            })
            .collect()
    };
    let relations: Vec<Value> = kids(&tree, desc, "relation")
        .map(|i| {
            json!({
                "ecli": tree.attr(i, "resourceIdentifier"),
                "type": tree.attr(i, "type"),
                "aanleg": tree.attr(i, "aanleg"),
                "gevolg": tree.attr(i, "gevolg"),
                "label": text(&tree, i),
            })
        })
        .collect();
    let vindplaatsen: Vec<String> = kids(&tree, desc, "hasVersion")
        .flat_map(|v| kids(&tree, v, "list").collect::<Vec<_>>())
        .flat_map(|l| kids(&tree, l, "li").collect::<Vec<_>>())
        .map(|li| text(&tree, li))
        .filter(|s| !s.is_empty() && s != "Rechtspraak.nl")
        .collect();
    let mut fields = Map::new();
    let mut put = |k: &str, v: Value| {
        fields.insert(k.to_string(), v);
    };
    put("ecli", json!(ecli));
    put("court", json!(court.ecli));
    put("creator", json!(one("creator")));
    put("date", json!(date));
    put("issued", json!(one("issued")));
    put("modified", json!(one("modified")));
    put("dockets", json!(rechtspraak_dockets(&all("zaaknummer"))));
    put("procedures", json!(labelled("procedure")));
    put("subjects", json!(labelled("subject")));
    put("seat", json!(one("spatial")));
    put("relations", json!(relations));
    put("references", json!(labelled("references")));
    put("vindplaatsen", json!(vindplaatsen));
    put("ljn", json!(all("replaces")));
    put("abstract", json!(abstract_));
    put("title", json!(web_title(&tree, root)));
    put(
        "source_url",
        json!(format!(
            "https://uitspraken.rechtspraak.nl/details?id={ecli}"
        )),
    );
    put("lang", json!(language(&body_text)));
    let source_fields = Value::Object(fields);
    let decision = decision_of(court, &source_fields, body_text);
    Ok(RechtspraakParsed {
        decision,
        source_fields,
        court,
    })
}

/// What enters the checksum: the stored fields but the time the source last touched the document,
/// which a mass re-publication moves on documents it leaves unchanged.
pub fn rechtspraak_checksummed(source_fields: &Value, text: &str) -> Vec<u8> {
    let mut fields = source_fields.clone();
    if let Some(map) = fields.as_object_mut() {
        map.remove("modified");
    }
    [
        serde_json::to_vec(&fields).expect("a JSON value serialises"),
        text.as_bytes().to_vec(),
    ]
    .concat()
}

/// Branch of [`Decision::from_source_fields`] for `rechtspraak/`, on what the parse stored.
pub(crate) fn from_source_fields_rechtspraak(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let court = source_fields
        .get("court")
        .and_then(Value::as_str)
        .and_then(dutch_court)
        .unwrap_or_else(|| panic!("stored rechtspraak court not taken ({source_uid})"));
    decision_of(court, source_fields, full_text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<open-rechtspraak>
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ecli="https://e-justice.europa.eu/ecli" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:bwb="bwb-dl" xmlns:psi="http://psi.rechtspraak.nl/" xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
    <rdf:Description>
      <dcterms:identifier>ECLI:NL:HR:2024:1000</dcterms:identifier>
      <dcterms:modified>2025-03-21T21:48:13</dcterms:modified>
      <dcterms:issued rdfs:label="Publicatiedatum">2024-07-01</dcterms:issued>
      <dcterms:language>nl</dcterms:language>
      <dcterms:creator rdfs:label="Instantie" resourceIdentifier="http://standaarden.overheid.nl/owms/terms/Hoge_Raad_der_Nederlanden">Hoge Raad</dcterms:creator>
      <dcterms:date rdfs:label="Uitspraakdatum">2024-07-02</dcterms:date>
      <psi:zaaknummer rdfs:label="Zaaknr">22/01514 en 22/01512</psi:zaaknummer>
      <dcterms:type rdf:language="nl" resourceIdentifier="http://psi.rechtspraak.nl/uitspraak">Uitspraak</dcterms:type>
      <psi:procedure rdfs:label="Procedure" resourceIdentifier="http://psi.rechtspraak.nl/procedure#cassatie">Cassatie</psi:procedure>
      <dcterms:subject rdfs:label="Rechtsgebied" resourceIdentifier="http://psi.rechtspraak.nl/rechtsgebied#strafrecht">Strafrecht</dcterms:subject>
      <dcterms:relation rdfs:label="Formele relatie" ecli:resourceIdentifier="ECLI:NL:PHR:2024:482" psi:type="http://psi.rechtspraak.nl/conclusie" psi:aanleg="http://psi.rechtspraak.nl/eerdereAanleg">Conclusie: ECLI:NL:PHR:2024:482</dcterms:relation>
      <dcterms:references rdfs:label="Wetsverwijzing" bwb:resourceIdentifier="jci1.31:c:BWBR0001854&amp;artikel=420bis&amp;g=2024-07-02">Wetboek van Strafrecht 420bis</dcterms:references>
      <dcterms:replaces>AB1234</dcterms:replaces>
      <dcterms:hasVersion rdfs:label="Vindplaatsen">
        <rdf:list>
          <rdf:li>Rechtspraak.nl</rdf:li>
          <rdf:li>RvdW 2024/724</rdf:li>
        </rdf:list>
      </dcterms:hasVersion>
    </rdf:Description>
    <rdf:Description rdf:about="http://deeplink.rechtspraak.nl/uitspraak?id=ECLI:NL:HR:2024:1000">
      <dcterms:title rdf:language="nl">ECLI:NL:HR:2024:1000 Hoge Raad , 02-07-2024 / 22/01514</dcterms:title>
    </rdf:Description>
  </rdf:RDF>
  <inhoudsindicatie id="ECLI:NL:HR:2024:1000:INH" xml:space="preserve" xmlns="http://www.rechtspraak.nl/schema/rechtspraak-1.0">
      <parablock>
        <para>Gewoontewitwassen van woning in Spanje. </para>
        <para>HR: art. 81.1 RO.</para>
      </parablock>
    </inhoudsindicatie>
  <uitspraak id="ECLI:NL:HR:2024:1000:DOC" xml:space="preserve" xmlns="http://www.rechtspraak.nl/schema/rechtspraak-1.0">
  <uitspraak.info>
    <parablock>
      <para>HOGE RAAD DER NEDERLANDEN</para>
    </parablock>
    <para />
    <parablock>
      <para>
        <emphasis role="bold">Nummer </emphasis>22/01514 </para>
      <para>
        <emphasis role="bold">Datum	</emphasis>2 juli 2024</para>
    </parablock>
  </uitspraak.info>
  <section role="procesverloop">
    <title>
      <nr>1</nr>Procesverloop in cassatie</title>
    <parablock>
      <para>Het beroep is ingesteld door de verdachte. Namens deze hebben de advocaten bij schriftuur een cassatiemiddel voorgesteld<footnote-ref linkend="_f1" />.</para>
    </parablock>
  </section>
  <section role="overwegingen">
    <paragroup>
      <nr>2.1</nr>
      <para>Het middel klaagt dat het hof heeft miskend dat de woning niet uit enig misdrijf afkomstig is.</para>
      <para>Het middel faalt.</para>
    </paragroup>
    <informaltable><tgroup cols="2"><colspec colname="c1"/><tbody><row><entry><para>woning</para></entry><entry><para>Spanje</para></entry></row></tbody></tgroup></informaltable>
  </section>
  <footnote id="_f1" label="1">
    <para> HR 23 december 2022, ECLI:NL:HR:2022:1900.</para>
  </footnote>
</uitspraak>
</open-rechtspraak>"#;

    #[test]
    fn a_ruling_gives_its_court_date_dockets_relations_and_its_text_one_block_a_line() {
        let parsed = parse_rechtspraak(DOC).unwrap();
        let d = &parsed.decision;
        assert_eq!(d.source_uid, "rechtspraak/ECLI:NL:HR:2024:1000");
        assert_eq!(d.ecli.as_deref(), Some("ECLI:NL:HR:2024:1000"));
        assert_eq!(
            d.jurisdiction_source_code.as_deref(),
            Some("nl_cour_supreme")
        );
        assert_eq!(d.jurisdiction_type.as_deref(), Some("COUR_SUPREME"));
        assert_eq!(d.date_lecture.as_deref(), Some("2024-07-02"));
        assert_eq!(
            d.numero_dossiers,
            Some(vec!["22/01514".to_string(), "22/01512".to_string()])
        );
        assert_eq!(
            d.texte_integral_clean,
            "HOGE RAAD DER NEDERLANDEN\n\
             Nummer 22/01514\n\
             Datum 2 juli 2024\n\
             1 Procesverloop in cassatie\n\
             Het beroep is ingesteld door de verdachte. Namens deze hebben de advocaten bij schriftuur een cassatiemiddel voorgesteld[1].\n\
             2.1 Het middel klaagt dat het hof heeft miskend dat de woning niet uit enig misdrijf afkomstig is.\n\
             Het middel faalt.\n\
             woning\n\
             Spanje\n\
             [1] HR 23 december 2022, ECLI:NL:HR:2022:1900."
        );
        let sf = &parsed.source_fields;
        assert_eq!(
            sf["abstract"],
            "Gewoontewitwassen van woning in Spanje.\nHR: art. 81.1 RO."
        );
        assert_eq!(sf["relations"][0]["ecli"], "ECLI:NL:PHR:2024:482");
        assert_eq!(
            sf["relations"][0]["type"],
            "http://psi.rechtspraak.nl/conclusie"
        );
        assert_eq!(
            sf["references"][0]["id"],
            "jci1.31:c:BWBR0001854&artikel=420bis&g=2024-07-02"
        );
        assert_eq!(sf["vindplaatsen"], json!(["RvdW 2024/724"]));
        assert_eq!(sf["ljn"], json!(["AB1234"]));
        assert_eq!(sf["subjects"][0]["label"], "Strafrecht");
        assert_eq!(sf["lang"], "nld");
        let rebuilt = from_source_fields_rechtspraak(&d.texte_integral_clean, sf, &d.source_uid);
        assert_eq!(rebuilt.numero_dossiers, d.numero_dossiers);
        assert_eq!(rebuilt.date_lecture, d.date_lecture);
    }

    #[test]
    fn a_republication_that_changes_nothing_but_its_time_keeps_its_checksum() {
        let parsed = parse_rechtspraak(DOC).unwrap();
        let text = &parsed.decision.texte_integral_clean;
        let mut moved = parsed.source_fields.clone();
        moved["modified"] = json!("2026-11-19T10:00:00");
        assert_eq!(
            rechtspraak_checksummed(&parsed.source_fields, text),
            rechtspraak_checksummed(&moved, text)
        );
    }

    #[test]
    fn a_conclusion_a_caribbean_court_and_a_document_without_text_are_refused() {
        let conclusion = DOC
            .replace(">Uitspraak</dcterms:type>", ">Conclusie</dcterms:type>")
            .replace("ECLI:NL:HR:2024:1000<", "ECLI:NL:PHR:2024:482<");
        assert!(parse_rechtspraak(&conclusion).is_err());
        let aruba = DOC.replace(
            "<dcterms:identifier>ECLI:NL:HR:2024:1000",
            "<dcterms:identifier>ECLI:NL:OGEAA:2024:1000",
        );
        assert!(parse_rechtspraak(&aruba)
            .unwrap_err()
            .to_string()
            .contains("not taken"));
        let start = DOC.find("<uitspraak ").unwrap();
        let bare = format!("{}</open-rechtspraak>", &DOC[..start]);
        assert!(parse_rechtspraak(&bare).is_err());
    }

    #[test]
    fn case_numbers_split_on_their_joiners_and_lose_their_label() {
        let split = |s: &str| rechtspraak_dockets(&[s.to_string()]);
        assert_eq!(
            split("C/09/669557 / FA RK 24-5090"),
            ["C/09/669557", "FA RK 24-5090"]
        );
        assert_eq!(split("202400029/3/A3"), ["202400029/3/A3"]);
        assert_eq!(
            split("SXM202200874, SXM202201471, SXM202201472 en SXM202201473"),
            [
                "SXM202200874",
                "SXM202201471",
                "SXM202201472",
                "SXM202201473"
            ]
        );
        assert_eq!(
            split("AWB 12/15512  en AWB 12/15513"),
            ["AWB 12/15512", "AWB 12/15513"]
        );
        assert_eq!(
            split("SXM20230013 t/m SXM202300118"),
            ["SXM20230013", "SXM202300118"]
        );
        assert_eq!(
            split("10/5427 MAW + 10/5576 MAW"),
            ["10/5427 MAW", "10/5576 MAW"]
        );
        assert_eq!(split("Parketnr: 06.060011-03"), ["06.060011-03"]);
        assert_eq!(
            split("9455280 \\ CV EXPL  21-3985"),
            ["9455280", "CV EXPL 21-3985"]
        );
    }

    #[test]
    fn a_listing_gives_its_count_and_each_ecli_with_its_time() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?><feed xmlns="http://www.w3.org/2005/Atom"><title type="text">Rechtspraak Open Data (Uitspraken)</title><subtitle type="text">Aantal gevonden ECLI's: 976854</subtitle><entry><id>ECLI:NL:GHDHA:2025:79</id><title type="text">ECLI:NL:GHDHA:2025:79, Gerechtshof Den Haag, 16-01-2025, BK-23/901</title><summary type="text">Wet bpm.</summary><updated>2026-11-19T15:55:40Z</updated></entry></feed>"#;
        let listing = rechtspraak_listing(xml).unwrap();
        assert_eq!(listing.total, 976_854);
        assert_eq!(
            listing.entries,
            vec![(
                "ECLI:NL:GHDHA:2025:79".to_string(),
                "2026-11-19T15:55:40Z".to_string()
            )]
        );
        assert!(rechtspraak_taken("ECLI:NL:GHDHA:2025:79"));
        assert!(!rechtspraak_taken("ECLI:NL:OGEAA:2025:79"));
        assert!(!rechtspraak_taken("ECLI:NL:PHR:2024:482"));
    }
}
