//! The decisions African courts publish on their own sites: each site's listing read into
//! [`SiteEntry`] rows, and a decision built from its row and its text.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};
use unicode_normalization::UnicodeNormalization;

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

use crate::ejo::{ejo_title_reference, EjoText};
use crate::entscheidsuche::language;
use crate::juricaf::script_language;
use crate::justice_lu::{small_number, year_in_words, SMALL};
use crate::reflow_page_text;

/// A court read from its own site, under the source its provenances carry.
#[derive(Debug)]
pub struct SiteCourt {
    pub source: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
    pub place: &'static str,
}

pub const SITE_COURTS: [SiteCourt; 16] = [
    SiteCourt {
        source: "cc-bj",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "bj_cour_constitutionnelle",
        label: "Cour constitutionnelle (Bénin)",
        place: "geo:BJ",
    },
    SiteCourt {
        source: "hcc-mg",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "mg_cour_constitutionnelle",
        label: "Haute Cour constitutionnelle (Madagascar)",
        place: "geo:MG",
    },
    SiteCourt {
        source: "cc-ci",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "ci_cour_constitutionnelle",
        label: "Conseil constitutionnel (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "journalofficiel-dj",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "dj_cour_constitutionnelle",
        label: "Conseil constitutionnel (Djibouti)",
        place: "geo:DJ",
    },
    SiteCourt {
        source: "cc-dj",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "dj_cour_constitutionnelle",
        label: "Conseil constitutionnel (Djibouti)",
        place: "geo:DJ",
    },
    SiteCourt {
        source: "jurisprudence-mg",
        jurisdiction_type: "COUR_CASSATION",
        code: "mg_cour_cassation",
        label: "Cour de cassation (Madagascar)",
        place: "geo:MG",
    },
    SiteCourt {
        source: "african-court",
        jurisdiction_type: "COUR_DROITS_HOMME",
        code: "unionafricaine_cour_droits_homme",
        label: "Cour africaine des droits de l'homme et des peuples",
        place: "geo:UNIONAFRICAINE",
    },
    SiteCourt {
        source: "cassation-ci",
        jurisdiction_type: "COUR_CASSATION",
        code: "ci_cour_cassation",
        label: "Cour de cassation (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "ccja",
        jurisdiction_type: "CCJA",
        code: "ohada_ccja",
        label: "Cour commune de justice et d'arbitrage (OHADA)",
        place: "geo:OHADA",
    },
    SiteCourt {
        source: "cndj-ci",
        jurisdiction_type: "COUR_APPEL",
        code: "ci_cour_appel_abidjan",
        label: "Cour d'appel d'Abidjan (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "cndj-ci",
        jurisdiction_type: "COUR_APPEL",
        code: "ci_cour_appel_commerce_abidjan",
        label: "Cour d'appel de commerce d'Abidjan (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "cndj-ci",
        jurisdiction_type: "TRIBUNAL_COMMERCE",
        code: "ci_tribunal_commerce_abidjan",
        label: "Tribunal de commerce d'Abidjan (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "cndj-ci",
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "ci_tribunal_premiere_instance_abidjan",
        label: "Tribunal de première instance d'Abidjan (Côte d'Ivoire)",
        place: "geo:CI",
    },
    SiteCourt {
        source: "cc-ma",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "ma_cour_constitutionnelle",
        label: "Cour constitutionnelle (Maroc)",
        place: "geo:MA",
    },
    SiteCourt {
        source: "cc-ma",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "ma_conseil_constitutionnel",
        label: "Conseil constitutionnel (Maroc)",
        place: "geo:MA",
    },
    SiteCourt {
        source: "cc-ma",
        jurisdiction_type: "COUR_CONSTITUTIONNELLE",
        code: "ma_chambre_constitutionnelle",
        label: "Chambre constitutionnelle de la Cour suprême (Maroc)",
        place: "geo:MA",
    },
];

/// The bodies whose decisions the Cour constitutionnelle du Maroc publishes, by the name its
/// pages give them: itself from 2017, the Conseil constitutionnel from 1994, and before it the
/// Cour suprême's Chambre constitutionnelle.
const CC_MA_COURTS: [(&str, &str); 3] = [
    ("المحكمة الدستورية", "ma_cour_constitutionnelle"),
    ("المجلس الدستوري", "ma_conseil_constitutionnel"),
    ("الغرفة الدستورية", "ma_chambre_constitutionnelle"),
];

/// The sites whose decisions are written in Arabic.
const ARABIC_SITES: [&str; 1] = ["cc-ma"];

/// The courts of a site publishing several, by the name its listing and their text give them,
/// folded, the tribunals first: a tribunal's heading names its court of appeal above it.
const CNDJ_CI_COURTS: [(&str, &str); 4] = [
    (
        "tribunal de premiere instance d'abidjan",
        "ci_tribunal_premiere_instance_abidjan",
    ),
    (
        "tribunal de commerce d'abidjan",
        "ci_tribunal_commerce_abidjan",
    ),
    (
        "cour d'appel de commerce d'abidjan",
        "ci_cour_appel_commerce_abidjan",
    ),
    ("cour d'appel d'abidjan", "ci_cour_appel_abidjan"),
];

pub fn site_court(source: &str) -> Option<&'static SiteCourt> {
    SITE_COURTS.iter().find(|c| c.source == source)
}

/// A court of a source by its code.
fn coded_court(source: &str, code: &str) -> Option<&'static SiteCourt> {
    SITE_COURTS
        .iter()
        .find(|c| c.source == source && c.code == code)
}

/// A name folded for comparison, an OCR's space after an apostrophe dropped.
fn folded_name(s: &str) -> String {
    lj_fold::fold(s).replace("' ", "'")
}

/// The court of a site publishing several that a name opens on: « COUR D'APPEL D'ABIDJAN
/// CHAMBRES SOCIALES » is the Cour d'appel d'Abidjan.
fn named_court(source: &str, name: &str) -> Option<&'static SiteCourt> {
    let courts: &[(&str, &str)] = match source {
        "cc-ma" => &CC_MA_COURTS,
        _ => &CNDJ_CI_COURTS,
    };
    let name = folded_name(name);
    let (_, code) = courts.iter().find(|(n, _)| name.starts_with(n))?;
    coded_court(source, code)
}

/// The court of a site publishing several that a decision's text states: the one speaking in it,
/// « La Cour d'Appel d'Abidjan, 1ère Chambre Civile, séant au Palais de justice de ladite ville,
/// en son audience », else the first its heading names, `None` when it names none of them.
fn text_court(source: &str, text: &str) -> Option<&'static SiteCourt> {
    static SPEAKS: LazyLock<Regex> = LazyLock::new(|| {
        let names: Vec<String> = CNDJ_CI_COURTS
            .iter()
            .map(|(n, _)| regex::escape(n))
            .collect();
        Regex::new(&format!(
            r"\b(?:la|le) ({})\b[^;.]{{0,250}}?\ben son audience",
            names.join("|")
        ))
        .expect("regex")
    });
    let head = folded_name(&text.chars().take(HEAD_CHARS).collect::<String>());
    let (_, code) = match SPEAKS.captures(&head) {
        Some(c) => CNDJ_CI_COURTS.iter().find(|(n, _)| *n == &c[1])?,
        None => {
            let heading: String = head.chars().take(HEADING_CHARS).collect();
            CNDJ_CI_COURTS.iter().find(|(n, _)| heading.contains(n))?
        }
    };
    coded_court(source, code)
}

/// How much of a decision's text its court and date are sought in.
const HEAD_CHARS: usize = 3000;
/// How much of a decision's text its heading spans.
const HEADING_CHARS: usize = 800;

/// The court whose source a `source_uid` opens on.
pub fn site_court_of_uid(source_uid: &str) -> Option<&'static SiteCourt> {
    let (source, _) = source_uid.split_once('/')?;
    site_court(source)
}

/// Where a decision's text is read.
#[derive(Debug, Clone, PartialEq)]
pub enum SiteBody {
    Pdf(String),
    /// The decision's own page, read by its site's page parser.
    Page(String),
    /// The decision's page, the French PDF it links read.
    PdfOnPage(String),
    /// The text the listing carries, and the PDF it links, read when the text holds no decision.
    Html {
        html: String,
        pdf: Option<String>,
    },
}

/// A decision as a site's listing gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteEntry {
    /// The site's own key for the decision, unique within the site.
    pub id: String,
    /// The decision's numbers, as the court cites them.
    pub numbers: Vec<String>,
    /// ISO date; `None` when the listing gives none, the decision's heading then giving it.
    pub date: Option<String>,
    pub body: SiteBody,
    pub page_url: String,
    /// What else the listing states, by its label.
    pub fields: Map<String, Value>,
}

/// One page of a site's listing.
#[derive(Debug, Default)]
pub struct SiteListing {
    pub entries: Vec<SiteEntry>,
    /// The decisions the site counts, when the page states it.
    pub total: Option<usize>,
    /// The listing rows no date or number could be read from, by their key or title.
    pub unread: Vec<String>,
    /// The walk's last page, when the page names it or its count gives it.
    pub last_page: Option<usize>,
}

/// The fewest non-blank characters a decision's text holds; a scan's text layer holds fewer.
pub const DECISION_FLOOR: usize = 200;

pub fn holds_a_decision(text: &str) -> bool {
    text.chars().filter(|c| !c.is_whitespace()).count() >= DECISION_FLOOR
}

/// Words broken by a digit for a letter (`Requ6rant`) per 1000 words, from which a text layer is
/// read again by OCR: 112 of the African Court's 484 PDFs reach 10, none of the others does.
pub const GARBLED_PER_1000_WORDS: usize = 10;

pub fn garbled_layer(text: &str) -> bool {
    static BROKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[A-Za-z\u{C0}-\u{FF}]+[0-9][A-Za-z\u{C0}-\u{FF}]+").expect("regex")
    });
    let words = text.split_whitespace().count();
    words > 0 && BROKEN.find_iter(text).count() * 1000 >= GARBLED_PER_1000_WORDS * words
}

/// An HTML fragment's text, a line per block.
pub fn html_text(html: &str) -> String {
    static DROPPED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<(script|style|head)\b.*?</(script|style|head)>").expect("regex")
    });
    static BREAK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<br\s*/?>|</(p|div|li|h[1-6]|tr|table|ul|ol|blockquote)>").expect("regex")
    });
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("regex"));
    let html = DROPPED.replace_all(html, "");
    let html = BREAK.replace_all(&html, "\n");
    let text = TAG.replace_all(&html, "");
    html_escape::decode_html_entities(&text)
        .replace(['\u{a0}', '\u{feff}'], " ")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn inline_text(fragment: &str) -> String {
    html_text(fragment)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A French date written `8 octobre 2026`, `1er février 2006`, `jeudi 16 août 2007`,
/// `21octobre 2026`, `22/05/2018` or `vingt-six mars deux mil dix-neuf`, as an ISO date. The OCR
/// drops a weekday's last letter, `MARD 26 MARS 2019`.
fn french_date(s: &str) -> Option<String> {
    static WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:lundi?|mardi?|mercredi?|jeudi?|vendredi?|samedi?|dimanche?),?\s+")
            .expect("regex")
    });
    static SPELLED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)^(premier|{SMALL}(?:[\s-]+{SMALL})*)\s+([a-zéèêûô]+)\s+(?:de\s+l['’]an\s+)?((?:deux\s+mil(?:le)?|mil(?:le)?\s+neuf\s+cents?)(?:[\s-]+{SMALL})*|\d{{4}}\b)"
        ))
        .expect("regex")
    });
    static WRITTEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(\d{1,2})(?:er)?\s*([a-zéèêûô]+)\.?\s*(\d{4})\b").expect("regex")
    });
    static NUMERIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{1,2})/(\d{1,2})/(\d{4})\b").expect("regex"));
    let s = s.trim();
    let s = WEEKDAY.find(s).map_or(s, |m| &s[m.end()..]);
    if let Some(c) = NUMERIC.captures(s) {
        return lj_core::dates::parse_date_fr(&format!("{}/{}/{}", &c[1], &c[2], &c[3]))
            .map(|d| d.to_string());
    }
    if let Some(c) = WRITTEN.captures(s) {
        return lj_core::dates::parse_date_fr(&format!("{} {} {}", &c[1], &c[2], &c[3]))
            .map(|d| d.to_string());
    }
    let c = SPELLED.captures(s)?;
    let day = small_number(&c[1]).filter(|d| (1..=31).contains(d))?;
    let year = c[3].parse().ok().or_else(|| year_in_words(&c[3]))?;
    lj_core::dates::parse_date_fr(&format!("{day} {} {year}", &c[2])).map(|d| d.to_string())
}

/// The date a decision's heading gives, « DECISION N° E 001/95 » then « du 22 novembre 1995 ».
fn heading_date(text: &str) -> Option<String> {
    static HEADING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:d[ée]cision|avis)\s+n[°º]").expect("regex"));
    let start = HEADING.find(text)?.end();
    let head: String = text[start..].chars().take(200).collect();
    date_after_du(&head)
}

/// The date after the first `du` of a title.
fn date_after_du(title: &str) -> Option<String> {
    static DU: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-zà-ÿ])du\s*").expect("regex"));
    DU.find_iter(title)
        .find_map(|m| french_date(&title[m.end()..]))
}

const BJ_BASE: &str = "https://courconstitutionnelle.bj";

/// A page of `courconstitutionnelle.bj/fr/decisions?page=N`: ten cards, each titled
/// « DCC26-023 du 8 octobre 2026 », its labelled lines, then the decision's page and its PDF.
pub fn cc_bj_listing(html: &str) -> SiteListing {
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Total\s*:\s*(\d+)\s*d[ée]cisions").expect("regex"));
    static TITLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)^[^>]*>(.*?)</h4>").expect("regex"));
    static FIELD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<span[^>]*>([^<]+)</span>\s*:\s*(.*?)<br\s*/?>"#).expect("regex")
    });
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="[^"]*/fr/decisions/([^"/?#]+)""#).expect("regex"));
    static PDF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="([^"]+\.pdf)""#).expect("regex"));
    static HEAD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([A-Za-z]+)\s*(\d+\s*-\s*\d+)\s+du\s+").expect("regex"));

    let mut listing = SiteListing {
        total: TOTAL
            .captures(html)
            .and_then(|c| c[1].parse::<usize>().ok()),
        ..Default::default()
    };
    for card in html.split("card-title").skip(1) {
        let (Some(title), Some(id)) = (TITLE.captures(card), PAGE.captures(card)) else {
            continue;
        };
        let title = inline_text(&title[1]);
        let id = id[1].to_string();
        let number = HEAD
            .captures(&title)
            .map(|h| format!("{} {}", &h[1], h[2].replace(' ', "")));
        let (Some(number), Some(date), Some(pdf)) =
            (number, date_after_du(&title), PDF.captures(card))
        else {
            listing.unread.push(id);
            continue;
        };
        let mut fields = Map::new();
        for f in FIELD.captures_iter(card) {
            let value = inline_text(&f[2]);
            if !value.is_empty() {
                fields.insert(inline_text(&f[1]), Value::String(value));
            }
        }
        let pdf = &pdf[1];
        listing.entries.push(SiteEntry {
            body: SiteBody::Pdf(if pdf.starts_with("http") {
                pdf.to_string()
            } else {
                format!("{BJ_BASE}{pdf}")
            }),
            page_url: format!("{BJ_BASE}/fr/decisions/{id}"),
            id,
            numbers: vec![number],
            date: Some(date),
            fields,
        });
    }
    listing
}

const CI_BASE: &str = "https://www.conseil-constitutionnel.ci";

/// A page of `conseil-constitutionnel.ci/decisions?page=N`: five rows, each titled « DÉCISION
/// N° CI-2025-EP-007/04-11/CC/SG du 04 novembre 2025 portant … », the older ones by their number
/// alone, then the link of the PDF.
pub fn cc_ci_listing(html: &str) -> SiteListing {
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?s)<div class="col-sm-8"><p><a href="(/[^"]+)">(.*?)</a>.*?<div class="col-sm-4">(.*?)</div>"#,
        )
        .expect("regex")
    });
    static NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)n[°º]\s*([a-z]\s+\d\S*|\S+)(.*)").expect("regex"));
    static DU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*du\s+(.*)").expect("regex"));
    static PDF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="([^"]+\.pdf)""#).expect("regex"));
    let mut listing = SiteListing::default();
    for row in ROW.captures_iter(html) {
        let path = &row[1];
        let id = path.rsplit('/').next().unwrap_or(path).to_string();
        let title = inline_text(&row[2]);
        let (Some(n), Some(pdf)) = (NUMBER.captures(&title), PDF.captures(&row[3])) else {
            listing.unread.push(id);
            continue;
        };
        let number = n[1].trim_end_matches([',', '.', ';', ':']).to_string();
        let date = DU
            .captures(&n[2])
            .and_then(|d| french_date(&d[1]))
            .or_else(|| ci_docket_date(&number));
        let pdf = &pdf[1];
        let mut fields = Map::new();
        fields.insert("title".into(), Value::String(title.clone()));
        listing.entries.push(SiteEntry {
            numbers: vec![number],
            date,
            body: SiteBody::Pdf(if pdf.starts_with("http") {
                pdf.to_string()
            } else {
                format!("{CI_BASE}{pdf}")
            }),
            page_url: format!("{CI_BASE}{path}"),
            id,
            fields,
        });
    }
    listing
}

/// The date an Ivorian docket carries, « CI-2021-145/A/16-11/CC/SG » giving 16 November 2021.
fn ci_docket_date(number: &str) -> Option<String> {
    static DOCKET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^CI-(\d{4})[^/]*(?:/[A-Z]+)?/(\d{1,2})-(\d{1,2})\b").expect("regex")
    });
    let c = DOCKET.captures(number)?;
    lj_core::dates::parse_date_fr(&format!("{}/{}/{}", &c[2], &c[3], &c[1])).map(|d| d.to_string())
}

/// A page of the Haute Cour constitutionnelle's posts from its WordPress API, `_fields` id, link,
/// title and content: a decision, an opinion, an arrêt or a deliberation titled « Décision
/// n°04-HCC/D2 du 28 octobre 2026 … », its text in the post. The Malagasy versions, the
/// communiqués and the speeches the same categories hold are left aside.
pub fn hcc_mg_listing(json: &str) -> Result<SiteListing> {
    static KIND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(d[ée]cision|arr[êe]t|avis|d[ée]lib[ée]ration)\b").expect("regex")
    });
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)n[°º]\s*(\d{1,3})\s*[-\x{2013}]?\s*HCC(?:\s*/\s*([A-Z]{1,2}\d?))?")
            .expect("regex")
    });
    static PDF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="([^"]+\.pdf)""#).expect("regex"));
    let posts: Vec<Value> =
        serde_json::from_str(json).map_err(|e| CoreError::Xml(format!("hcc-mg posts: {e}")))?;
    let mut listing = SiteListing::default();
    for post in posts {
        let text = |k: &str| {
            post.get(k)
                .and_then(|v| v.get("rendered"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let (Some(id), Some(link)) = (
            post.get("id").and_then(Value::as_i64),
            post.get("link").and_then(Value::as_str),
        ) else {
            continue;
        };
        let title = inline_text(&text("title"));
        let Some(kind) = KIND.captures(&title).map(|k| k[1].to_string()) else {
            continue;
        };
        let content = text("content");
        let number = NUMBER.captures(&title).map(|n| match n.get(2) {
            Some(series) => format!("{:0>2}-HCC/{}", &n[1], series.as_str().to_uppercase()),
            None => format!("{:0>2}-HCC", &n[1]),
        });
        let (Some(number), Some(date)) = (number, date_after_du(&title)) else {
            listing.unread.push(format!("{id} {title}"));
            continue;
        };
        let mut fields = Map::new();
        fields.insert("kind".into(), Value::String(kind));
        fields.insert("title".into(), Value::String(title));
        listing.entries.push(SiteEntry {
            id: id.to_string(),
            numbers: vec![number],
            date: Some(date),
            body: SiteBody::Html {
                pdf: PDF.captures(&content).map(|p| p[1].to_string()),
                html: content,
            },
            page_url: link.to_string(),
            fields,
        });
    }
    Ok(listing)
}

/// A ruling of Djibouti's Conseil constitutionnel as its Journal officiel posts it, dated by the
/// « du » right after its number, else by its post: a later « du » names the vote it rules on.
pub fn ejo_ruling(t: &EjoText) -> SiteEntry {
    static DATED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bn\s*[°º]\s*[0-9][^\s,;]*\s+du\s+").expect("regex"));
    let content = t.content.clone().unwrap_or_default();
    let html = [
        Some(t.title.as_str()),
        t.visas.as_deref(),
        Some(content.as_str()),
        t.signature.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let reference = t
        .reference
        .clone()
        .or_else(|| ejo_title_reference(&t.title));
    let mut fields = Map::new();
    fields.insert("title".into(), Value::String(t.title.clone()));
    SiteEntry {
        id: t.id.to_string(),
        numbers: reference.into_iter().collect(),
        date: DATED
            .find(&t.title)
            .and_then(|m| french_date(&t.title[m.end()..]))
            .or_else(|| t.day.clone()),
        body: SiteBody::Html { html, pdf: None },
        page_url: t.link.clone().unwrap_or_default(),
        fields,
    }
}

/// A page of the Conseil constitutionnel de Djibouti's PDF uploads: a ruling or a proclamation,
/// numbered and dated by its title when it says so, its date otherwise read in its heading.
pub fn cc_dj_listing(json: &str) -> Result<SiteListing> {
    static KIND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:scan\s+)?(d[ée]cisi?on|proclamation)\b").expect("regex")
    });
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\bn(?:[°º]|o\b)\s*(\d{1,3}?)\s*(?:cc\s*)?((?:19|20)\d{2})?(?:\s*cc)?\b(?:\s+du\s+(.*))?",
        )
        .expect("regex")
    });
    let media: Vec<Value> =
        serde_json::from_str(json).map_err(|e| CoreError::Xml(format!("cc-dj media: {e}")))?;
    let mut listing = SiteListing::default();
    for m in media {
        let (Some(id), Some(url)) = (
            m.get("id").and_then(Value::as_i64),
            m.get("source_url").and_then(Value::as_str),
        ) else {
            continue;
        };
        let raw = m
            .pointer("/title/rendered")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let title = inline_text(&raw.replace(['-', '_'], " "));
        let Some(kind) = KIND.captures(&title).map(|k| k[1].to_lowercase()) else {
            continue;
        };
        let number = NUMBER.captures(&title);
        let mut fields = Map::new();
        fields.insert("kind".into(), Value::String(kind));
        fields.insert("title".into(), Value::String(title.clone()));
        listing.entries.push(SiteEntry {
            id: id.to_string(),
            numbers: number
                .as_ref()
                .map(|n| match n.get(2) {
                    Some(year) => format!("{}/{}", &n[1], year.as_str()),
                    None => n[1].to_string(),
                })
                .into_iter()
                .collect(),
            date: number
                .as_ref()
                .and_then(|n| n.get(3))
                .and_then(|d| french_date(d.as_str())),
            body: SiteBody::Pdf(url.to_string()),
            page_url: m
                .get("link")
                .and_then(Value::as_str)
                .unwrap_or(url)
                .to_string(),
            fields,
        });
    }
    Ok(listing)
}

/// A Djibouti Council ruling's number as `NN/YYYY/CC`, its year read in the number when the
/// number holds the ruling's year, else the ruling's: `98-001/CC`, `03/CC/21` and `4` agree with
/// the Council's own `01/2002/CC`.
fn dj_council_docket(number: &str, date: &str) -> Option<String> {
    static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("regex"));
    let year = date.get(..4)?;
    let short = &year[2..];
    let mut groups: Vec<&str> = DIGITS.find_iter(number).map(|m| m.as_str()).collect();
    if let [glued] = groups[..] {
        if glued.len() > 4 && glued.ends_with(year) {
            groups = vec![&glued[..glued.len() - 4], year];
        }
    }
    let at = groups.iter().rposition(|g| *g == year || *g == short);
    let ordinal = groups
        .iter()
        .enumerate()
        .find(|(i, _)| Some(*i) != at)
        .map(|(_, g)| g.parse::<u32>().ok())??;
    Some(format!("{ordinal:02}/{year}/CC"))
}

const AFRICAN_COURT_BASE: &str = "https://www.african-court.org";

/// A page of the African Court on Human and Peoples' Rights' decisions API, newest first: each
/// decision's date, the applications it rules on, its kind, its parties and its French PDF. A
/// decision without a French PDF is left unread.
pub fn african_court_listing(json: &str) -> Result<SiteListing> {
    let page: Value = serde_json::from_str(json)
        .map_err(|e| CoreError::Xml(format!("african-court decisions: {e}")))?;
    let text = |v: &Value, pointer: &str| {
        v.pointer(pointer)
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
    };
    let mut listing = SiteListing {
        total: page
            .get("total")
            .and_then(Value::as_u64)
            .and_then(|t| usize::try_from(t).ok()),
        ..Default::default()
    };
    for decision in page
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = decision.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let legals = decision
            .get("legals")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let numbers: Vec<String> = legals
            .iter()
            .filter_map(|l| text(l, "/case_number"))
            .collect();
        let date = text(decision, "/decision_date").and_then(|d| d.get(..10).map(str::to_string));
        let pdf = text(decision, "/fr_decision_file/path");
        let (Some(date), Some(pdf), Some(case)) = (date, pdf, legals.first()) else {
            listing.unread.push(format!(
                "{id} without its date, its application or its French PDF"
            ));
            continue;
        };
        let mut fields = Map::new();
        for (key, value) in [
            ("kind", text(decision, "/decision_type/fr_designation")),
            ("session", text(decision, "/session/fr_desc")),
            ("applicant", text(case, "/applicant/fr_designation")),
        ] {
            if let Some(value) = value {
                fields.insert(key.into(), Value::String(value));
            }
        }
        let respondents: Vec<Value> = case
            .get("defendants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| text(d, "/fr_title"))
            .map(Value::String)
            .collect();
        fields.insert("respondents".into(), Value::Array(respondents));
        listing.entries.push(SiteEntry {
            id: id.to_string(),
            numbers,
            date: Some(date),
            body: SiteBody::Pdf(pdf),
            page_url: format!(
                "{AFRICAN_COURT_BASE}/ajudata/details-case/{}",
                text(case, "/slug").unwrap_or_default()
            ),
            fields,
        });
    }
    Ok(listing)
}

/// The African Court's finalised advisory proceedings, one page: each request's number, the day
/// it was filed and the day it ended, its outcome, and its page, which links the decision's PDF.
pub fn african_court_advisories(html: &str) -> SiteListing {
    static ROW: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<tr>(.*?)</tr>").expect("regex"));
    static CELL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<td[^>]*>(.*?)</td>").expect("regex"));
    static DAY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"data-order="(\d{4}-\d{2}-\d{2})""#).expect("regex"));
    static NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(\d{3})\s*/\s*(\d{4})\b").expect("regex"));
    static PAGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"href="(https://[^"]*/details-advisory/(\d+))""#).expect("regex")
    });
    let body = html.split_once("<tbody").map_or("", |(_, b)| b);
    let body = body.split_once("</tbody>").map_or(body, |(b, _)| b);
    let mut listing = SiteListing::default();
    for row in ROW.captures_iter(body) {
        let cells: Vec<String> = CELL
            .captures_iter(&row[1])
            .map(|c| inline_text(&c[1]))
            .collect();
        let days: Vec<String> = DAY
            .captures_iter(&row[1])
            .map(|d| d[1].to_string())
            .collect();
        let title = cells.get(1).cloned().unwrap_or_default();
        let (Some(number), [filed, ended], Some(page)) =
            (NUMBER.captures(&title), &days[..], PAGE.captures(&row[1]))
        else {
            listing.unread.push(title);
            continue;
        };
        let mut fields = Map::new();
        fields.insert("title".into(), Value::String(title.clone()));
        fields.insert("filed".into(), Value::String(filed.clone()));
        if let Some(outcome) = cells.get(4) {
            fields.insert("outcome".into(), Value::String(outcome.clone()));
        }
        listing.entries.push(SiteEntry {
            id: format!("advisory-{}", &page[2]),
            numbers: vec![format!("{}/{}", &number[1], &number[2])],
            date: Some(ended.clone()),
            body: SiteBody::PdfOnPage(page[1].to_string()),
            page_url: page[1].to_string(),
            fields,
        });
    }
    listing
}

/// The French PDF of the decision an African Court advisory page lists: the last numbered
/// document that is no summary. The judges' opinions sit on unnumbered rows.
pub fn african_court_advisory_pdf(html: &str) -> Option<String> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?s)<tr>\s*<td>\s*<span[^>]*>\s*\d+\s*</span>\s*</td>",
            r"\s*<td>(.*?)</td>(.*?)</tr>"
        ))
        .expect("regex")
    });
    static FRENCH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<a href="(https://[^"]+)"[^>]*>\s*<i[^>]*></i>\s*FR\s*</a>"#)
            .expect("regex")
    });
    let table = html.split_once(r#"id="advisory-details""#)?.1;
    let table = table.split_once("</table>").map_or(table, |(t, _)| t);
    ROW.captures_iter(table)
        .filter(|r| !inline_text(&r[1]).to_lowercase().contains("summary"))
        .filter_map(|r| FRENCH.captures(&r[2]).map(|f| f[1].to_string()))
        .last()
}

const CCJA_BASE: &str = "https://biblio.ohada.org";

/// A page of a CCJA year category of `biblio.ohada.org`: each notice titled « Arrêt N° 016/2013 -
/// Affaire : … », then the link of its PDF. The date is the decision's heading's.
pub fn ccja_listing(html: &str) -> SiteListing {
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"id="nb_aut_details">\s*(\d+)"#).expect("regex"));
    static NOTICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)").expect("regex"));
    static TITLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<span class="tit1_notCourte">(.*?)</span>"#).expect("regex")
    });
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(?:arr[êe]t|ordonnance|avis)\s*(?:n[°º]\s*)?([0-9]{1,4})\s*/\s*([0-9]{4})\b",
        )
        .expect("regex")
    });
    static PDF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"doc_num\.php\?explnum_id=(\d+)").expect("regex"));
    let total = TOTAL
        .captures(html)
        .and_then(|c| c[1].parse::<usize>().ok());
    let mut listing = SiteListing {
        total,
        last_page: total.map(|t| t.div_ceil(CCJA_PAGE).max(1)),
        ..Default::default()
    };
    for notice in html.split("record-selection-accessibility-label-").skip(1) {
        let (Some(id), Some(title)) = (NOTICE.captures(notice), TITLE.captures(notice)) else {
            continue;
        };
        let id = id[1].to_string();
        let title = inline_text(&title[1]);
        let (Some(number), Some(pdf)) = (NUMBER.captures(&title), PDF.captures(notice)) else {
            listing.unread.push(format!("{id} {title}"));
            continue;
        };
        let mut fields = Map::new();
        fields.insert("title".into(), Value::String(title.clone()));
        listing.entries.push(SiteEntry {
            numbers: vec![ccja_number(&number[1], &number[2])],
            date: None,
            body: SiteBody::Pdf(format!("{CCJA_BASE}/doc_num.php?explnum_id={}", &pdf[1])),
            page_url: format!("{CCJA_BASE}/index.php?lvl=notice_display&id={id}"),
            id,
            fields,
        });
    }
    listing
}

/// An arrêt's number without its spaces and the zeros some copies pad it with: « 0021. Pe/24 »
/// and « 021. Pe/24 » give « 21.Pe/24 ».
fn cassation_ci_number(printed: &str) -> String {
    let number: String = printed.split_whitespace().collect();
    match number.trim_start_matches('0') {
        unpadded if unpadded.starts_with(|c: char| c.is_ascii_digit()) => unpadded.to_string(),
        _ => number,
    }
}

/// The notices a page of the OHADA library's listing holds, the most it offers.
pub const CCJA_PAGE: usize = 200;

/// A CCJA number as JuriCAF writes it, on three digits at least: « 16 », « 2013 » give
/// « 016/2013 ».
fn ccja_number(number: &str, year: &str) -> String {
    let n: u32 = number.parse().expect("digits");
    format!("{n:03}/{year}")
}

/// What a CCJA decision's heading states: its number and date from « ARRET N°002/2008 du 28
/// février 2008 », the date of « Audience publique du 28 février 2008 » when that line has none.
/// A notice's title on the site can carry another year than the decision's.
fn ccja_heading(text: &str) -> Heading {
    static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:ARR[EÊ]T|ORDONNANCE|AVIS)\s*N\s*[°º]?\s*([0-9]{1,4})\s*/\s*([0-9]{4})\b")
            .expect("regex")
    });
    static HEARING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)audience\s+publique\s+du\s+").expect("regex"));
    let head: String = text.chars().take(3000).collect();
    let numbered = NUMBERED.captures(&head);
    let date = numbered
        .as_ref()
        .and_then(|c| {
            let after = c.get(0).expect("match").end();
            date_after_du(&head[after..].chars().take(40).collect::<String>())
        })
        .or_else(|| {
            HEARING
                .find(&head)
                .and_then(|m| french_date(&head[m.end()..]))
        });
    Heading {
        number: numbered.map(|c| ccja_number(&c[1], &c[2])),
        date,
    }
}

/// The date most of a CNDJ decision's statements give: each « du <date> » of its heading, up to
/// the line where the court speaks, that line's own date, its file name's (`447-23042019`), and
/// for a court of appeal's arrêt its listing row's, a tribunal's row carrying the summons' date.
/// The clerks' templates leave stale dates in each of them, a heading's « AUDIENCE DU MARDI 26
/// FEVRIER 2019 » over « en son audience … du mardi vingt-six mars deux mil dix-neuf », or the
/// reverse, so none is trusted alone. A date before the year its file name numbers the decision
/// in (`533-2019`) is none; at a tie, the speaking line's wins.
fn cndj_date(entry: &SiteEntry, court: &SiteCourt, text: &str) -> Option<String> {
    static SPEAKING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\ben son audience\b").expect("regex"));
    static DU: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-zà-ÿ])du\s*").expect("regex"));
    static NAMED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"-(\d{2})(\d{2})(20\d{2})(?:\D|$)").expect("regex"));
    static NUMBERED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[-/]((?:19|20)\d{2})(?:\D|$)").expect("regex"));
    let head: String = text.chars().take(HEAD_CHARS).collect();
    let speaks = SPEAKING.find(&head).map(|m| m.start());
    let heading_end = head
        .char_indices()
        .nth(HEADING_CHARS)
        .map_or(head.len(), |(at, _)| at)
        .min(speaks.unwrap_or(head.len()));
    let speaking =
        speaks.and_then(|at| date_after_du(&head[at..].chars().take(160).collect::<String>()));
    let named = NAMED.captures(&entry.id).and_then(|c| {
        lj_core::dates::parse_date_fr(&format!("{}/{}/{}", &c[1], &c[2], &c[3]))
            .map(|d| d.to_string())
    });
    let listed = entry
        .fields
        .get("date")
        .and_then(Value::as_str)
        .filter(|_| court.jurisdiction_type == "COUR_APPEL")
        .and_then(french_date);
    let floor = NUMBERED
        .captures_iter(&entry.id)
        .last()
        .map_or(0, |c| c[1].parse().expect("a year"));
    let statements: Vec<String> = DU
        .find_iter(&head[..heading_end])
        .filter_map(|m| french_date(&head[m.end()..heading_end]))
        .chain(speaking.clone())
        .chain(named)
        .chain(listed)
        .filter(|d| d[..4].parse::<u16>().is_ok_and(|year| year >= floor))
        .collect();
    let votes = |d: &String| statements.iter().filter(|s| *s == d).count();
    let most = statements.iter().map(votes).max()?;
    speaking
        .filter(|d| votes(d) == most)
        .or_else(|| statements.iter().find(|d| votes(d) == most).cloned())
}

/// What a decision's own heading states, read when its listing row lacks it or may err.
struct Heading {
    number: Option<String>,
    date: Option<String>,
}

fn heading(source: &str, entry: &SiteEntry, court: &SiteCourt, text: &str) -> Heading {
    static ARRET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\barr[êe]t\s+n[°º]\s*[0-9]+\s*/\s*[0-9]{2,4}\s+").expect("regex")
    });
    match source {
        "ccja" => ccja_heading(text),
        "cassation-ci" => Heading {
            number: None,
            date: ARRET
                .find(text)
                .and_then(|m| date_after_du(&text[m.end()..].chars().take(40).collect::<String>())),
        },
        "cndj-ci" => Heading {
            number: None,
            date: cndj_date(entry, court, text),
        },
        "cc-dj" => {
            let head: String = text.chars().take(HEADING_CHARS).collect();
            let number = DJ_NUMBER.captures(&head);
            Heading {
                date: number
                    .as_ref()
                    .and_then(|n| n.get(2))
                    .and_then(|du| french_date(&head[du.end()..]))
                    .or_else(|| {
                        DJ_DONE
                            .find_iter(text)
                            .find_map(|m| french_date(&text[m.end()..]))
                    }),
                number: number.map(|n| n[1].trim_end_matches(['.', ',', ':']).to_string()),
            }
        }
        _ => Heading {
            number: None,
            date: heading_date(text),
        },
    }
}

/// A Djibouti Council ruling's number in its heading, and the « du » right after it.
static DJ_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:d[ée]cision|proclamation)\s+n\s*[°º]\s*([0-9][^\s,;]*)(\s+du\s+)?")
        .expect("regex")
});
/// Where a Djibouti Council ruling states the day it was given.
static DJ_DONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:fait\s+[àa]\s+djibouti,?\s+le|s[ée]ance\s+du)\s+").expect("regex")
});

/// A page of the Cour de cassation de Côte d'Ivoire's arrêts of one matter, on its e-services:
/// cards holding the outcome, « ARRET N°249/24 », the subject, and the arrêt's PDF, named after
/// its number and its date (« arret-n-249-24-du-05-mars-2024-1-1.pdf »). A page past the last
/// serves the last again.
pub fn cassation_ci_listing(html: &str) -> SiteListing {
    static CARD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<article class="stoppage-card">(.*?)</article>"#).expect("regex")
    });
    static OUTCOME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)class="post-category"[^>]*>(.*?)</p>"#).expect("regex"));
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)ARR[EÊ]T\s*N[°º]\s*([0-9]+(?:\s*\.\s*[a-z]+)?\s*/\s*[0-9]{2,4})")
            .expect("regex")
    });
    static SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Objet de l[^<]{1,8}Arr[êe]t</strong>\s*:\s*(.*?)</li>").expect("regex")
    });
    static PDF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"href="((https://[^"/]+)/pdfjs/web/viewer\.html\?file=(/uploads/stoppages/([^"/]+)\.pdf))""#,
        )
        .expect("regex")
    });
    static FILE_DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"-du-([0-9]{1,2})-([a-zéèûô]+)-([0-9]{4})").expect("regex"));
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"nb_page=([0-9]+)").expect("regex"));
    let mut listing = SiteListing {
        last_page: PAGE
            .captures_iter(html)
            .filter_map(|c| c[1].parse::<usize>().ok())
            .max(),
        ..Default::default()
    };
    for card in CARD.captures_iter(html) {
        let card = &card[1];
        let (Some(number), Some(pdf)) = (NUMBER.captures(card), PDF.captures(card)) else {
            listing.unread.push(inline_text(card));
            continue;
        };
        let id = pdf[4].to_string();
        let mut fields = Map::new();
        for (key, re) in [("outcome", &OUTCOME), ("subject", &SUBJECT)] {
            if let Some(c) = re.captures(card) {
                fields.insert(key.into(), Value::String(inline_text(&c[1])));
            }
        }
        listing.entries.push(SiteEntry {
            numbers: vec![cassation_ci_number(&number[1])],
            date: FILE_DATE.captures(&id).and_then(|d| {
                lj_core::dates::parse_date_fr(&format!("{} {} {}", &d[1], &d[2], &d[3]))
                    .map(|d| d.to_string())
            }),
            body: SiteBody::Pdf(format!("{}{}", &pdf[2], &pdf[3])),
            page_url: pdf[1].to_string(),
            id,
            fields,
        });
    }
    listing
}

/// A page of a free collection of `cndj.ci/jurisprudences`: ten table rows, each holding « 452/2019
/// du Vendredi 29 Mars 2019 », the court, the matter, the theme, the formation and the decision's
/// PDF. A row's id is its PDF's name, which two rows share now and then: the court and the date
/// are the decision's text's, the row's often another decision's.
pub fn cndj_ci_listing(html: &str) -> SiteListing {
    static ROW: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<tr>(.*?)</tr>").expect("regex"));
    static CELL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<td>(.*?)</td>").expect("regex"));
    static PDF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"href="(https://www\.cndj\.ci/download/decision/pdf/([^"/]+)\.pdf)""#)
            .expect("regex")
    });
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[?&]page=([0-9]+)").expect("regex"));
    let mut listing = SiteListing {
        last_page: PAGE
            .captures_iter(html)
            .filter_map(|c| c[1].parse::<usize>().ok())
            .max(),
        ..Default::default()
    };
    let Some(body) = html.split_once("<tbody>").map(|(_, b)| b) else {
        return listing;
    };
    let body = body.split_once("</tbody>").map_or(body, |(b, _)| b);
    for row in ROW.captures_iter(body) {
        let cells: Vec<String> = CELL
            .captures_iter(&row[1])
            .map(|c| inline_text(&c[1]))
            .collect();
        let pdf = PDF.captures(&row[1]);
        let ([first, court, matter, theme, formation, ..], Some(pdf)) = (&cells[..], pdf) else {
            listing.unread.push(inline_text(&row[1]));
            continue;
        };
        let (number, when) = first.split_once(" du ").unwrap_or((first, ""));
        if named_court("cndj-ci", court).is_none() {
            listing.unread.push(format!("{first} {court}"));
            continue;
        }
        let mut fields = Map::new();
        for (key, value) in [
            ("date", &when.trim().to_string()),
            ("court", court),
            ("matter", matter),
            ("theme", theme),
            ("formation", formation),
        ] {
            if !value.is_empty() {
                fields.insert(key.into(), Value::String(value.clone()));
            }
        }
        listing.entries.push(SiteEntry {
            id: pdf[2].to_string(),
            numbers: vec![number.trim().to_string()],
            date: None,
            body: SiteBody::Pdf(pdf[1].to_string()),
            page_url: pdf[1].to_string(),
            fields,
        });
    }
    listing
}

const MG_JURISPRUDENCE_BASE: &str = "https://jurisprudence.justice.gov.mg";

/// A page of `jurisprudence.justice.gov.mg/decision/{nature}?page=N`: twenty rows, each the
/// court, the kind of cassation, the chamber, the date and the arrêt's number.
pub fn jurisprudence_mg_listing(html: &str) -> SiteListing {
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+)\s*d[ée]cisions\s+trouv[ée]es").expect("regex"));
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)href="/decision/details/([A-Za-z0-9]+)">(.*?)</a>"#).expect("regex")
    });
    static SPAN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<span>([^<]*)</span>").expect("regex"));
    let mut listing = SiteListing {
        total: TOTAL
            .captures(html)
            .and_then(|c| c[1].parse::<usize>().ok()),
        ..Default::default()
    };
    for row in ROW.captures_iter(html) {
        let id = row[1].to_string();
        let cells: Vec<String> = SPAN
            .captures_iter(&row[2])
            .map(|s| inline_text(&s[1]).trim_end_matches(',').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let date = cells.iter().find_map(|c| french_date(c));
        let number = cells
            .last()
            .filter(|n| n.chars().any(|c| c.is_ascii_digit()) && french_date(n).is_none());
        let (Some(date), Some(number)) = (date, number) else {
            listing.unread.push(id);
            continue;
        };
        let mut fields = Map::new();
        if let Some(chamber) = cells.iter().find(|c| c.starts_with("Chambre")) {
            fields.insert("chamber".into(), Value::String(chamber.clone()));
        }
        let url = format!("{MG_JURISPRUDENCE_BASE}/decision/details/{id}");
        listing.entries.push(SiteEntry {
            numbers: vec![number.clone()],
            date: Some(date),
            body: SiteBody::Page(url.clone()),
            page_url: url,
            id,
            fields,
        });
    }
    listing
}

/// A decision's page on `jurisprudence.justice.gov.mg`: its heading « … - dossier 078/11-CU -
/// N° 255 du 22/05/2018 », its headnote and outcome, then the decision as an embedded document.
/// The text, and the labelled facts with the case file number under `dossier`.
pub fn jurisprudence_mg_page(html: &str) -> Option<(String, Map<String, Value>)> {
    static DOSSIER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)-\s*dossier\s+(\S+)\s*-\s*N°").expect("regex"));
    static LABELLED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<span class="fw-semibold">\s*([^<:]+?)\s*:\s*</span>(.*?)</p>"#)
            .expect("regex")
    });
    static PRINCIPLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Principe juridique\s*</h6>\s*<p>(.*?)</p>").expect("regex")
    });
    let (head, body) = html.split_once("Contenu de la d")?;
    let body = body.split_once("</h3>").map_or(body, |(_, b)| b);
    let body = body.split("</html>").next().unwrap_or(body);
    let text = html_text(body);
    let mut fields = Map::new();
    if let Some(d) = DOSSIER.captures(head) {
        fields.insert("dossier".into(), Value::String(d[1].to_string()));
    }
    for f in LABELLED.captures_iter(head) {
        let value = inline_text(&f[2]);
        if !value.is_empty() {
            fields.insert(inline_text(&f[1]), Value::String(value));
        }
    }
    if let Some(p) = PRINCIPLE.captures(head) {
        fields.insert(
            "Principe juridique".into(),
            Value::String(inline_text(&p[1])),
        );
    }
    Some((text, fields))
}

const CC_MA_BASE: &str = "https://www.cour-constitutionnelle.ma";

/// The decisions a page of the Cour constitutionnelle du Maroc's listing holds.
pub const CC_MA_PAGE: usize = 100;

/// A page of the Cour constitutionnelle du Maroc's listing, the JSON its decisions table is
/// served: each decision's key, its number « 295/26 », its case files « ملفان عدد : <b>552/26</b>
/// و<b>528/26</b> » and its date.
pub fn cc_ma_listing(json: &str) -> Result<SiteListing> {
    static FILE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<b>\s*([^<]*?)\s*</b>").expect("regex"));
    let page: Value =
        serde_json::from_str(json).map_err(|e| CoreError::Xml(format!("cc-ma listing: {e}")))?;
    let total = page["recordsTotal"].as_u64().map(|n| n as usize);
    let mut listing = SiteListing {
        total,
        last_page: total.map(|n| n.div_ceil(CC_MA_PAGE)),
        ..Default::default()
    };
    for row in page["data"].as_array().into_iter().flatten() {
        let Some(id) = row["iddecision"].as_u64().map(|n| n.to_string()) else {
            listing.unread.push(row.to_string());
            continue;
        };
        let number = row["numerodecision"].as_str().map(str::trim);
        let date = row["datedecision"]
            .as_str()
            .and_then(|d| d.get(..10))
            .filter(|d| d.parse::<jiff::civil::Date>().is_ok());
        let (Some(number), Some(date)) = (number.filter(|n| !n.is_empty()), date) else {
            listing.unread.push(id);
            continue;
        };
        let files = row["dossiers"].as_str().unwrap_or_default();
        let mut numbers = vec![number.to_string()];
        numbers.extend(
            FILE.captures_iter(files)
                .map(|f| f[1].to_string())
                .filter(|f| !f.is_empty()),
        );
        let url = format!("{CC_MA_BASE}/Decision?id={id}&Page=Decision");
        listing.entries.push(SiteEntry {
            numbers,
            date: Some(date.to_string()),
            body: SiteBody::Page(url.clone()),
            page_url: url,
            id,
            fields: Map::new(),
        });
    }
    Ok(listing)
}

/// A decision's page on `cour-constitutionnelle.ma`: the body that gave it, « قرارات المجلس
/// الدستوري », as its title, then the decision. The text, and the body under `court`.
pub fn cc_ma_page(html: &str) -> Option<(String, Map<String, Value>)> {
    static TITLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)id="decision-detail".*?<h2 class="title">([^<]*)</h2>"#).expect("regex")
    });
    let title = inline_text(&TITLE.captures(html)?[1]);
    let court = title.strip_prefix("قرارات ").unwrap_or(&title).to_string();
    let (_, body) = html.split_once(r#"data-db-content="true""#)?;
    let (_, body) = body.split_once('>')?;
    let body = body
        .split(r#"<div class="d-flex justify-content-between"#)
        .next()
        .unwrap_or(body);
    let mut fields = Map::new();
    fields.insert("court".into(), Value::String(court));
    Some((html_text(body), fields))
}

pub struct SiteParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static SiteCourt,
}

/// A decision from its listing row and its text: a PDF's as `pdftotext` reads it, reflowed, or
/// a page's as [`html_text`] reads it.
pub fn parse_site_decision(
    source: &str,
    entry: &SiteEntry,
    text: &str,
    from_pdf: bool,
) -> Result<SiteParsed> {
    let fail = |why: &str| CoreError::Xml(format!("{source}/{}: {why}", entry.id));
    let listed = match entry.fields.get("court").and_then(Value::as_str) {
        Some(name) => named_court(source, name),
        None => site_court(source),
    };
    let arabic = ARABIC_SITES.contains(&source);
    let text = if from_pdf {
        reflow_page_text(text)
    } else {
        text.to_string()
    };
    let text = if arabic { text.nfkc().collect() } else { text };
    if !holds_a_decision(&text) {
        return Err(fail("no decision text"));
    }
    let court = text_court(source, &text)
        .or(listed)
        .ok_or_else(|| fail("no court holds this source"))?;
    let lang = match script_language(&text) {
        "ara" => "ara",
        _ => language(&text),
    };
    if lang != if arabic { "ara" } else { "fra" } {
        return Err(fail(if arabic {
            "not written in Arabic"
        } else {
            "not written in French"
        }));
    }
    let stated = heading(source, entry, court, &text);
    let date = match &entry.date {
        Some(date) => date.clone(),
        None => stated
            .date
            .ok_or_else(|| fail("no date in its listing nor its heading"))?,
    };
    let numbered = stated.number.is_some();
    let mut numbers = stated
        .number
        .map_or_else(|| entry.numbers.clone(), |n| vec![n]);
    if let Some(dossier) = entry.fields.get("dossier").and_then(Value::as_str) {
        numbers.push(dossier.to_string());
    }
    if court.code == "dj_cour_constitutionnelle" {
        let docket = numbers.iter().find_map(|n| dj_council_docket(n, &date));
        if source == "cc-dj" && !numbered {
            numbers.clear();
        }
        if let Some(docket) = docket {
            let key = lj_core::docket_key::docket_key(&docket);
            if !numbers
                .iter()
                .any(|n| lj_core::docket_key::docket_key(n) == key)
            {
                numbers.push(docket);
            }
        }
    }
    let source_fields = json!({
        "court": court.code,
        "id": entry.id,
        "numbers": numbers,
        "date": date,
        "listing": entry.fields,
        "file_url": match &entry.body {
            SiteBody::Pdf(url) => Some(url.as_str()),
            SiteBody::Html { pdf, .. } if from_pdf => pdf.as_deref(),
            _ => None,
        },
        "url_site": entry.page_url,
        "lang": lang,
    });
    let decision = decision_of(
        court,
        format!("{source}/{}", entry.id),
        entry.id.clone(),
        date,
        numbers,
        text,
    );
    Ok(SiteParsed {
        decision,
        source_fields,
        court,
    })
}

fn decision_of(
    court: &SiteCourt,
    source_uid: String,
    member_name: String,
    date: String,
    numbers: Vec<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid,
        member_name,
        ecli: None,
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: numbers.first().cloned(),
        numero_dossiers: (!numbers.is_empty()).then_some(numbers),
        numero_role: None,
        date_lecture: Some(date),
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

/// Branch of [`Decision::from_source_fields`] for a court read from its site, on what the parse
/// stored.
pub(crate) fn from_source_fields_site(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let source = source_uid.split_once('/').map_or(source_uid, |(s, _)| s);
    let court = field("court")
        .and_then(|code| coded_court(source, code))
        .unwrap_or_else(|| panic!("stored site decision of no court ({source_uid})"));
    let date = field("date").unwrap_or_else(|| {
        panic!("source_fields of a site decision without a date ({source_uid})")
    });
    let numbers = source_fields
        .get("numbers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    decision_of(
        court,
        source_uid.to_string(),
        field("id").unwrap_or_default().to_string(),
        date.to_string(),
        numbers,
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BJ_CARDS: &str = r#"
        Total : 7470 décisions
        <div class="card-body">
            <h4 class="card-title mb-1 text-4 font-weight-bold">
                <i class="icon-docs icons text-color-primary text-4 me-2"></i>
                DCC26-023 du 8 octobre 2026
            </h4>
            <p class="card-text">
                <span class="text-decoration-underline font-weight-bold">Type de décision</span> : Décisions ordinaires (DCC) <br>
                <span class="text-decoration-underline font-weight-bold">Requérant</span> : Prosper ALLAGBE <br>
                <span class="text-decoration-underline font-weight-bold">Table analytique</span> :  <br>
                <span class="text-decoration-underline font-weight-bold">Objet de la requête</span> : Recours en inconstitutionnalit&eacute; du refus... <br>
            </p>
            <p class="card-text">
                <a href="https://courconstitutionnelle.bj/fr/decisions/DCC26-023" class="btn">Lire</a>
                <a href="https://courconstitutionnelle.bj/files/decisions/DCC26-023_8_octobre_2026.pdf" class="btn" download>PDF</a>
            </p>
        </div>
        <div class="card-body">
            <h4 class="card-title mb-1"><i class="icon-docs"></i> EP01-015 du 1er février 2006 </h4>
            <span>Type de décision</span> : Décisions électorales présidentielles (EP) <br>
            <a href="https://courconstitutionnelle.bj/fr/decisions/EP01-015">Lire</a>
            <a href="https://courconstitutionnelle.bj/files/decisions/EP01-015_21_fevrier_2006.pdf" download>PDF</a>
        </div>
        <div class="card-body">
            <h4 class="card-title mb-1"><i class="icon-docs"></i> DCC26-024 </h4>
            <a href="https://courconstitutionnelle.bj/fr/decisions/DCC26-024">Lire</a>
        </div>"#;

    const BODY: &str = "La Cour constitutionnelle, saisie d'une requête par laquelle le requérant \
        forme un recours en inconstitutionnalité, considère que la requête est recevable et que \
        le moyen tiré de la violation de la Constitution n'est pas fondé ; dit et juge qu'il n'y \
        a pas violation de la Constitution. La présente décision sera notifiée aux parties.";

    fn djibouti_ruling(title: &str, day: &str) -> EjoText {
        EjoText {
            id: 62654,
            day: Some(day.to_string()),
            modified: None,
            title: title.to_string(),
            reference: None,
            natures: vec![254],
            institutions: vec![880],
            issue: None,
            link: Some("https://www.journalofficiel.dj/texte-juridique/decision/".to_string()),
            content: Some(format!("<p>{BODY}</p>")),
            visas: Some("Vu la Constitution ;".to_string()),
            signature: Some("Le Président".to_string()),
        }
    }

    #[test]
    fn a_djibouti_ruling_is_dated_after_its_number_else_by_its_post() {
        let ruling = ejo_ruling(&djibouti_ruling(
            "Décision n° 1/2018/CC du 19 Février 2018 statuant sur une requête",
            "2018-02-22",
        ));
        assert_eq!(ruling.numbers, ["1/2018/CC"]);
        assert_eq!(ruling.date.as_deref(), Some("2018-02-19"));
        let SiteBody::Html { html, .. } = &ruling.body else {
            panic!("an HTML body: {:?}", ruling.body);
        };
        let parsed = parse_site_decision("journalofficiel-dj", &ruling, &html_text(html), false)
            .expect("a ruling");
        assert_eq!(parsed.court.code, "dj_cour_constitutionnelle");
        assert_eq!(parsed.decision.source_uid, "journalofficiel-dj/62654");
        assert!(parsed
            .decision
            .texte_integral_clean
            .starts_with("Décision n° 1/2018/CC du 19 Février 2018"));

        let proclamation = ejo_ruling(&djibouti_ruling(
            "Proclamation n° des résultats des élections législatives du 22 février 2013.",
            "2013-03-12",
        ));
        assert!(proclamation.numbers.is_empty());
        assert_eq!(proclamation.date.as_deref(), Some("2013-03-12"));
    }

    #[test]
    fn a_djibouti_council_number_is_also_read_as_its_ordinal_and_year() {
        for (number, date, docket) in [
            ("98-001/CC", "1998-01-19", Some("01/1998/CC")),
            ("002/98/CC", "1998-02-08", Some("02/1998/CC")),
            ("2021-01/CC", "2021-01-11", Some("01/2021/CC")),
            ("03/CC/21", "2021-04-19", Some("03/2021/CC")),
            ("4", "2016-04-19", Some("04/2016/CC")),
            ("032026CC", "2026-04-21", Some("03/2026/CC")),
            ("21", "2021-03-01", None),
        ] {
            assert_eq!(
                dj_council_docket(number, date).as_deref(),
                docket,
                "{number}"
            );
        }
        let ruling = ejo_ruling(&djibouti_ruling(
            "Décision n° 03/CC/21 du 19 Avril 2021 statuant sur une Requête",
            "2021-04-19",
        ));
        let SiteBody::Html { html, .. } = &ruling.body else {
            panic!("an HTML body: {:?}", ruling.body);
        };
        let parsed = parse_site_decision("journalofficiel-dj", &ruling, &html_text(html), false)
            .expect("a ruling");
        assert_eq!(
            parsed.decision.numero_dossiers.as_deref(),
            Some(&["03/CC/21".to_string(), "03/2021/CC".to_string()][..])
        );
    }

    #[test]
    fn a_djibouti_council_upload_is_a_ruling_numbered_by_its_title_else_by_its_heading() {
        let media = serde_json::json!([
            {"id": 2305, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2305",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2026/06/D-032026CC.pdf",
             "title": {"rendered": "DÉCISION N° 032026CC du 21 avril 2026 portant proclamation des résulta"}},
            {"id": 2426, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2426",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2026/07/D-2-2023.pdf",
             "title": {"rendered": "Decision-no-2-2023-du-05-fevrier-2023-statuant-la-requete-de-lAlliance"}},
            {"id": 2182, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2182",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2025/08/D-4.pdf",
             "title": {"rendered": "DECISION N° 4 &#8211; PROCLAMATION DES RESULTATS DEFINITIFS DE L&rsquo;ELECTION"}},
            {"id": 2241, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2241",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2025/08/P-2008.pdf",
             "title": {"rendered": "PROCLAMATION DES RESULTATS DES ELECTIONS DU 8 FEVRIER 2008"}},
            {"id": 2349, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2349",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2026/06/CONSTITUTION.pdf",
             "title": {"rendered": "CONSTITUTION"}},
            {"id": 2160, "link": "https://www.conseil-constitutionnel.dj/?attachment_id=2160",
             "source_url": "https://www.conseil-constitutionnel.dj/wp-content/uploads/2025/08/L.pdf",
             "title": {"rendered": "Scan Lettre de constestation relative à la décision rendue le 19-04-20"}}
        ])
        .to_string();
        let listing = cc_dj_listing(&media).expect("a media page");
        let [glued, spelled, bare, proclamation] = &listing.entries[..] else {
            panic!("four rulings: {:?}", listing.entries);
        };
        assert_eq!(glued.numbers, ["03/2026"]);
        assert_eq!(glued.date.as_deref(), Some("2026-04-21"));
        assert_eq!(spelled.numbers, ["2/2023"]);
        assert_eq!(spelled.date.as_deref(), Some("2023-02-05"));
        assert_eq!(bare.numbers, ["4"]);
        assert_eq!(bare.date, None);
        assert!(proclamation.numbers.is_empty());
        assert_eq!(proclamation.date, None);

        let text = format!(
            "RÉPUBLIQUE DE DJIBOUTI\nCONSEIL CONSTITUTIONNEL\nDÉCISION N° 04/2016/CC du 19 avril 2016\n\
             portant proclamation des résultats de l'élection présidentielle du 8 avril 2016\n{BODY}"
        );
        let parsed = parse_site_decision("cc-dj", bare, &text, true).expect("a ruling");
        assert_eq!(parsed.court.code, "dj_cour_constitutionnelle");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2016-04-19"));
        assert_eq!(
            parsed.decision.numero_dossiers.as_deref(),
            Some(&["04/2016/CC".to_string()][..])
        );
        let text = format!(
            "PROCLAMATION DES RÉSULTATS DÉFINITIFS DES ÉLECTIONS LÉGISLATIVES\n{BODY}\n\
             Fait à Djibouti, le 12 février 2008\nLe Président"
        );
        let parsed = parse_site_decision("cc-dj", proclamation, &text, true).expect("a ruling");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2008-02-12"));
        assert_eq!(parsed.decision.numero_dossiers, None);
    }

    #[test]
    fn a_benin_listing_card_gives_the_number_the_date_and_the_labelled_lines() {
        let listing = cc_bj_listing(BJ_CARDS);
        assert_eq!(listing.total, Some(7470));
        assert_eq!(listing.unread, ["DCC26-024"]);
        let [first, second] = &listing.entries[..] else {
            panic!("two cards read: {:?}", listing.entries);
        };
        assert_eq!(first.id, "DCC26-023");
        assert_eq!(first.numbers, ["DCC 26-023"]);
        assert_eq!(first.date.as_deref(), Some("2026-10-08"));
        assert_eq!(
            first.body,
            SiteBody::Pdf(
                "https://courconstitutionnelle.bj/files/decisions/DCC26-023_8_octobre_2026.pdf"
                    .into()
            )
        );
        assert_eq!(first.fields["Requérant"], "Prosper ALLAGBE");
        assert_eq!(
            first.fields["Objet de la requête"],
            "Recours en inconstitutionnalité du refus..."
        );
        assert!(!first.fields.contains_key("Table analytique"));
        assert_eq!(second.numbers, ["EP 01-015"]);
        assert_eq!(second.date.as_deref(), Some("2006-02-01"));
    }

    #[test]
    fn a_madagascar_constitutional_post_is_numbered_from_its_title_and_a_malagasy_one_left_aside() {
        let posts = serde_json::json!([
            {"id": 10128, "link": "https://www.hcc.gov.mg/?p=10128",
             "title": {"rendered": "Décision n°04-HCC/D2 du 28 octobre 2026 concernant une requête"},
             "content": {"rendered": "<p>La Haute Cour Constitutionnelle,</p><p>Vu la Constitution&nbsp;;</p>"}},
            {"id": 10116, "link": "https://www.hcc.gov.mg/?p=10116",
             "title": {"rendered": "Décision n°16-HCC/D1du 22 octobre 2026 concernant l&#8217;Accord"},
             "content": {"rendered": "<p><a href=\"https://www.hcc.gov.mg/wp-content/uploads/d.pdf\">PDF</a></p>"}},
            {"id": 662, "link": "https://www.hcc.gov.mg/?p=662",
             "title": {"rendered": "Décision n°7 HCC/D3 du jeudi 16 août 2007 concernant la loi"},
             "content": {"rendered": "<p>texte</p>"}},
            {"id": 10052, "link": "https://www.hcc.gov.mg/?p=10052",
             "title": {"rendered": "Décision n°16-HCC du 21 août 2026  concernant la loi"},
             "content": {"rendered": "<p>texte</p>"}},
            {"id": 9188, "link": "https://www.hcc.gov.mg/?p=9188",
             "title": {"rendered": "Didim-pitsarana laharana faha 31-HCC/AR tamin’ny 27 jona 2024"},
             "content": {"rendered": "<p>teny</p>"}},
            {"id": 9065, "link": "https://www.hcc.gov.mg/?p=9065",
             "title": {"rendered": "Arrêt n°09-HCC/AR du 14 avril 204 relatif à la requête"},
             "content": {"rendered": "<p>texte</p>"}}
        ]);
        let listing = hcc_mg_listing(&posts.to_string()).expect("posts");
        let numbers: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.numbers[0].as_str(), e.date.as_deref().unwrap_or_default()))
            .collect();
        assert_eq!(
            numbers,
            [
                ("04-HCC/D2", "2026-10-28"),
                ("16-HCC/D1", "2026-10-22"),
                ("07-HCC/D3", "2007-08-16"),
                ("16-HCC", "2026-08-21")
            ]
        );
        assert_eq!(listing.unread.len(), 1);
        assert!(listing.unread[0].starts_with("9065 "));
        match &listing.entries[1].body {
            SiteBody::Html { pdf, .. } => assert_eq!(
                pdf.as_deref(),
                Some("https://www.hcc.gov.mg/wp-content/uploads/d.pdf")
            ),
            other => panic!("an HTML body: {other:?}"),
        }
        let SiteBody::Html { html, .. } = &listing.entries[0].body else {
            panic!("an HTML body");
        };
        assert_eq!(
            html_text(html),
            "La Haute Cour Constitutionnelle,\nVu la Constitution ;"
        );
    }

    #[test]
    fn a_madagascar_cassation_row_and_page_give_the_arret_and_its_case_file() {
        let rows = r#"<span class="fw-semibold">834 décisions trouvées</span>
            <a class="text-dark"
               href="/decision/details/DCS68389AFFEDD0F">
                <span>1. <span class="bx bx-chevron-right me-3"></span> </span>
                <span>Cour de Cassation, </span>
                <span> Ordinaire, </span>
                <span>Chambre Civile,</span>
                <span>07/09/2018,</span>
                <span> 282,</span>
            </a>"#;
        let listing = jurisprudence_mg_listing(rows);
        assert_eq!(listing.total, Some(834));
        let [entry] = &listing.entries[..] else {
            panic!("one row read: {:?}", listing);
        };
        assert_eq!(entry.id, "DCS68389AFFEDD0F");
        assert_eq!(entry.date.as_deref(), Some("2018-09-07"));
        assert_eq!(entry.numbers, ["282"]);
        assert_eq!(entry.fields["chamber"], "Chambre Civile");

        let page = r#"<h3>
                Contribution aux charges du ménage - dossier 078/11-CU - N° 255
                du 22/05/2018
            </h3>
            <p><span class="fw-semibold">Mots clés : </span>
                <span>Contribution aux charges du ménage, Question de fait</span></p>
            <h6 class="mb-1">Principe juridique </h6>
            <p>L’octroi d’une contribution est une question de fait.</p>
            <p><span class="fw-semibold">Solution : </span>
                Rejet
            </p>
            <h3 class="text-center">Contenu de la décision <small>PDF</small></h3>
            <html><head><style>.cs{color:#000}</style></head><body>
            <p class="cs"><span>LA COUR</span></p><p><span>Apr&egrave;s en avoir d&eacute;lib&eacute;r&eacute; ;</span></p>
            </body></html>
            <footer>Jurisprudence Madagascar</footer>"#;
        let (text, fields) = jurisprudence_mg_page(page).expect("a page");
        assert_eq!(text, "LA COUR\nAprès en avoir délibéré ;");
        assert_eq!(fields["dossier"], "078/11-CU");
        assert_eq!(fields["Solution"], "Rejet");
        assert_eq!(
            fields["Principe juridique"],
            "L’octroi d’une contribution est une question de fait."
        );
        let mut entry = entry.clone();
        entry.fields.extend(fields);
        let parsed = parse_site_decision("jurisprudence-mg", &entry, BODY, false).expect("parsed");
        assert_eq!(
            parsed.decision.numero_dossiers.as_deref(),
            Some(&["282".to_string(), "078/11-CU".to_string()][..])
        );
    }

    #[test]
    fn a_decision_keeps_its_site_key_and_is_rebuilt_from_what_the_parse_stored() {
        let listing = cc_bj_listing(BJ_CARDS);
        let parsed =
            parse_site_decision("cc-bj", &listing.entries[0], BODY, true).expect("a decision");
        assert_eq!(parsed.decision.source_uid, "cc-bj/DCC26-023");
        assert_eq!(parsed.court.code, "bj_cour_constitutionnelle");
        assert_eq!(
            parsed.source_fields["file_url"],
            "https://courconstitutionnelle.bj/files/decisions/DCC26-023_8_octobre_2026.pdf"
        );
        let rebuilt =
            from_source_fields_site(BODY, &parsed.source_fields, &parsed.decision.source_uid);
        assert_eq!(rebuilt.numero_dossiers, parsed.decision.numero_dossiers);
        assert_eq!(rebuilt.date_lecture.as_deref(), Some("2026-10-08"));
        assert!(parse_site_decision("cc-bj", &listing.entries[0], "1\n2\n", true).is_err());
    }

    #[test]
    fn an_ivorian_row_without_a_date_takes_the_one_its_heading_gives() {
        let rows = r#"
            <div class="col-sm-8"><p><a href="/archives-et-decisions/decision-ndeg-ci-2025-ep-00704-11ccsg-du-04-novembre-2025-portant-proclamation">DÉCISION N° CI-2025-EP-007/04-11/CC/SG du 04 novembre 2025 portant proclamation des résultats définitifs de l...</a></p><br />
            <span class="bd-deci"></span>
            </div>
            <div class="col-sm-4"><a href="https://www.conseil-constitutionnel.ci/sites/default/files/decision_2025-ep-007_du_04.11.2025.pdf">Télécharger</a> </div>
            <div class="col-sm-8"><p><a href="/archives-et-decisions/decision-ndeg-e-00195">DECISION N° E 001/95</a></p><br />
            </div>
            <div class="col-sm-4"><a href="https://www.conseil-constitutionnel.ci/sites/default/files/13607711001.pdf">Télécharger</a> </div>
            <div class="col-sm-8"><p><a href="/archives-et-decisions/sans-fichier">DECISION N° CI-2016-EL-235/CC/SG</a></p><br />
            </div>
            <div class="col-sm-4"></div>"#;
        let listing = cc_ci_listing(rows);
        assert_eq!(listing.unread, ["sans-fichier"]);
        let [first, second] = &listing.entries[..] else {
            panic!("two rows read: {:?}", listing.entries);
        };
        assert_eq!(first.numbers, ["CI-2025-EP-007/04-11/CC/SG"]);
        assert_eq!(first.date.as_deref(), Some("2025-11-04"));
        assert_eq!(
            first.page_url,
            "https://www.conseil-constitutionnel.ci/archives-et-decisions/decision-ndeg-ci-2025-ep-00704-11ccsg-du-04-novembre-2025-portant-proclamation"
        );
        assert_eq!(second.numbers, ["E 001/95"]);
        assert_eq!(second.date, None);
        let text = format!(
            "RÉPUBLIQUE DE CÔTE D’IVOIRE\nDECISION N° E 001/95\ndu 22 novembre 1995\nAU NOM DU PEUPLE DE CÔTE D’IVOIRE,\n{BODY}"
        );
        let parsed = parse_site_decision("cc-ci", second, &text, false).expect("a decision");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("1995-11-22"));
        assert_eq!(parsed.source_fields["date"], "1995-11-22");
        assert!(parse_site_decision("cc-ci", second, BODY, false).is_err());
    }

    #[test]
    fn an_african_court_decision_gives_its_applications_its_date_and_its_french_pdf() {
        let page = r#"{"total": 488, "data": [
            {"id": 779, "decision_date": "2026-09-24 12:00:00",
             "decision_type": {"fr_designation": "Arrêts (Fond  et réparations)"},
             "session": {"fr_desc": "82ième  Session Ordinaire"},
             "legals": [
                {"case_number": "004/2022", "slug": "0042022",
                 "applicant": {"fr_designation": "Kambeyo Ka Kaunda"},
                 "defendants": [{"fr_title": "République du Malawi"}]},
                {"case_number": "005/2022", "slug": "0052022"}],
             "fr_decision_file": {"path": "https://www.african-court.org/ajudata/storage/app/uploads/public/6ad/09e/835/a.pdf"}},
            {"id": 786, "decision_date": "2026-09-29 10:56:32", "legals": [{"case_number": "001/2026"}],
             "fr_decision_file": null}
        ]}"#;
        let listing = african_court_listing(page).expect("a page");
        assert_eq!(listing.total, Some(488));
        assert_eq!(listing.unread.len(), 1);
        let [entry] = &listing.entries[..] else {
            panic!("one decision read: {:?}", listing.entries);
        };
        assert_eq!(entry.id, "779");
        assert_eq!(entry.numbers, ["004/2022", "005/2022"]);
        assert_eq!(entry.date.as_deref(), Some("2026-09-24"));
        assert_eq!(
            entry.body,
            SiteBody::Pdf(
                "https://www.african-court.org/ajudata/storage/app/uploads/public/6ad/09e/835/a.pdf"
                    .into()
            )
        );
        assert_eq!(
            entry.page_url,
            "https://www.african-court.org/ajudata/details-case/0042022"
        );
        assert_eq!(entry.fields["kind"], "Arrêts (Fond et réparations)");
        assert_eq!(entry.fields["applicant"], "Kambeyo Ka Kaunda");
        assert_eq!(entry.fields["respondents"], json!(["République du Malawi"]));
    }

    #[test]
    fn an_african_court_advisory_is_dated_by_its_end_and_read_from_its_decision_s_french_pdf() {
        let listing = r#"<table><thead><tr><th>#</th></tr></thead><tbody>
            <tr>
              <td class="text-center ajudata-col-num">1</td>
              <td>  Advisory Opinion No 001/2020, On the right to participate, requested by PALU
            </td>
              <td data-order="2020-06-03">03 Jun 2020</td>
              <td data-order="2021-07-16">16 Jul 2021</td>
              <td>  Finalized
            </td>
              <td><a href="https://www.african-court.org/ajudata/details-advisory/0012020"
                 class="btn btn-sm btn-primary">Details</a></td>
            </tr>
            <tr>
              <td class="text-center ajudata-col-num">2</td>
              <td>A request without its number</td>
              <td data-order="2011-05-12">12 May 2011</td>
              <td data-order="2012-03-07">07 Mar 2012</td>
              <td>Struck out</td>
              <td><a href="https://www.african-court.org/ajudata/details-advisory/0012011">Details</a></td>
            </tr>
            </tbody></table>"#;
        let listing = african_court_advisories(listing);
        assert_eq!(listing.unread, ["A request without its number"]);
        let [entry] = &listing.entries[..] else {
            panic!("one request read: {:?}", listing.entries);
        };
        assert_eq!(entry.id, "advisory-0012020");
        assert_eq!(entry.numbers, ["001/2020"]);
        assert_eq!(entry.date.as_deref(), Some("2021-07-16"));
        assert_eq!(entry.fields["filed"], "2020-06-03");
        assert_eq!(entry.fields["outcome"], "Finalized");
        let page = "https://www.african-court.org/ajudata/details-advisory/0012020";
        assert_eq!(entry.body, SiteBody::PdfOnPage(page.into()));
        assert_eq!(entry.page_url, page);

        let link = |url: &str, lang: &str| {
            format!(
                r#"<a href="{url}" target="_blank" class="btn btn-sm btn-success"><i class="bi bi-download"></i> {lang} </a>"#
            )
        };
        let off = r##"<a href="#" class="btn btn-sm btn-danger disabled"><i class="bi bi-x-circle"></i> FR </a>"##;
        let numbered = |n: u8, label: &str, links: &str| {
            format!(
                r#"<tr><td><span class="badge text-bg-secondary">{n}</span></td>
                <td>{label} <span class="badge text-bg-info float-end">16 Jul 2021</span></td>
                <td>{links}</td></tr>"#
            )
        };
        let page = [
            r#"<table id="advisory-details"><tbody>"#.to_string(),
            numbered(1, "Executive Summary", off),
            numbered(
                2,
                "Advisory Opinion summary",
                &link("https://x/summary-fr.pdf", "FR"),
            ),
            numbered(
                3,
                "Advisory opinion",
                &[
                    link("https://x/opinion-en.pdf", "EN"),
                    link("https://x/opinion-fr.pdf", "FR"),
                ]
                .concat(),
            ),
            format!(
                r#"<tr><td></td><td class="ajudata-opinion-row">Separate Opinion by Justice X</td>
                <td>{}</td></tr>"#,
                link("https://x/separate-fr.pdf", "FR")
            ),
            "</tbody></table>".to_string(),
        ]
        .concat();
        assert_eq!(
            african_court_advisory_pdf(&page).as_deref(),
            Some("https://x/opinion-fr.pdf")
        );
        let untranslated = page.replace(&link("https://x/opinion-fr.pdf", "FR"), off);
        assert_eq!(african_court_advisory_pdf(&untranslated), None);
    }

    #[test]
    fn an_ivorian_avis_is_dated_by_its_docket_and_never_by_its_subject() {
        let rows = r#"
            <div class="col-sm-8"><p><a href="/archives-et-decisions/avis-ndeg-ci-2021-145a16-11ccsg">AVIS N° CI-2021-145/A/16-11/CC/SG Relatif à la demande du Président de la République sur la modification de la loi organique n° 2018-867 du 19 novembre 2018</a></p><br />
            </div>
            <div class="col-sm-4"><a href="https://www.conseil-constitutionnel.ci/sites/default/files/avis_ndeg_ci-2021-145_du_16.11.2021.pdf">Télécharger</a> </div>
            <div class="col-sm-8"><p><a href="/archives-et-decisions/decision-ndeg-ci-2016-el-23603-12ccsg">DECISION N° CI-2016-EL-236/03-12/CC/SG</a></p><br />
            </div>
            <div class="col-sm-4"><a href="https://www.conseil-constitutionnel.ci/sites/default/files/el-236.pdf">Télécharger</a> </div>"#;
        let listing = cc_ci_listing(rows);
        let [avis, decision] = &listing.entries[..] else {
            panic!("two rows read: {:?}", listing.entries);
        };
        assert_eq!(avis.numbers, ["CI-2021-145/A/16-11/CC/SG"]);
        assert_eq!(avis.date.as_deref(), Some("2021-11-16"));
        assert_eq!(decision.numbers, ["CI-2016-EL-236/03-12/CC/SG"]);
        assert_eq!(decision.date.as_deref(), Some("2016-12-03"));
    }

    #[test]
    fn a_ccja_notice_is_numbered_and_dated_by_its_decision_s_heading() {
        let page = r#"
            Documents disponibles dans cette catégorie (<span id="nb_aut_details">6</span>)
            <h3 id="record-selection-accessibility-label-6395"><a href="https://biblio.ohada.org/index.php?lvl=notice_display&amp;id=6395">
                <span class="tit1_notCourte">Arrêt N° 003/2011 - Affaire : Emile WAKIM c/ SOCIETE IAMGOLD / AGEM</span></a></h3>
            <div class="plusN"><a href="https://biblio.ohada.org/index.php?lvl=notice_display&amp;id=6395">Plus d'information...</a></div>
            <span class="docNumN"><a href="https://biblio.ohada.org/doc_num.php?explnum_id=4077" target="_blank">Document numérique</a></span>
            <h3 id="record-selection-accessibility-label-4783"><a href="https://biblio.ohada.org/index.php?lvl=notice_display&amp;id=4783">
                <span class="tit1_notCourte">Arrêt 16/2013 - Affaire : Banque OMNIFINANCE c/ Société METAL TRADING SA</span></a></h3>
            <span class="docNumN"><a href="https://biblio.ohada.org/doc_num.php?explnum_id=3584" target="_blank">Document numérique</a></span>
            <h3 id="record-selection-accessibility-label-9001"><a href="https://biblio.ohada.org/index.php?lvl=notice_display&amp;id=9001">
                <span class="tit1_notCourte">Arrêt 017/2013 - Affaire : sans fichier</span></a></h3>"#;
        let listing = ccja_listing(page);
        assert_eq!(listing.total, Some(6));
        assert_eq!(listing.last_page, Some(1));
        assert_eq!(
            listing.unread,
            ["9001 Arrêt 017/2013 - Affaire : sans fichier"]
        );
        let [misnumbered, plain] = &listing.entries[..] else {
            panic!("two notices read: {:?}", listing.entries);
        };
        assert_eq!(misnumbered.numbers, ["003/2011"]);
        assert_eq!(misnumbered.date, None);
        assert_eq!(
            misnumbered.body,
            SiteBody::Pdf("https://biblio.ohada.org/doc_num.php?explnum_id=4077".into())
        );
        assert_eq!(
            misnumbered.page_url,
            "https://biblio.ohada.org/index.php?lvl=notice_display&id=6395"
        );
        assert_eq!(plain.numbers, ["016/2013"]);

        let text = format!(
            "COUR COMMUNE DE JUSTICE\nET D’ARBITRAGE\nAudience publique du 18 octobre 2001\nPOURVOI : n°012/2000/PC du 3 mai 2000\nARRET N°003/2001 du 18 octobre 2001\n{BODY}"
        );
        let parsed = parse_site_decision("ccja", misnumbered, &text, false).expect("a decision");
        assert_eq!(
            parsed.decision.numero_dossiers.as_deref(),
            Some(&["003/2001".to_string()][..])
        );
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2001-10-18"));
        assert_eq!(
            parsed.decision.jurisdiction_source_code.as_deref(),
            Some("ohada_ccja")
        );
        let text = format!(
            "COUR COMMUNE DE JUSTICE\nAudience publique du 7 mars 2013\nARRET N° 016/2013\n{BODY}"
        );
        let parsed = parse_site_decision("ccja", plain, &text, false).expect("a decision");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2013-03-07"));
        assert!(parse_site_decision("ccja", plain, BODY, false).is_err());
    }

    #[test]
    fn an_ivorian_cassation_card_gives_the_number_the_date_of_its_file_and_the_last_page() {
        let page = r#"
            <article class="stoppage-card">
                <p class="post-category" style="color: #a61d34;">INCOMPETENCE</p>
                <ul class="list-unstyled">
                    <li><strong>ARRET N°249/24</strong></li>
                    <li><strong>Objet de l'Arrêt</strong> : Acte uniforme portant procédure collective d’apurement du passif</li>
                </ul>
                <div class="border-top pt-2">
                    <a href="https://sigcc.courdecassation.ci/pdfjs/web/viewer.html?file=/uploads/stoppages/arret-n-249-24-du-05-mars-2024-1-1.pdf" class="btn" target="_blank">Visualiser l'arrêt</a>
                </div>
            </article>
            <article class="stoppage-card">
                <p class="post-category">REJET</p>
                <ul class="list-unstyled"><li><strong>ARRET N°203/24</strong></li></ul>
            </article>
            <article class="stoppage-card">
                <p class="post-category">IRRECEVABILITE</p>
                <ul class="list-unstyled"><li><strong>ARRET N°029. Pe/24</strong></li></ul>
                <a href="https://sigcc.courdecassation.ci/pdfjs/web/viewer.html?file=/uploads/stoppages/arret-n-029-pe-24-du-15-fevrier-2024-2-1.pdf">Visualiser l'arrêt</a>
            </article>
            <article class="stoppage-card">
                <p class="post-category">IRRECEVABILITE</p>
                <ul class="list-unstyled"><li><strong>ARRET N°0029. Pe/24</strong></li></ul>
                <a href="https://sigcc.courdecassation.ci/pdfjs/web/viewer.html?file=/uploads/stoppages/arret-n-029-pe-24-du-15-fevrier-2024-1.pdf">Visualiser l'arrêt</a>
            </article>
            <a href="https://eservices.courdecassation.ci/search/stoppages?matter=civ_com&amp;nb_page=7">7</a>
            <a href="https://eservices.courdecassation.ci/search/stoppages?matter=civ_com&amp;nb_page=8" class="active">8</a>"#;
        let listing = cassation_ci_listing(page);
        assert_eq!(listing.last_page, Some(8));
        assert_eq!(listing.unread.len(), 1);
        let [arret, criminal, padded] = &listing.entries[..] else {
            panic!("three cards read: {:?}", listing.entries);
        };
        assert_eq!(criminal.numbers, ["29.Pe/24"]);
        assert_eq!(padded.numbers, criminal.numbers);
        assert_eq!(criminal.date.as_deref(), Some("2024-02-15"));
        assert_eq!(arret.id, "arret-n-249-24-du-05-mars-2024-1-1");
        assert_eq!(arret.numbers, ["249/24"]);
        assert_eq!(arret.date.as_deref(), Some("2024-03-05"));
        assert_eq!(
            arret.body,
            SiteBody::Pdf(
                "https://sigcc.courdecassation.ci/uploads/stoppages/arret-n-249-24-du-05-mars-2024-1-1.pdf"
                    .into()
            )
        );
        assert_eq!(arret.fields["outcome"], "INCOMPETENCE");
        assert_eq!(
            arret.fields["subject"],
            "Acte uniforme portant procédure collective d’apurement du passif"
        );
    }

    #[test]
    fn a_text_layer_reading_digits_for_accented_letters_is_garbled_and_a_clean_one_is_not() {
        let clean = "La Cour, composée de : Gérard NIYUNGEKO, Président ; le requérant allègue la violation de l'article 7 de la Charte. ";
        let garbled = "La Gour compos6e de : G6rard NIYUNGEKO, Pr6sident ; le Requ6rant all6gue la violation de l'article 7 de la Charte. ";
        assert!(!garbled_layer(&clean.repeat(20)));
        assert!(garbled_layer(&garbled.repeat(20)));
        assert!(garbled_layer(&format!("{}{}", clean.repeat(20), garbled)));
        assert!(!garbled_layer(&format!("{}{}", clean.repeat(30), garbled)));
        assert!(!garbled_layer(""));
    }

    const CNDJ_ROWS: &str = r#"
        <table class="table table-striped"><thea><tr><th>N° et date de publication</th></tr></thea>
        <tbody>
            <tr>
                <td><span>390/2019</span> du <br><span>Mercredi 31 Juillet 2019</span></td>
                <td><span>COUR D&#039;APPEL DE COMMERCE D&#039;ABIDJAN</span></td>
                <td><span>COMMERCIALE</span></td>
                <td><span>IRRECEVABILITE DE L&#039;ACTION</span></td>
                <td><span></span></td>
                <td><a href="https://www.cndj.ci/download/decision/pdf/390-2019.pdf"><span class="fa fa-file-pdf"></span></a></td>
            </tr>
            <tr>
                <td><span>262</span> du <br><span>Jeudi 21 Mars 2019</span></td>
                <td><span>COUR D&#039;APPEL D&#039;ABIDJAN CHAMBRES SOCIALES</span></td>
                <td><span>SOCIALE</span></td>
                <td><span>LICENCIEMENT ABUSIF</span></td>
                <td><span>CHAMBRES SOCIALES</span></td>
                <td><a href="https://www.cndj.ci/download/decision/pdf/1747131083_262-2019.pdf"><span class="fa fa-file-pdf"></span></a></td>
            </tr>
            <tr>
                <td><span>458/2019</span> du <br><span>Samedi 18 Juillet 0009</span></td>
                <td><span>TRIBUNAL DE PREMIERE INSTANCE D&#039;ABIDJAN-PLATEAU</span></td>
                <td><span>CIVILE</span></td>
                <td><span>COMPETENCE</span></td>
                <td><span></span></td>
                <td><a href="https://www.cndj.ci/download/decision/pdf/458-2019.pdf"><span class="fa fa-file-pdf"></span></a></td>
            </tr>
            <tr>
                <td><span>12/2019</span> du <br><span>Lundi 07 Janvier 2019</span></td>
                <td><span>TRIBUNAL DE TRAVAIL DE MAN</span></td>
                <td><span>SOCIALE</span></td>
                <td><span>SALAIRES</span></td>
                <td><span></span></td>
                <td><a href="https://www.cndj.ci/download/decision/pdf/12-2019.pdf"><span class="fa fa-file-pdf"></span></a></td>
            </tr>
        </tbody></table>
        <a href="https://www.cndj.ci/jurisprudences/5?page=2">2</a>
        <a href="https://www.cndj.ci/jurisprudences/5?page=316">316</a>"#;

    #[test]
    fn a_cndj_row_is_its_pdf_with_the_number_and_the_court_listed() {
        let listing = cndj_ci_listing(CNDJ_ROWS);
        assert_eq!(listing.last_page, Some(316));
        assert_eq!(listing.unread.len(), 1);
        let [commerce, social, plateau] = &listing.entries[..] else {
            panic!("three rows: {:?}", listing.entries);
        };
        assert_eq!(commerce.id, "390-2019");
        assert_eq!(commerce.numbers, ["390/2019"]);
        assert_eq!(commerce.date, None);
        assert_eq!(commerce.fields["date"], "Mercredi 31 Juillet 2019");
        assert_eq!(
            commerce.fields["court"],
            "COUR D'APPEL DE COMMERCE D'ABIDJAN"
        );
        assert!(!commerce.fields.contains_key("formation"));
        assert_eq!(social.id, "1747131083_262-2019");
        assert_eq!(social.fields["formation"], "CHAMBRES SOCIALES");
        assert_eq!(
            plateau.body,
            SiteBody::Pdf("https://www.cndj.ci/download/decision/pdf/458-2019.pdf".into())
        );
    }

    #[test]
    fn a_cndj_decision_takes_the_court_and_the_date_its_text_states() {
        let listing = cndj_ci_listing(CNDJ_ROWS);
        let (commerce, social, plateau) = (
            &listing.entries[0],
            &listing.entries[1],
            &listing.entries[2],
        );
        let judgment = format!(
            "REPUBLIQUE DE CÔTE D'IVOIRE\nCOUR D'APPEL DE\nCOMMERCE D'ABIDJAN\nTRIBUNAL DE COMMERCE\nD'ABIDJAN\n\
             RG N° 0570/2018\nJUGEMENT CONTRADICTOIRE\nDu 24/01/2019\n\
             AUDIENCE PUBLIQUE ORDINAIRE DU 24 JANVIER 2019\n\
             Le Tribunal de Commerce d'Abidjan, en son audience publique ordinaire du jeudi vingt \
             quatre janvier de l'an deux mil dix-neuf. {BODY}"
        );
        let parsed = parse_site_decision("cndj-ci", social, &judgment, true).expect("a judgment");
        assert_eq!(parsed.court.code, "ci_tribunal_commerce_abidjan");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2019-01-24"));
        assert_eq!(
            parsed.source_fields["court"],
            "ci_tribunal_commerce_abidjan"
        );
        let rebuilt = from_source_fields_site(
            &parsed.decision.texte_integral_clean,
            &parsed.source_fields,
            &parsed.decision.source_uid,
        );
        assert_eq!(
            rebuilt.jurisdiction_source_code.as_deref(),
            Some("ci_tribunal_commerce_abidjan")
        );
        let arret = format!(
            "COUR D'APPEL D'ABIDJAN COTE D'IVOIRE\nN° 862/18\nDU 21/12/18\nAUDIENCE DU VENDREDI 21/12/2018\n\
             Infirme le jugement du Tribunal de première instance d'Abidjan-Plateau.\n\
             La Cour d'Appel d'Abidjan, 1ère Chambre Civile, séant au Palais de justice de ladite \
             ville, en son audience publique ordinaire du Vendredi vingt et un décembre deux mil \
             dix huit. {BODY}"
        );
        let named = |id: &str| SiteEntry {
            id: id.into(),
            ..plateau.clone()
        };
        let parsed =
            parse_site_decision("cndj-ci", &named("862-2018"), &arret, true).expect("an arrêt");
        assert_eq!(parsed.court.code, "ci_cour_appel_abidjan");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2018-12-21"));
        let headed = format!(
            "COUR D'APPEL D'ABIDJAN\nTRIBUNAL DE COMMERCE D'ABIDJAN\n\
             AUDIENCE PUBLIQUE ORDINAIRE DU 17 AVRIL 2019\n{BODY}"
        );
        assert_eq!(
            parse_site_decision("cndj-ci", commerce, &headed, true)
                .expect("a heading")
                .court
                .code,
            "ci_tribunal_commerce_abidjan"
        );
        let misread = format!(
            "N° 793\nDU 23/11/2018\nARRET CIVIL\nCOUR D'APPEL D'ABIDJAN\n\
             AUDIENCE DU VENDRED 23 NOVEMBRE 2018\n{BODY}"
        );
        assert_eq!(
            parse_site_decision("cndj-ci", &named("1744287428_793-2018"), &misread, true)
                .expect("its heading's date")
                .decision
                .date_lecture
                .as_deref(),
            Some("2018-11-23")
        );
        let spelled = format!(
            "COUR D'APPEL D'ABIDJAN COTE D'IVOIRE\nSIXIEME CHAMBRE CIVILE\nAUDIENCE DU MARD 26 FEVRIER 2019\n\
             La Cour d'Appel d'Abidjan, 6ème Chambre civile, séant au Palais de justice de ladite \
             ville, en son audience publique ordinaire du mardi vingt- six mars deux mil dixneuf à \
             laquelle siégeaient ; a assigné madame X à comparaître par devant la Cour de ce siège à \
             l'audience du 06 octobre 2017. {BODY}"
        );
        assert_eq!(
            parse_site_decision("cndj-ci", plateau, &spelled, true)
                .expect("its speaking line's date")
                .decision
                .date_lecture
                .as_deref(),
            Some("2019-03-26")
        );
        let stale = format!(
            "N° 447\nDU 23/04/2019\nCOUR D'APPEL D'ABIDJAN\nAUDIENCE DU MARDI 23 AVRIL 2019\n\
             La Cour, en son audience publique ordinaire du vendredi sept décembre deux mil dix \
             huit à laquelle siégeaient. {BODY}"
        );
        assert_eq!(
            parse_site_decision("cndj-ci", &named("447-23042019"), &stale, true)
                .expect("its heading's date")
                .decision
                .date_lecture
                .as_deref(),
            Some("2019-04-23")
        );
        let unnamed = format!("AUDIENCE PUBLIQUE DU 17 AVRIL 2019\n{BODY}");
        assert_eq!(
            parse_site_decision("cndj-ci", commerce, &unnamed, true)
                .expect("its row's court")
                .court
                .code,
            "ci_cour_appel_commerce_abidjan"
        );
        assert!(parse_site_decision("cndj-ci", plateau, BODY, true).is_err());
    }

    #[test]
    fn a_moroccan_constitutional_decision_is_its_body_s_in_arabic_with_its_case_files() {
        let listing = cc_ma_listing(
            r#"{"draw":"1","recordsTotal":2159,"data":[
                {"iddecision":2158,"numerodecision":"294/26","dossiers":"ملفان عدد : <b>552/26</b> و<b>528/26</b>","datedecision":"2026-11-04T00:00:00"},
                {"iddecision":2157,"numerodecision":"","dossiers":"","datedecision":"2026-11-03T00:04:53"}]}"#,
        )
        .expect("a listing");
        assert_eq!((listing.total, listing.last_page), (Some(2159), Some(22)));
        assert_eq!(listing.unread, ["2157"]);
        let entry = &listing.entries[0];
        assert_eq!(entry.numbers, ["294/26", "552/26", "528/26"]);
        assert_eq!(entry.date.as_deref(), Some("2026-11-04"));
        let page = r#"<div id="decision-detail"><div class="wrap-title">
            <h2 class="title">&#x642;&#x631;&#x627;&#x631;&#x627;&#x62A; &#x627;&#x644;&#x645;&#x62C;&#x644;&#x633; &#x627;&#x644;&#x62F;&#x633;&#x62A;&#x648;&#x631;&#x64A;</h2></div>
            <input type="hidden" id="decision-content" value="&lt;p&gt;نسخة&lt;/p&gt;" />
            <div class="col decision-content article-content" data-db-content="true" role="document">
            <div xmlns="http://www.w3.org/1999/xhtml"><p dir="rtl">ﺍﻟﻤﺠﻠﺲ ﺍﻟﺪﺳﺘﻮﺭﻱ، بعد الاطلاع على العريضة المسجلة بالأمانة العامة للمجلس الدستوري، وبناء على الدستور وعلى القانون التنظيمي المتعلق بالمجلس الدستوري، وبعد الاستماع إلى تقرير العضو المقرر والمداولة طبق القانون؛</p>
            <p>لهذه الأسباب: يصرح بأن الطلب غير مقبول؛ ويأمر بتبليغ نسخة من قراره هذا إلى الأطراف وبنشره في الجريدة الرسمية.</p></div>
            <div class="d-flex justify-content-between align-items-center"><a href="/Decision?id=2157">التالي</a></div>"#;
        let (text, fields) = cc_ma_page(page).expect("a decision page");
        assert!(!text.contains("نسخة</p>") && !text.contains("التالي"));
        let mut entry = entry.clone();
        entry.fields.extend(fields);
        let parsed =
            parse_site_decision("cc-ma", &entry, &text, false).expect("an Arabic decision");
        assert_eq!(parsed.court.code, "ma_conseil_constitutionnel");
        assert_eq!(parsed.source_fields["lang"], "ara");
        assert!(parsed
            .decision
            .texte_integral_clean
            .starts_with("المجلس الدستوري، بعد"));
        assert!(parse_site_decision("cc-ma", &entry, BODY, false).is_err());
    }
}
