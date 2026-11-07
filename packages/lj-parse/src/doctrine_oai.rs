//! Law journals and blogs harvested over OAI-PMH, read as outbound `note`
//! links on the cassation decisions they comment.

use std::sync::LazyLock;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::Regex;
use serde_json::{json, Value};

use crate::hal::extract_dockets;

/// One `<record>` of an OAI-PMH answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OaiRecord {
    pub identifier: String,
    pub datestamp: String,
    pub deleted: bool,
    /// Every metadata element holding text, by local name, in document order.
    pub fields: Vec<(String, String)>,
    /// The `article` divisions of a METS issue record, `(ID, LABEL)`.
    pub articles: Vec<(String, String)>,
}

impl OaiRecord {
    pub fn all<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a str> + use<'a, 'n> {
        self.fields
            .iter()
            .filter(move |(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn first(&self, name: &str) -> Option<&str> {
        self.all(name).next()
    }

    /// The landing page the publisher gives, never the DOI resolver.
    fn page_url(&self) -> Option<&str> {
        self.all("identifier")
            .find(|i| i.starts_with("http") && !i.contains("doi.org"))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OaiPage {
    pub records: Vec<OaiRecord>,
    pub resumption_token: Option<String>,
}

/// Reads one `ListRecords` or `GetRecord` answer. `noRecordsMatch` is an empty
/// page; any other OAI error is an error.
pub fn parse_oai_page(xml: &str) -> Result<OaiPage, String> {
    let mut reader = Reader::from_str(xml);
    let mut page = OaiPage::default();
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut record: Option<OaiRecord> = None;
    let mut error_code: Option<String> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("OAI XML at byte {}: {e}", reader.buffer_position()))?;
        match event {
            Event::Start(e) => {
                let name = local_name(&e);
                open(&e, &name, &mut record, &mut error_code)?;
                stack.push(name);
                text.clear();
            }
            Event::Empty(e) => {
                let name = local_name(&e);
                open(&e, &name, &mut record, &mut error_code)?;
            }
            Event::Text(t) => text.push_str(&t.xml10_content().map_err(|e| e.to_string())?),
            Event::CData(t) => text.push_str(&t.decode().map_err(|e| e.to_string())?),
            Event::GeneralRef(r) => match r.resolve_char_ref().map_err(|e| e.to_string())? {
                Some(c) => text.push(c),
                None => {
                    let name = r.decode().map_err(|e| e.to_string())?;
                    text.push(match name.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => return Err(format!("OAI XML: undeclared entity &{other};")),
                    });
                }
            },
            Event::End(_) => {
                let name = stack.pop().unwrap_or_default();
                let value = text.split_whitespace().collect::<Vec<_>>().join(" ");
                text.clear();
                match name.as_str() {
                    "record" => page.records.extend(record.take()),
                    "resumptionToken" if !value.is_empty() => page.resumption_token = Some(value),
                    "error" => {
                        return match error_code.as_deref() {
                            Some("noRecordsMatch") => Ok(OaiPage::default()),
                            code => Err(format!("OAI error {}: {value}", code.unwrap_or("?"))),
                        };
                    }
                    "identifier" | "datestamp" if stack.last().is_some_and(|p| p == "header") => {
                        if let Some(r) = record.as_mut() {
                            if name == "identifier" {
                                r.identifier = value;
                            } else {
                                r.datestamp = value;
                            }
                        }
                    }
                    _ if !value.is_empty() && stack.iter().any(|s| s == "metadata") => {
                        if let Some(r) = record.as_mut() {
                            r.fields.push((name, value));
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(page)
}

fn local_name(e: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart<'_>, name: &str) -> Result<Option<String>, String> {
    let Some(a) = e.try_get_attribute(name).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let value = a
        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map_err(|e| e.to_string())?;
    Ok(Some(value.into_owned()))
}

fn open(
    e: &BytesStart<'_>,
    name: &str,
    record: &mut Option<OaiRecord>,
    error_code: &mut Option<String>,
) -> Result<(), String> {
    match name {
        "record" => *record = Some(OaiRecord::default()),
        "header" => {
            if let Some(r) = record.as_mut() {
                r.deleted = attr(e, "status")?.as_deref() == Some("deleted");
            }
        }
        "error" => *error_code = attr(e, "code")?,
        "div" if attr(e, "TYPE")?.as_deref() == Some("article") => {
            if let (Some(r), Some(id)) = (record.as_mut(), attr(e, "ID")?) {
                r.articles.push((id, attr(e, "LABEL")?.unwrap_or_default()));
            }
        }
        _ => {}
    }
    Ok(())
}

/// A note as its publisher describes it, with the field that names the
/// decision it comments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DoctrineNote {
    /// Stable within the platform: `cairn/RCDIP_262_0227`, `openedition/dsf/783`.
    pub uid: String,
    pub cites: String,
    pub title: String,
    pub authors: Vec<String>,
    pub journal: String,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub date: Option<String>,
    pub url: String,
}

/// The title of a Cairn METS article label, `Title / Authors / p. 12-30 / free
/// access`. Cairn cuts the label near 200 characters, and a cut label is the
/// title alone.
pub fn cairn_label_title(label: &str) -> String {
    let parts: Vec<&str> = label.split(" / ").collect();
    let title = match parts
        .iter()
        .rposition(|p| p.trim_start().starts_with("p. "))
    {
        Some(i) if i >= 2 => parts[..i - 1].join(" / "),
        Some(1) => parts[0].to_string(),
        _ => label.to_string(),
    };
    title.trim().to_string()
}

/// A Cairn article from its METS title and its `oai_dc` record, whose `source`
/// reads `Journal | n° 920-921-922 | 4 | 2025-03-10 | p. 45-53 | ISSN`.
pub fn cairn_note(
    article_id: &str,
    title: &str,
    dc: &OaiRecord,
    journal: &str,
) -> Option<DoctrineNote> {
    let source: Vec<&str> = dc
        .first("source")
        .unwrap_or_default()
        .split('|')
        .map(str::trim)
        .collect();
    let issue = match source.get(1) {
        Some(label) if label.contains(|c: char| c.is_ascii_digit()) => Some(label.to_string()),
        _ => source
            .get(2)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string()),
    };
    Some(DoctrineNote {
        uid: format!("cairn/{article_id}"),
        cites: title.to_string(),
        title: title.to_string(),
        authors: persons(dc.all("creator")),
        journal: journal.to_string(),
        issue,
        pages: source
            .iter()
            .find(|s| s.starts_with("p. "))
            .map(|s| s.to_string()),
        date: dc.first("date").map(str::to_string),
        url: dc.page_url()?.to_string(),
    })
}

/// A Hypotheses post: its title and the first lines OpenEdition carries, which
/// open on the citation of the decision.
pub fn openedition_note(record: &OaiRecord, journal: &str) -> Option<DoctrineNote> {
    let cites: Vec<&str> = ["title", "description", "subject"]
        .iter()
        .flat_map(|f| record.all(f))
        .collect();
    Some(DoctrineNote {
        uid: format!("openedition/{}", local_id(&record.identifier)),
        cites: cites.join(" \n "),
        title: record.first("title")?.to_string(),
        authors: persons(record.all("creator")),
        journal: journal.to_string(),
        date: record.first("date").map(|d| d.chars().take(10).collect()),
        url: record.page_url()?.to_string(),
        ..Default::default()
    })
}

/// The page of a Prairial article, whose `DC.title` carries the subtitle the
/// OAI title drops. An article set inside its issue's page is linked there,
/// `index.php?id=<issue>#<article>`, and still has a page of its own.
pub fn prairial_page_url(record: &OaiRecord) -> Option<String> {
    if record.first("type") != Some("article") {
        return None;
    }
    let base = record.page_url()?.split("index.php?id=").next()?;
    let id = record.identifier.rsplit('/').next()?;
    Some(format!("{base}index.php?id={id}"))
}

pub fn page_dc_title(html: &str) -> Option<String> {
    static META: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"<meta\s+name="DC\.title"\s+content="([^"]*)""#).expect("DC.title regex")
    });
    let raw = META.captures(html)?.get(1)?.as_str();
    let title = html_escape::decode_html_entities(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some(title)
}

pub fn prairial_note(record: &OaiRecord, page_title: &str, journal: &str) -> Option<DoctrineNote> {
    Some(DoctrineNote {
        uid: format!("prairial/{}", local_id(&record.identifier)),
        cites: page_title.to_string(),
        title: page_title.to_string(),
        authors: persons(record.all("creator")),
        journal: journal.to_string(),
        date: record.first("date").map(str::to_string),
        url: record.page_url()?.to_string(),
        ..Default::default()
    })
}

/// `oai:hypotheses.org:dsf/783` gives `dsf/783`, `oai:ajdc/326` gives `ajdc/326`.
fn local_id(identifier: &str) -> &str {
    identifier.rsplit(':').next().unwrap_or(identifier)
}

/// A creator written `Surname, Given` is a person, shown `Given Surname`. A
/// blog's login or a laboratory is not an author.
fn persons<'a>(creators: impl Iterator<Item = &'a str>) -> Vec<String> {
    creators
        .filter_map(|c| {
            let (surname, given) = c.split_once(", ")?;
            let person =
                !given.contains(',') && !c.contains(|ch: char| ch.is_ascii_digit() || ch == '(');
            person.then(|| format!("{} {}", given.trim(), surname.trim()))
        })
        .collect()
}

/// The decisions a note names: its cassation dockets and the days it writes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Citation {
    pub dockets: Vec<String>,
    /// ISO dates.
    pub days: Vec<String>,
}

static DAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(1er|\d{1,2})\s+(\p{L}+\.?)\s+((?:19|20)\d{2})\b").expect("day regex")
});

pub fn cited_decisions(text: &str) -> Citation {
    let mut days: Vec<String> = Vec::new();
    for c in DAY.captures_iter(text) {
        let written = format!("{} {} {}", &c[1], &c[2], &c[3]);
        if let Some(day) = lj_core::dates::parse_date_fr(&written).map(|d| d.to_string()) {
            if !days.contains(&day) {
                days.push(day);
            }
        }
    }
    Citation {
        dockets: extract_dockets(text),
        days,
    }
}

impl Citation {
    /// The day guard: a note that writes a day comments a decision of that
    /// day. Without it, a court of appeal roll number shaped `NN-NN.NNN` and the
    /// earlier arrêts of the same pourvoi attach.
    pub fn admits(&self, decided: Option<&str>) -> bool {
        self.days.is_empty() || decided.is_some_and(|d| self.days.iter().any(|day| day == d))
    }
}

/// Whether a HAL note already on the decision is this one: published in the
/// same journal, or under the same title.
pub fn same_note(
    note: &DoctrineNote,
    hal_publisher: Option<&str>,
    hal_title: Option<&str>,
) -> bool {
    let key = |s: &str| lj_fold::slug(s, ' ');
    let journal = key(&note.journal);
    hal_publisher.is_some_and(|p| key(p).starts_with(&journal))
        || hal_title.is_some_and(|t| key(t) == key(&note.title))
}

/// Whether a robots.txt lets the crawler named `agent` read `path`, as RFC
/// 9309 reads it: the groups naming the agent, else the `*` groups, and the
/// longest matching rule wins, `Allow` on a tie.
pub fn robots_allows(robots: &str, agent: &str, path: &str) -> bool {
    #[derive(Default)]
    struct Group {
        agents: Vec<String>,
        /// `(allow, pattern)`.
        rules: Vec<(bool, String)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut naming = false;
    for line in robots.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match key.as_str() {
            "user-agent" => {
                if !naming || groups.is_empty() {
                    groups.push(Group::default());
                }
                naming = true;
                if let Some(g) = groups.last_mut() {
                    g.agents.push(value.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                naming = false;
                if let (Some(g), false) = (groups.last_mut(), value.is_empty()) {
                    g.rules.push((key == "allow", value.to_string()));
                }
            }
            _ => {}
        }
    }
    let agent = agent.to_ascii_lowercase();
    let naming_agent: Vec<&Group> = groups
        .iter()
        .filter(|g| g.agents.contains(&agent))
        .collect();
    let chosen = if naming_agent.is_empty() {
        groups
            .iter()
            .filter(|g| g.agents.iter().any(|a| a == "*"))
            .collect()
    } else {
        naming_agent
    };
    chosen
        .into_iter()
        .flat_map(|g| &g.rules)
        .filter(|(_, pattern)| robots_match(pattern, path))
        .max_by_key(|(allow, pattern)| (pattern.len(), *allow))
        .is_none_or(|(allow, _)| *allow)
}

fn robots_match(pattern: &str, path: &str) -> bool {
    let (body, anchored) = match pattern.strip_suffix('$') {
        Some(b) => (b, true),
        None => (pattern, false),
    };
    let mut re = String::from("^");
    for (i, piece) in body.split('*').enumerate() {
        if i > 0 {
            re.push_str(".*");
        }
        re.push_str(&regex::escape(piece));
    }
    if anchored {
        re.push('$');
    }
    Regex::new(&re).is_ok_and(|r| r.is_match(path))
}

/// The outbound link of one note; no body is stored.
pub fn build_doctrine_source_fields(note: &DoctrineNote) -> Value {
    json!({
        "commentaires": [{
            "kind": "note",
            "title": note.title,
            "author": (!note.authors.is_empty()).then(|| note.authors.join(", ")),
            "publisher": note.journal,
            "issue": note.issue,
            "pages": note.pages,
            "date": note.date,
            "url": note.url,
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DC_PAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/"><ListRecords>
  <record><header><identifier>oai:cairn.info:DROU_920_0003</identifier><datestamp>2025-03-10</datestamp></header>
  <metadata><oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/">
    <dc:identifier>https://droit.cairn.info/revue-le-droit-ouvrier-2025-4-5-6-page-45?lang=fr</dc:identifier>
    <dc:identifier>https://doi.org/10.3917/drou.920.0003</dc:identifier>
    <dc:title>L&#8217;apport du droit &amp; de l'Union</dc:title>
    <dc:creator xml:id="https://www.idref.fr/052578615">Héas, Franck</dc:creator>
    <dc:date>2025</dc:date>
    <dc:source>Le Droit Ouvrier | n° 920-921-922 | 4 | 2025-03-10 | p. 45-53 | 0222-4194</dc:source>
    <dcterms:accessRights>restricted access</dcterms:accessRights><dcterms:accessRights> </dcterms:accessRights>
  </oai_dc:dc></metadata></record>
  <record><header status="deleted"><identifier>oai:cairn.info:DROU_920_0012</identifier></header></record>
  <resumptionToken completeListSize="1000" cursor="0">token-0001</resumptionToken>
</ListRecords></OAI-PMH>"#;

    fn dc_record() -> OaiRecord {
        parse_oai_page(DC_PAGE).unwrap().records.remove(0)
    }

    #[test]
    fn a_page_gives_its_records_their_fields_and_the_next_token() {
        let page = parse_oai_page(DC_PAGE).unwrap();
        assert_eq!(page.resumption_token.as_deref(), Some("token-0001"));
        assert_eq!(page.records.len(), 2);
        let r = &page.records[0];
        assert_eq!(r.identifier, "oai:cairn.info:DROU_920_0003");
        assert_eq!(r.datestamp, "2025-03-10");
        assert!(!r.deleted);
        assert_eq!(
            r.first("title"),
            Some("L\u{2019}apport du droit & de l'Union")
        );
        assert_eq!(r.all("identifier").count(), 2);
        assert_eq!(
            r.all("accessRights").collect::<Vec<_>>(),
            ["restricted access"]
        );
        assert!(page.records[1].deleted);
    }

    #[test]
    fn no_records_match_is_an_empty_page_and_any_other_error_fails() {
        let empty =
            r#"<OAI-PMH><error code="noRecordsMatch">No matching records</error></OAI-PMH>"#;
        assert_eq!(parse_oai_page(empty).unwrap(), OaiPage::default());
        let bad = r#"<OAI-PMH><error code="badResumptionToken">expired</error></OAI-PMH>"#;
        assert!(parse_oai_page(bad)
            .unwrap_err()
            .contains("badResumptionToken"));
    }

    #[test]
    fn a_mets_issue_gives_its_article_labels() {
        let xml = r#"<OAI-PMH><ListRecords><record><header><identifier>RCDIP_252</identifier></header>
<metadata><mets:mets xmlns:mets="http://www.loc.gov/mets/"><mets:structMap><mets:div ID="RCDIP_252">
<mets:div ID="RCDIP_252_0368" LABEL="La compétence. Civ. 1re, 4&#160;nov. 2025, n°&#160;24-15.108 / Gaudemet-Tallon, Hélène / p. 368-376 / restricted access" TYPE="article"><mets:fptr FILEID="x"/></mets:div>
<mets:div ID="RCDIP_252_0001" LABEL="Pages de début /  / p. 1-2 / free access" TYPE="article"/>
</mets:div></mets:structMap></mets:mets></metadata></record></ListRecords></OAI-PMH>"#;
        let r = parse_oai_page(xml).unwrap().records.remove(0);
        assert_eq!(r.articles.len(), 2);
        assert_eq!(r.articles[0].0, "RCDIP_252_0368");
        assert_eq!(
            cairn_label_title(&r.articles[0].1),
            "La compétence. Civ. 1re, 4\u{a0}nov. 2025, n°\u{a0}24-15.108"
        );
        assert_eq!(cairn_label_title(&r.articles[1].1), "Pages de début");
    }

    #[test]
    fn a_cairn_label_keeps_a_title_holding_a_slash_and_a_cut_label_is_all_title() {
        assert_eq!(
            cairn_label_title(
                "Accueillir les uns / éloigner les autres / Turpin, Dominique / p. 235-266 / free access"
            ),
            "Accueillir les uns / éloigner les autres"
        );
        let cut =
            "RUPTURE DU CONTRAT. Reçu pour solde / Mention d'une somme globale concernant les";
        assert_eq!(cairn_label_title(cut), cut);
    }

    #[test]
    fn a_cairn_note_reads_its_issue_pages_and_landing_page() {
        let n = cairn_note(
            "DROU_920_0003",
            "Titre. Cass. soc., 5 février 2025",
            &dc_record(),
            "Le Droit Ouvrier",
        )
        .unwrap();
        assert_eq!(n.uid, "cairn/DROU_920_0003");
        assert_eq!(n.issue.as_deref(), Some("n° 920-921-922"));
        assert_eq!(n.pages.as_deref(), Some("p. 45-53"));
        assert_eq!(
            n.url,
            "https://droit.cairn.info/revue-le-droit-ouvrier-2025-4-5-6-page-45?lang=fr"
        );
        assert_eq!(n.authors, ["Franck Héas"]);
        let c = &build_doctrine_source_fields(&n)["commentaires"][0];
        assert_eq!(c["publisher"], "Le Droit Ouvrier");
        assert_eq!(c["kind"], "note");

        let mut season = dc_record();
        season.fields.retain(|(k, _)| k != "source");
        season.fields.push((
            "source".into(),
            "Revue critique de droit international privé | Avril-Juin | 2 | 2026-09-15 | p. 227-259 | 0035-0958".into(),
        ));
        let n = cairn_note("RCDIP_262_0227", "t", &season, "RCDIP").unwrap();
        assert_eq!(n.issue.as_deref(), Some("2"));
    }

    #[test]
    fn a_login_or_a_laboratory_is_no_author() {
        let names = [
            "Tricoit, Jean-Philippe",
            "jeandupont",
            "Droits et perspectives du droit (EA n°4487), Droits et perspectives du droit (LIlle 2, EA n°4487)",
        ];
        assert_eq!(persons(names.into_iter()), ["Jean-Philippe Tricoit"]);
    }

    #[test]
    fn a_blog_post_is_read_in_its_title_lines_and_keywords() {
        let r = OaiRecord {
            identifier: "oai:hypotheses.org:dsf/714".into(),
            fields: vec![
                ("identifier".into(), "https://doi.org/10.58079/nxzh".into()),
                ("identifier".into(), "https://dsf.hypotheses.org/714".into()),
                (
                    "title".into(),
                    "Nouvelle assiette du budget du comité".into(),
                ),
                (
                    "description".into(),
                    "Références : Cass. Soc.,7 février 2018, n°16-24.231".into(),
                ),
                ("date".into(), "2018-05-28T13:11:52Z".into()),
            ],
            ..Default::default()
        };
        let n = openedition_note(&r, "Droit du travail et de la protection sociale").unwrap();
        assert_eq!(n.uid, "openedition/dsf/714");
        assert_eq!(n.url, "https://dsf.hypotheses.org/714");
        assert_eq!(n.date.as_deref(), Some("2018-05-28"));
        let cited = cited_decisions(&n.cites);
        assert_eq!(cited.dockets, ["16-24.231"]);
        assert_eq!(cited.days, ["2018-02-07"]);
    }

    #[test]
    fn a_prairial_article_is_read_on_its_own_page_even_when_linked_inside_its_issue() {
        let mut r = OaiRecord {
            identifier: "oai:ajdc/1916".into(),
            fields: vec![
                (
                    "identifier".into(),
                    "https://publications-prairial.fr/ajdc/index.php?id=1835#1916".into(),
                ),
                ("type".into(), "article".into()),
            ],
            ..Default::default()
        };
        assert_eq!(
            prairial_page_url(&r).as_deref(),
            Some("https://publications-prairial.fr/ajdc/index.php?id=1916")
        );
        let n = prairial_note(&r, "Cass. 2e Civ. 6 juillet 2023 n° 21-25.667", "AJDC").unwrap();
        assert_eq!(
            n.url,
            "https://publications-prairial.fr/ajdc/index.php?id=1835#1916"
        );
        assert_eq!(n.uid, "prairial/ajdc/1916");
        r.fields[1].1 = "issue".into();
        assert_eq!(prairial_page_url(&r), None);
    }

    #[test]
    fn the_page_title_is_the_dc_title_meta_decoded() {
        let html = r#"<head><meta name="DC.title" content="Perte de chance. Civ. 2e, 25&#160;mai 2022, n&#176; 20-16.351 &amp; autres" /></head>"#;
        assert_eq!(
            page_dc_title(html).as_deref(),
            Some("Perte de chance. Civ. 2e, 25 mai 2022, n° 20-16.351 & autres")
        );
        assert_eq!(page_dc_title("<head></head>"), None);
    }

    #[test]
    fn days_are_read_in_full_and_abbreviated_and_the_first_of_the_month() {
        let c = cited_decisions(
            "Civ. 1re, 12\u{a0}mars 2025, n°\u{a0}23-22.051 ; Civ. 1re, 2\u{a0}avr. 2025, n° 23-11.456 ; loi du 1er août 2016",
        );
        assert_eq!(c.dockets, ["23-22.051", "23-11.456"]);
        assert_eq!(c.days, ["2025-03-12", "2025-04-02", "2016-08-01"]);
    }

    #[test]
    fn the_day_guard_admits_only_a_written_day_and_anything_when_none_is_written() {
        let c = cited_decisions("Cass. soc., 7 décembre 2017, n° 16-21.814");
        assert!(c.admits(Some("2017-12-07")));
        assert!(!c.admits(Some("2015-06-03")));
        assert!(!c.admits(None));
        let undated = cited_decisions("Note sous pourvoi n° 16-21.814");
        assert!(undated.admits(Some("2015-06-03")));
        assert!(undated.admits(None));
    }

    #[test]
    fn a_hal_note_of_the_same_journal_or_title_is_the_same_note() {
        let note = DoctrineNote {
            title: "Obligation d\u{2019}évaluer un dommage certain".into(),
            journal: "Actualité juridique du dommage corporel".into(),
            ..Default::default()
        };
        let ajdc = Some("Actualité juridique du dommage corporel (AJDC)");
        assert!(same_note(&note, ajdc, None));
        let same_title = Some("Obligation d'évaluer un dommage certain");
        assert!(same_note(&note, Some("Gazette du Palais"), same_title));
        assert!(!same_note(&note, Some("Gazette du Palais"), Some("Autre")));
        assert!(!same_note(&note, None, None));
    }

    #[test]
    fn robots_rules_follow_rfc_9309() {
        assert!(robots_allows("", "LibreJusticeBot", "/oai.php"));
        let all = "User-agent: *\nDisallow: /\n";
        assert!(!robots_allows(all, "LibreJusticeBot", "/oai.php"));
        let named =
            "User-agent: *\nDisallow: /\n\nUser-agent: librejusticebot\nAllow: /oai/\nDisallow: /\n";
        assert!(robots_allows(named, "LibreJusticeBot", "/oai/"));
        assert!(!robots_allows(named, "LibreJusticeBot", "/ajdc/index.php"));
        let wildcard = "User-agent: *\nDisallow: /*.php$\n";
        assert!(!robots_allows(
            wildcard,
            "LibreJusticeBot",
            "/ajdc/index.php"
        ));
        assert!(robots_allows(wildcard, "LibreJusticeBot", "/oai/"));
    }
}
