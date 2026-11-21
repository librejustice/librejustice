//! A juportal.be decision page into a [`Decision`] and its `source_fields`: the court from the
//! ECLI the page states, the date from its heading, the dockets and summaries from its fieldsets,
//! the text from its « Texte de la décision », or from the same fieldsets of a page in Dutch. And the
//! ECLIs an RSS feed of the portal lists.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

use crate::entscheidsuche::language;

/// A Belgian court the load takes, by the court segment of its ECLIs.
#[derive(Debug)]
pub struct BelgianCourt {
    pub ecli: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
    /// Its arrêts are cited by the number their ECLI ends on, `ARR.72.281`.
    pub numbered: bool,
}

pub const BELGIAN_PLACE: &str = "geo:BE";

const fn court(
    ecli: &'static str,
    jurisdiction_type: &'static str,
    code: &'static str,
    label: &'static str,
) -> BelgianCourt {
    BelgianCourt {
        ecli,
        jurisdiction_type,
        code,
        label,
        numbered: false,
    }
}

const APPEL: &str = "COUR_APPEL";
const INSTANCE: &str = "TRIBUNAL_PREMIERE_INSTANCE";
const ENTREPRISE: &str = "TRIBUNAL_COMMERCE";

/// The courts whose French decisions the ECLI search engine lists for Belgium.
pub const BELGIAN_COURTS: [BelgianCourt; 39] = [
    court(
        "CASS",
        "COUR_CASSATION",
        "be_cour_cassation",
        "Cour de cassation (Belgique)",
    ),
    court(
        "GHCC",
        "COUR_CONSTITUTIONNELLE",
        "be_cour_constitutionnelle",
        "Cour constitutionnelle (Belgique)",
    ),
    BelgianCourt {
        ecli: "RVSCE",
        jurisdiction_type: "COUR_ADMINISTRATIVE",
        code: "be_cour_administrative_conseil_etat",
        label: "Conseil d'État (Belgique)",
        numbered: true,
    },
    court(
        "CABRL",
        APPEL,
        "be_cour_appel_bruxelles",
        "Cour d'appel de Bruxelles (Belgique)",
    ),
    court(
        "CALIE",
        APPEL,
        "be_cour_appel_liege",
        "Cour d'appel de Liège (Belgique)",
    ),
    court(
        "CAMON",
        APPEL,
        "be_cour_appel_mons",
        "Cour d'appel de Mons (Belgique)",
    ),
    court(
        "HBANT",
        APPEL,
        "be_cour_appel_anvers",
        "Cour d'appel d'Anvers (Belgique)",
    ),
    court(
        "HBGNT",
        APPEL,
        "be_cour_appel_gand",
        "Cour d'appel de Gand (Belgique)",
    ),
    court(
        "CTBRL",
        APPEL,
        "be_cour_appel_bruxelles_travail",
        "Cour du travail de Bruxelles (Belgique)",
    ),
    court(
        "CTLIE",
        APPEL,
        "be_cour_appel_liege_travail",
        "Cour du travail de Liège (Belgique)",
    ),
    court(
        "CTMON",
        APPEL,
        "be_cour_appel_mons_travail",
        "Cour du travail de Mons (Belgique)",
    ),
    court(
        "AHANT",
        APPEL,
        "be_cour_appel_anvers_travail",
        "Cour du travail d'Anvers (Belgique)",
    ),
    court(
        "AHGNT",
        APPEL,
        "be_cour_appel_gand_travail",
        "Cour du travail de Gand (Belgique)",
    ),
    court(
        "PIBRL",
        INSTANCE,
        "be_tribunal_premiere_instance_bruxelles_francophone",
        "Tribunal de première instance francophone de Bruxelles (Belgique)",
    ),
    court(
        "EABRL",
        INSTANCE,
        "be_tribunal_premiere_instance_bruxelles_neerlandophone",
        "Tribunal de première instance néerlandophone de Bruxelles (Belgique)",
    ),
    court(
        "PILIE",
        INSTANCE,
        "be_tribunal_premiere_instance_liege",
        "Tribunal de première instance de Liège (Belgique)",
    ),
    court(
        "PIHAI",
        INSTANCE,
        "be_tribunal_premiere_instance_hainaut",
        "Tribunal de première instance du Hainaut (Belgique)",
    ),
    court(
        "PINAM",
        INSTANCE,
        "be_tribunal_premiere_instance_namur",
        "Tribunal de première instance de Namur (Belgique)",
    ),
    court(
        "PILUX",
        INSTANCE,
        "be_tribunal_premiere_instance_luxembourg",
        "Tribunal de première instance du Luxembourg (Belgique)",
    ),
    court(
        "PIBRW",
        INSTANCE,
        "be_tribunal_premiere_instance_brabant_wallon",
        "Tribunal de première instance du Brabant wallon (Belgique)",
    ),
    court(
        "EAANT",
        INSTANCE,
        "be_tribunal_premiere_instance_anvers",
        "Tribunal de première instance d'Anvers (Belgique)",
    ),
    court(
        "EAOVL",
        INSTANCE,
        "be_tribunal_premiere_instance_flandre_orientale",
        "Tribunal de première instance de Flandre orientale (Belgique)",
    ),
    court(
        "EAWVL",
        INSTANCE,
        "be_tribunal_premiere_instance_flandre_occidentale",
        "Tribunal de première instance de Flandre occidentale (Belgique)",
    ),
    court(
        "EALIM",
        INSTANCE,
        "be_tribunal_premiere_instance_limbourg",
        "Tribunal de première instance du Limbourg (Belgique)",
    ),
    court(
        "EALEU",
        INSTANCE,
        "be_tribunal_premiere_instance_louvain",
        "Tribunal de première instance de Louvain (Belgique)",
    ),
    court(
        "TREUP",
        INSTANCE,
        "be_tribunal_premiere_instance_eupen",
        "Tribunal d'Eupen (Belgique)",
    ),
    court(
        "TTBRL",
        INSTANCE,
        "be_tribunal_premiere_instance_bruxelles_francophone_travail",
        "Tribunal du travail francophone de Bruxelles (Belgique)",
    ),
    court(
        "ARBRL",
        INSTANCE,
        "be_tribunal_premiere_instance_bruxelles_neerlandophone_travail",
        "Tribunal du travail néerlandophone de Bruxelles (Belgique)",
    ),
    court(
        "TTLIE",
        INSTANCE,
        "be_tribunal_premiere_instance_liege_travail",
        "Tribunal du travail de Liège (Belgique)",
    ),
    court(
        "TTHAI",
        INSTANCE,
        "be_tribunal_premiere_instance_hainaut_travail",
        "Tribunal du travail du Hainaut (Belgique)",
    ),
    court(
        "TTBRW",
        INSTANCE,
        "be_tribunal_premiere_instance_brabant_wallon_travail",
        "Tribunal du travail du Brabant wallon (Belgique)",
    ),
    court(
        "ARANT",
        INSTANCE,
        "be_tribunal_premiere_instance_anvers_travail",
        "Tribunal du travail d'Anvers (Belgique)",
    ),
    court(
        "ARGNT",
        INSTANCE,
        "be_tribunal_premiere_instance_gand_travail",
        "Tribunal du travail de Gand (Belgique)",
    ),
    court(
        "ARLEU",
        INSTANCE,
        "be_tribunal_premiere_instance_louvain_travail",
        "Tribunal du travail de Louvain (Belgique)",
    ),
    court(
        "TELIE",
        ENTREPRISE,
        "be_tribunal_commerce_liege",
        "Tribunal de l'entreprise de Liège (Belgique)",
    ),
    court(
        "TEHAI",
        ENTREPRISE,
        "be_tribunal_commerce_hainaut",
        "Tribunal de l'entreprise du Hainaut (Belgique)",
    ),
    court(
        "TEBRW",
        ENTREPRISE,
        "be_tribunal_commerce_brabant_wallon",
        "Tribunal de l'entreprise du Brabant wallon (Belgique)",
    ),
    court(
        "ORBRL",
        ENTREPRISE,
        "be_tribunal_commerce_bruxelles_neerlandophone",
        "Tribunal de l'entreprise néerlandophone de Bruxelles (Belgique)",
    ),
    court(
        "ORGNT",
        ENTREPRISE,
        "be_tribunal_commerce_gand",
        "Tribunal de l'entreprise de Gand (Belgique)",
    ),
];

/// Each court's Dutch name, the one its Dutch-speaking readers know it by.
pub fn juportal_names() -> Vec<(&'static str, Value)> {
    BELGIAN_COURTS
        .iter()
        .map(|court| {
            let name = match court.code {
                "be_cour_cassation" => "Hof van Cassatie",
                "be_cour_constitutionnelle" => "Grondwettelijk Hof",
                "be_cour_administrative_conseil_etat" => "Raad van State",
                "be_cour_appel_bruxelles" => "Hof van beroep Brussel",
                "be_cour_appel_liege" => "Hof van beroep Luik",
                "be_cour_appel_mons" => "Hof van beroep Bergen",
                "be_cour_appel_anvers" => "Hof van beroep Antwerpen",
                "be_cour_appel_gand" => "Hof van beroep Gent",
                "be_cour_appel_bruxelles_travail" => "Arbeidshof Brussel",
                "be_cour_appel_liege_travail" => "Arbeidshof Luik",
                "be_cour_appel_mons_travail" => "Arbeidshof Bergen",
                "be_cour_appel_anvers_travail" => "Arbeidshof Antwerpen",
                "be_cour_appel_gand_travail" => "Arbeidshof Gent",
                "be_tribunal_premiere_instance_bruxelles_francophone" => {
                    "Franstalige rechtbank van eerste aanleg Brussel"
                }
                "be_tribunal_premiere_instance_bruxelles_neerlandophone" => {
                    "Nederlandstalige rechtbank van eerste aanleg Brussel"
                }
                "be_tribunal_premiere_instance_liege" => "Rechtbank van eerste aanleg Luik",
                "be_tribunal_premiere_instance_hainaut" => "Rechtbank van eerste aanleg Henegouwen",
                "be_tribunal_premiere_instance_namur" => "Rechtbank van eerste aanleg Namen",
                "be_tribunal_premiere_instance_luxembourg" => {
                    "Rechtbank van eerste aanleg Luxemburg"
                }
                "be_tribunal_premiere_instance_brabant_wallon" => {
                    "Rechtbank van eerste aanleg Waals-Brabant"
                }
                "be_tribunal_premiere_instance_anvers" => "Rechtbank van eerste aanleg Antwerpen",
                "be_tribunal_premiere_instance_flandre_orientale" => {
                    "Rechtbank van eerste aanleg Oost-Vlaanderen"
                }
                "be_tribunal_premiere_instance_flandre_occidentale" => {
                    "Rechtbank van eerste aanleg West-Vlaanderen"
                }
                "be_tribunal_premiere_instance_limbourg" => "Rechtbank van eerste aanleg Limburg",
                "be_tribunal_premiere_instance_louvain" => "Rechtbank van eerste aanleg Leuven",
                "be_tribunal_premiere_instance_eupen" => "Rechtbank van eerste aanleg Eupen",
                "be_tribunal_premiere_instance_bruxelles_francophone_travail" => {
                    "Franstalige arbeidsrechtbank Brussel"
                }
                "be_tribunal_premiere_instance_bruxelles_neerlandophone_travail" => {
                    "Nederlandstalige arbeidsrechtbank Brussel"
                }
                "be_tribunal_premiere_instance_liege_travail" => "Arbeidsrechtbank Luik",
                "be_tribunal_premiere_instance_hainaut_travail" => "Arbeidsrechtbank Henegouwen",
                "be_tribunal_premiere_instance_brabant_wallon_travail" => {
                    "Arbeidsrechtbank Waals-Brabant"
                }
                "be_tribunal_premiere_instance_anvers_travail" => "Arbeidsrechtbank Antwerpen",
                "be_tribunal_premiere_instance_gand_travail" => "Arbeidsrechtbank Gent",
                "be_tribunal_premiere_instance_louvain_travail" => "Arbeidsrechtbank Leuven",
                "be_tribunal_commerce_liege" => "Ondernemingsrechtbank Luik",
                "be_tribunal_commerce_hainaut" => "Ondernemingsrechtbank Henegouwen",
                "be_tribunal_commerce_brabant_wallon" => "Ondernemingsrechtbank Waals-Brabant",
                "be_tribunal_commerce_bruxelles_neerlandophone" => {
                    "Nederlandstalige ondernemingsrechtbank Brussel"
                }
                "be_tribunal_commerce_gand" => "Ondernemingsrechtbank Gent",
                other => panic!("a Belgian court without a Dutch name: {other}"),
            };
            (
                court.code,
                json!({ "nl": { "name": format!("{name} (België)") } }),
            )
        })
        .collect()
}

pub fn belgian_court(ecli_court: &str) -> Option<&'static BelgianCourt> {
    BELGIAN_COURTS.iter().find(|c| c.ecli == ecli_court)
}

/// The court segment and the kind of document an ECLI names: `ECLI:BE:CASS:2021:ARR.20211117.2F.6`
/// is `CASS`, `ARR`.
pub fn ecli_court_and_kind(ecli: &str) -> Option<(&str, &str)> {
    let mut parts = ecli.strip_prefix("ECLI:BE:")?.split(':');
    let court = parts.next()?;
    let _year = parts.next()?;
    let kind = parts.next()?.split('.').next()?;
    Some((court, kind))
}

/// Judgments and arrêts; conclusions, opinions, orders and the bodies' decisions stay out.
const TAKEN_KINDS: [&str; 2] = ["ARR", "JUG"];

/// An ECLI of a court of [`BELGIAN_COURTS`] naming a judgment or an arrêt.
pub fn juportal_taken(ecli: &str) -> bool {
    ecli_court_and_kind(ecli)
        .is_some_and(|(court, kind)| belgian_court(court).is_some() && TAKEN_KINDS.contains(&kind))
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("juportal regex"))
}

/// A decision page in any of the portal's languages: its form posts back to the ECLI it shows.
pub fn is_juportal_page(html: &str) -> bool {
    html.contains("action=\"/content/ECLI:")
}

/// The language of a decision page, `fra` or `nld`, by the label of its ECLI row.
pub fn juportal_page_lang(html: &str) -> Option<&'static str> {
    let (_, head) = fieldsets(html).into_iter().next()?;
    let labels = rows(head);
    [&FRENCH, &DUTCH]
        .into_iter()
        .find(|l| labels.iter().any(|(label, _)| label == l.ecli))
        .map(|l| l.lang)
}

/// The ECLIs an RSS feed of the portal lists, in its order.
pub fn juportal_feed(xml: &str) -> Result<Vec<String>> {
    static GUID: OnceLock<Regex> = OnceLock::new();
    if !xml.contains("<rss") || !xml.contains("<channel>") {
        return Err(CoreError::Xml("juportal feed: no RSS channel".into()));
    }
    Ok(regex(&GUID, r"<guid[^>]*>\s*(ECLI:[^<\s]+)\s*</guid>")
        .captures_iter(xml)
        .map(|c| c[1].to_string())
        .collect())
}

/// The text of an HTML fragment, a line per paragraph or break, its entities decoded.
fn text_of(html: &str) -> String {
    static BREAK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let broken = regex(&BREAK, r"(?i)<br\s*/?>|</?p\b[^>]*>|</div>").replace_all(html, "\n");
    let bare = regex(&TAG, r"<[^>]*>").replace_all(&broken, "");
    // Some pages carry a NUL, which Postgres refuses in a text column.
    html_escape::decode_html_entities(&bare)
        .replace('\0', "")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each `<fieldset>` of the page: its legend, then its inner HTML.
fn fieldsets(html: &str) -> Vec<(String, &str)> {
    static FIELDSET: OnceLock<Regex> = OnceLock::new();
    regex(
        &FIELDSET,
        r"(?s)<fieldset[^>]*>\s*<legend[^>]*>(.*?)</legend>(.*?)</fieldset>",
    )
    .captures_iter(html)
    .map(|c| {
        (
            text_of(c.get(1).expect("legend").as_str()),
            c.get(2).expect("body").as_str(),
        )
    })
    .collect()
}

/// The rows of a fieldset's table: the label without its colon, then the value's HTML.
fn rows(html: &str) -> Vec<(String, &str)> {
    static ROW: OnceLock<Regex> = OnceLock::new();
    regex(
        &ROW,
        r"(?s)<tr>\s*<td>\s*<p[^>]*>\s*([^<]+?)\s*:\s*</p>\s*</td>\s*<td>(.*?)</td>\s*</tr>",
    )
    .captures_iter(html)
    .map(|c| {
        (
            text_of(c.get(1).expect("label").as_str()),
            c.get(2).expect("value").as_str(),
        )
    })
    .collect()
}

/// The inner HTML of the first `<div` of a fieldset, to its last `</div>`.
fn first_div(html: &str) -> Option<&str> {
    let open = html.find("<div")?;
    let inner = open + html[open..].find('>')? + 1;
    let close = html.rfind("</div>")?;
    (close > inner).then(|| &html[inner..close])
}

/// « 17 novembre 2021 », « 1er mars 1998 » or « 12 februari 2015 », at the end of a heading.
fn heading_date(heading: &str, months: &[&str; 12]) -> Option<String> {
    static DATE: OnceLock<Regex> = OnceLock::new();
    let c = regex(&DATE, r"(\d{1,2})(?:er)?\s+(\p{L}+)\s+(\d{4})\s*$").captures(heading)?;
    let month = months.iter().position(|m| *m == c[2].to_lowercase())? + 1;
    let date = format!("{}-{month:02}-{:0>2}", &c[3], &c[1]);
    date.parse::<jiff::civil::Date>().ok()?;
    Some(date)
}

/// The labels of a decision page in one of the portal's languages.
struct Labels {
    lang: &'static str,
    path: &'static str,
    ecli: &'static str,
    dockets: [&'static str; 2],
    replaces: &'static str,
    case: &'static str,
    chamber: &'static str,
    field: &'static str,
    introduced: &'static str,
    text: &'static str,
    date_from: &'static str,
    months: [&'static str; 12],
}

const FRENCH: Labels = Labels {
    lang: "fra",
    path: "FR",
    ecli: "No ECLI",
    dockets: ["No Rôle", "No Arrêt/No Rôle"],
    replaces: "Remplace le numéro",
    case: "Affaire",
    chamber: "Chambre",
    field: "Domaine juridique",
    introduced: "Date d'introduction",
    text: "Texte de la décision",
    date_from: " du ",
    months: [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
};

const DUTCH: Labels = Labels {
    lang: "nld",
    path: "NL",
    ecli: "ECLI nr",
    dockets: ["Rolnummer", "Arrest- Rolnummer"],
    replaces: "Vervangt nummer",
    case: "Zaak",
    chamber: "Kamer",
    field: "Rechtsgebied",
    introduced: "Invoerdatum",
    text: "Tekst van de beslissing",
    date_from: " van ",
    months: [
        "januari",
        "februari",
        "maart",
        "april",
        "mei",
        "juni",
        "juli",
        "augustus",
        "september",
        "oktober",
        "november",
        "december",
    ],
};

/// The language a text reads in, `fra`, `nld` or another the detector names: Dutch when it holds
/// more Dutch function words than French ones.
fn text_language(text: &str) -> &'static str {
    const DUTCH_WORDS: [&str; 12] = [
        "het", "een", "van", "dat", "niet", "voor", "zijn", "wordt", "deze", "worden", "bij", "aan",
    ];
    const FRENCH_WORDS: [&str; 12] = [
        "le", "les", "du", "et", "que", "qui", "pour", "dans", "est", "par", "sur", "aux",
    ];
    let (mut dutch, mut french) = (0usize, 0usize);
    for word in text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .take(5_000)
    {
        let word = word.to_lowercase();
        if DUTCH_WORDS.contains(&word.as_str()) {
            dutch += 1;
        } else if FRENCH_WORDS.contains(&word.as_str()) {
            french += 1;
        }
    }
    if dutch > french {
        "nld"
    } else {
        language(text)
    }
}

#[derive(Debug)]
pub struct JuportalParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static BelgianCourt,
}

/// The non-blank characters a decision text holds at least.
const DECISION_FLOOR: usize = 200;

fn decision_of(court: &BelgianCourt, source_fields: &Value, text: String) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let ecli = field("ecli").expect("a juportal decision states its ECLI");
    let dockets: Vec<String> = source_fields
        .get("dockets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let source_uid = match field("lang") {
        Some("nld") => format!("juportal/{ecli}/nl"),
        _ => format!("juportal/{ecli}"),
    };
    Decision {
        source_uid,
        member_name: ecli.to_string(),
        ecli: Some(ecli.to_string()),
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber: field("chamber").map(str::to_string),
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

/// Parses a decision page in French or in Dutch, its labels saying which; a Dutch page is the
/// provenance `juportal/{ECLI}/nl`. A page in another language, one whose text reads in another
/// language than its labels, one of a court or a kind the load does not take, and one without a
/// decision text are refused.
pub fn parse_juportal(html: &str) -> Result<JuportalParsed> {
    let fail = |why: String| CoreError::Xml(format!("juportal: {why}"));
    let sets = fieldsets(html);
    let (heading, head) = sets.first().ok_or_else(|| fail("no fieldset".into()))?;
    let head_rows = rows(head);
    let value = |label: &str| {
        head_rows
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| text_of(v))
    };
    let labels = [&FRENCH, &DUTCH]
        .into_iter()
        .find(|labels| value(labels.ecli).is_some())
        .ok_or_else(|| fail("no ECLI row, a page neither French nor Dutch".into()))?;
    let ecli = value(labels.ecli).expect("the row the labels were found by");
    let (court_segment, kind) =
        ecli_court_and_kind(&ecli).ok_or_else(|| fail(format!("ECLI {ecli} unread")))?;
    let court = belgian_court(court_segment)
        .ok_or_else(|| fail(format!("{ecli}: court {court_segment} not taken")))?;
    if !TAKEN_KINDS.contains(&kind) {
        return Err(fail(format!("{ecli}: kind {kind} not taken")));
    }
    let date = heading_date(heading, &labels.months)
        .ok_or_else(|| fail(format!("{ecli}: no date in {heading:?}")))?;
    let text = sets
        .iter()
        .find(|(legend, _)| legend.starts_with(labels.text))
        .and_then(|(_, body)| first_div(body))
        .map(text_of)
        .unwrap_or_default();
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(fail(format!("{ecli}: no decision text")));
    }
    let lang = text_language(&text);
    if lang != labels.lang {
        return Err(fail(format!(
            "{ecli}: a text in {lang} under {} labels",
            labels.lang
        )));
    }
    let mut dockets: Vec<String> = Vec::new();
    if court.numbered {
        if let Some((_, number)) = ecli.rsplit(':').next().and_then(|t| t.split_once('.')) {
            dockets.push(number.to_string());
        }
    }
    for docket in head_rows
        .iter()
        .filter(|(l, _)| labels.dockets.contains(&l.as_str()))
        .flat_map(|(_, v)| {
            text_of(v)
                .split(['\n', ';'])
                .map(|d| d.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|d| !d.is_empty() && d != "N/A")
    {
        if !dockets.contains(&docket) {
            dockets.push(docket);
        }
    }
    let replaces: Vec<String> = head_rows
        .iter()
        .filter(|(l, _)| l == labels.replaces)
        .flat_map(|(_, v)| {
            text_of(v)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    let summaries: Vec<String> = sets
        .iter()
        .filter(|(legend, _)| legend.starts_with("Fiche"))
        .filter_map(|(_, body)| first_div(body).map(text_of))
        .filter(|s| !s.is_empty())
        .collect();
    let kind_label = heading
        .split_once(labels.date_from)
        .map(|(k, _)| k.trim().to_string());
    let source_fields = json!({
        "ecli": ecli,
        "replaces": replaces,
        "court": court.ecli,
        "kind": kind_label,
        "date": date,
        "dockets": dockets,
        "case": value(labels.case),
        "chamber": value(labels.chamber),
        "field": value(labels.field).filter(|f| !f.is_empty()),
        "introduced": value(labels.introduced),
        "summaries": summaries,
        "source_url": format!("https://juportal.be/content/{ecli}/{}", labels.path),
        "lang": lang,
    });
    let decision = decision_of(court, &source_fields, text);
    Ok(JuportalParsed {
        decision,
        source_fields,
        court,
    })
}

/// Branch of [`Decision::from_source_fields`] for `juportal/`, on what the parse stored.
pub(crate) fn from_source_fields_juportal(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let court = source_fields
        .get("court")
        .and_then(Value::as_str)
        .and_then(belgian_court)
        .unwrap_or_else(|| panic!("stored juportal court not taken ({source_uid})"));
    decision_of(court, source_fields, full_text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "LA COUR,<br>\n\tVu l&apos;arrêt attaqué rendu le 30 juin 2021 par la cour \
        d'appel de Mons, chambre correctionnelle.<br>\n\tLe demandeur invoque un moyen dans un \
        mémoire annexé au présent arrêt.<br>\n\tSur le moyen : le moyen est pris de la violation \
        de l'article 12ter de la loi du 28 février 1882 sur la chasse, et les défendeurs \
        soutiennent qu'il est dénué d'intérêt pour les faits qui sont établis.<br>\n\tPAR CES \
        MOTIFS, LA COUR casse l'arrêt attaqué et renvoie la cause à la cour d'appel de \
        Bruxelles.";

    fn row(label: &str, value: &str) -> String {
        format!(
            "<tr>\n<td><p class=\"QUZx\">{label}:</p></td>\n<td><p class=\"QUZy\">{value}</p>\n</td>\n</tr>\n"
        )
    }

    fn page(heading: &str, rows: &[String], text: &str) -> String {
        page_in("Texte de la décision", heading, rows, text)
    }

    fn page_in(text_legend: &str, heading: &str, rows: &[String], text: &str) -> String {
        format!(
            "<form id=\"form_table\" name=\"detail\" action=\"/content/ECLI:BE:X/FR\" method=\"post\">\
             <fieldset>\n<legend title=\"\">{heading}            </legend>\n<table><tbody>\n{}</tbody></table>\n</fieldset>\
             <fieldset id=\"QUZf-1\" >\n<legend title=\"\">Fiche 1            </legend>\n\
             <div class=\"QUZd\">\n<p> L&apos;article 12ter n&apos;autorise le nourrissage \n que sous deux conditions.\n</p>\n</div>\n\
             <table><tbody>\n{}</tbody></table>\n</fieldset>\
             <fieldset id=\"QUZt\">\n<legend title=\"{heading}\">{text_legend}            </legend>\n\
             <div id=\"QUZd\">\n<p>\n{text}</p>\n</div>\n</fieldset></form>",
            rows.concat(),
            row("Thésaurus Cassation", "CHASSE"),
        )
    }

    #[test]
    fn a_cassation_page_gives_the_ecli_it_states_its_docket_chamber_and_summary() {
        let html = page(
            "Jugement/arrêt du 17 novembre 2021",
            &[
                row("No ECLI", "ECLI:BE:CASS:2021:ARR.20211117.2F.6"),
                row("Remplace le numéro", "ECLI:BE:CASS:2021:ARR.20211103.2F.5 "),
                row("No Rôle", "P.21.1147.F"),
                row("Chambre", "2F - deuxième chambre\n      "),
                row("Domaine juridique", "\n Droit pénal"),
                row("Date d'introduction", "2021-11-23"),
                row("Consultations", "867 - dernière vue 2026-11-14 03:04"),
            ],
            TEXT,
        );
        assert!(is_juportal_page(&html));
        let parsed = parse_juportal(&html).unwrap();
        let d = &parsed.decision;
        assert_eq!(d.source_uid, "juportal/ECLI:BE:CASS:2021:ARR.20211117.2F.6");
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:BE:CASS:2021:ARR.20211117.2F.6")
        );
        assert_eq!(
            d.jurisdiction_source_code.as_deref(),
            Some("be_cour_cassation")
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2021-11-17"));
        assert_eq!(d.numero_dossiers, Some(vec!["P.21.1147.F".to_string()]));
        assert_eq!(d.chamber.as_deref(), Some("2F - deuxième chambre"));
        assert!(d
            .texte_integral_clean
            .starts_with("LA COUR,\nVu l'arrêt attaqué"));
        assert!(!d.texte_integral_clean.contains('\t'));
        let sf = &parsed.source_fields;
        assert_eq!(
            sf["replaces"],
            json!(["ECLI:BE:CASS:2021:ARR.20211103.2F.5"])
        );
        assert_eq!(sf["field"], "Droit pénal");
        assert_eq!(sf["kind"], "Jugement/arrêt");
        assert_eq!(
            sf["summaries"],
            json!(["L'article 12ter n'autorise le nourrissage\nque sous deux conditions."])
        );
        assert!(sf.get("Consultations").is_none());
        let rebuilt = from_source_fields_juportal(&d.texte_integral_clean, sf, &d.source_uid);
        assert_eq!(rebuilt.numero_dossiers, d.numero_dossiers);
        assert_eq!(rebuilt.ecli, d.ecli);
        assert_eq!(rebuilt.date_lecture, d.date_lecture);
    }

    #[test]
    fn a_council_of_state_arret_is_numbered_from_its_ecli_before_its_roll_number() {
        let html = page(
            "Jugement/arrêt du 06 mars 1998",
            &[
                row("No ECLI", "ECLI:BE:RVSCE:1998:ARR.72.281"),
                row("No Rôle", "A. 77149/VIII-731"),
            ],
            TEXT,
        );
        let d = parse_juportal(&html).unwrap().decision;
        assert_eq!(
            d.numero_dossiers,
            Some(vec!["72.281".to_string(), "A. 77149/VIII-731".to_string()])
        );
        assert_eq!(
            d.jurisdiction_source_code.as_deref(),
            Some("be_cour_administrative_conseil_etat")
        );
    }

    #[test]
    fn a_missing_roll_number_leaves_no_docket() {
        let html = page(
            "Jugement/arrêt du 21 janvier 1833",
            &[
                row("No ECLI", "ECLI:BE:CASS:1833:ARR.18330121.1"),
                row("No Rôle", "N/A"),
            ],
            TEXT,
        );
        assert_eq!(
            parse_juportal(&html).unwrap().decision.numero_dossiers,
            None
        );
    }

    #[test]
    fn a_dutch_page_is_the_dutch_provenance_of_its_ecli() {
        let text = "HET HOF,<br>\n\tGelet op het bestreden arrest, op 30 juni 2021 gewezen door het hof \
            van beroep te Gent.<br>\n\tDe eiser voert een middel aan in een memorie die aan dit arrest \
            is gehecht.<br>\n\tHet middel is niet ontvankelijk voor zover het opkomt tegen een \
            beslissing die niet door de appelrechters werd gewezen, en de verweerders voeren aan dat \
            het middel van belang is ontbloot.<br>\n\tOM DIE REDENEN, HET HOF verwerpt het cassatieberoep.";
        let html = page_in(
            "Tekst van de beslissing",
            "Vonnis/arrest van 16 oktober 2025",
            &[
                row("ECLI nr", "ECLI:BE:CASS:2025:ARR.20251016.1N.10"),
                row("Rolnummer", "C.24.0199.N"),
                row("Kamer", "1N - eerste kamer"),
                row("Rechtsgebied", "Burgerlijk recht"),
            ],
            text,
        );
        assert_eq!(juportal_page_lang(&html), Some("nld"));
        let parsed = parse_juportal(&html).unwrap();
        let d = &parsed.decision;
        assert_eq!(
            d.source_uid,
            "juportal/ECLI:BE:CASS:2025:ARR.20251016.1N.10/nl"
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2025-10-16"));
        assert_eq!(d.numero_dossiers, Some(vec!["C.24.0199.N".to_string()]));
        assert_eq!(d.chamber.as_deref(), Some("1N - eerste kamer"));
        let sf = &parsed.source_fields;
        assert_eq!(sf["lang"], "nld");
        assert_eq!(sf["kind"], "Vonnis/arrest");
        assert_eq!(
            sf["source_url"],
            "https://juportal.be/content/ECLI:BE:CASS:2025:ARR.20251016.1N.10/NL"
        );
        let stored = from_source_fields_juportal(&d.texte_integral_clean, sf, &d.source_uid);
        assert_eq!(stored.source_uid, d.source_uid);
    }

    #[test]
    fn a_page_whose_text_reads_in_another_language_an_order_and_a_page_without_text_are_refused() {
        let dutch = page(
            "Vonnis/arrest van 16 oktober 2025",
            &[
                row("ECLI nr", "ECLI:BE:CASS:2025:ARR.20251016.1N.10"),
                row("Rolnummer", "C.24.0199.N"),
            ],
            TEXT,
        );
        assert!(is_juportal_page(&dutch));
        assert!(parse_juportal(&dutch).is_err());
        let order = page(
            "Ordonnance du 29 octobre 2025",
            &[row("No ECLI", "ECLI:BE:RVSCE:2025:ORD.16.497")],
            TEXT,
        );
        assert!(parse_juportal(&order).is_err());
        let bare = page(
            "Jugement/arrêt du 17 novembre 2021",
            &[row("No ECLI", "ECLI:BE:CASS:2021:ARR.20211117.2F.6")],
            "",
        );
        assert!(parse_juportal(&bare).is_err());
    }

    #[test]
    fn a_feed_lists_its_ecli_guids_and_a_page_that_is_no_feed_is_an_error() {
        let feed = "<?xml version=\"1.0\"?>\n<rss version=\"2.0\">\n<channel>\n<title>JUPORTAL</title>\n\
            <item>\n<title>ECLI:BE:CASS:2026:ARR.20261020.2N.3</title>\n\
            <guid isPermaLink=\"false\">ECLI:BE:CASS:2026:ARR.20261020.2N.3</guid>\n</item>\n\
            <item>\n<guid isPermaLink=\"false\">ECLI:BE:COHSAV:2026:DEC.20260603.2</guid>\n</item>\n\
            </channel>\n</rss>";
        let eclis = juportal_feed(feed).unwrap();
        assert_eq!(
            eclis,
            vec![
                "ECLI:BE:CASS:2026:ARR.20261020.2N.3",
                "ECLI:BE:COHSAV:2026:DEC.20260603.2"
            ]
        );
        assert!(juportal_taken(&eclis[0]));
        assert!(!juportal_taken(&eclis[1]));
        assert!(!juportal_taken("ECLI:BE:CASS:2025:CONC.20250915.3F.6"));
        assert!(juportal_feed("<html><body>Request Rejected</body></html>").is_err());
    }
}
