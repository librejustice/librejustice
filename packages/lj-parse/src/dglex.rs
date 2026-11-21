//! DGLex: one coordinated text per PDF, read from its `pdftohtml -xml` runs.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;
use crate::pdf_articles::{pdf_articles, Words};

static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\[?)(?i:Artikel|Art\.)\s*([0-9]+[A-Za-z]*(?:\s(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies|undecies|duodecies|terdecies|quaterdecies|quindecies|sexdecies|septdecies|octodecies|novodecies|vicies)\b)?(?:[./][0-9]+[A-Za-z]*)*)\.?\s*[-\x{2013}:]?\s*").unwrap()
});
static SOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\[?)(?i:Einziger\s+Artikel)\.?\s*[-\x{2013}:]?\s*").unwrap());
/// What follows the last article: the promulgation, the place and day of signature, the signatures.
static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Wir fertigen|(?:Gegeben zu )?Eupen, den \d|Für die Regierung)").unwrap()
});
static ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a class="item-link" href="[^"]*DokID=([0-9a-f-]+)&FileID=([0-9a-f-]+)"[^>]*>(.*?)</a>(.*?)</li>"#)
        .unwrap()
});
static FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<label class="wsdokdesc">([^<]+?): </label><span class="wsdokdata">([^<]*)</span>"#,
    )
    .unwrap()
});
static FOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9.]+) Ergebnisse gefunden").unwrap());
static LISTED_DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}\.\d{2}\.\d{2}[,.]\s*").unwrap());
static DOTTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{2})\.(\d{2})\.(\d{4})$").unwrap());

/// A document DGLex's search lists: its ids, title, adoption and Moniteur days (`YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DglexListed {
    pub document: String,
    pub file: String,
    pub title: String,
    pub adopted: Option<String>,
    pub published: Option<String>,
}

/// One page of DGLex's search: how many documents it found in all, and this page's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DglexList {
    pub total: usize,
    pub documents: Vec<DglexListed>,
}

fn dotted(d: &str) -> Option<String> {
    let c = DOTTED.captures(d.trim())?;
    day(&c[1], c[2].parse().ok()?, c[3].parse().ok()?)
}

pub fn dglex_listing(html: &str) -> Result<DglexList, String> {
    let total = FOUND.captures(html).ok_or("no result count")?[1]
        .replace('.', "")
        .parse()
        .map_err(|e| format!("result count: {e}"))?;
    let documents = ITEM
        .captures_iter(html)
        .map(|c| {
            let fields: HashMap<&str, &str> = FIELD
                .captures_iter(c.get(4).map_or("", |m| m.as_str()))
                .filter_map(|f| Some((f.get(1)?.as_str(), f.get(2)?.as_str())))
                .collect();
            let title = html_escape::decode_html_entities(&c[3])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            DglexListed {
                document: c[1].to_string(),
                file: c[2].to_string(),
                title: LISTED_DAY.replace(&title, "").into_owned(),
                adopted: fields.get("Verabschiedungsdatum").and_then(|d| dotted(d)),
                published: fields.get("Veröffentlichungsdatum").and_then(|d| dotted(d)),
            }
        })
        .collect();
    Ok(DglexList { total, documents })
}

fn day(d: &str, m: i8, y: i16) -> Option<String> {
    jiff::civil::Date::new(y, m, d.parse().ok()?)
        .ok()
        .map(|d| d.to_string())
}

/// The rank of a heading by its first word, deepest last.
fn level(heading: &str) -> Option<u8> {
    let word = heading
        .trim_start_matches(['[', ' '])
        .split([' ', '-'])
        .next()?;
    match word.to_lowercase().as_str() {
        "teil" => Some(1),
        "buch" => Some(2),
        "titel" => Some(3),
        "kapitel" => Some(4),
        "abschnitt" => Some(5),
        "unterabschnitt" => Some(6),
        _ => None,
    }
}

static GERMAN: Words = Words {
    head: &HEAD,
    sole: &SOLE,
    closing: &CLOSING,
    annexed: false,
    level,
    furniture: None,
    restart: Some("[BS "),
};

/// A coordinated text's articles: what follows each bold head, the headings above it as its path.
/// A text with no bold head is read again on its plain heads, a number followed by a separator.
pub fn dglex_articles(xml: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    pdf_articles(xml, &GERMAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(top: i32, left: i32, width: i32, font: u32, inner: &str) -> String {
        format!(
            r#"<text top="{top}" left="{left}" width="{width}" height="15" font="{font}">{inner}</text>"#
        )
    }

    #[test]
    fn a_document_reads_its_headings_and_articles_without_its_title_notes_or_wrapped_heads() {
        let xml = [
            r#"<page number="1" position="absolute" top="0" left="0" height="1188" width="918">"#
                .to_string(),
            r##"<fontspec id="0" size="12" family="Verdana" color="#000000"/>"##.to_string(),
            r##"<fontspec id="1" size="12" family="Verdana" color="#000000"/>"##.to_string(),
            r##"<fontspec id="2" size="12" family="Verdana" color="#000000"/>"##.to_string(),
            r##"<fontspec id="6" size="8" family="Verdana" color="#000000"/>"##.to_string(),
            r##"<fontspec id="7" size="15" family="Verdana" color="#000000"/>"##.to_string(),
            run(1120, 778, 9, 0, "<b>1</b>"),
            run(1120, 787, 5, 2, "/"),
            run(1120, 792, 9, 0, "<b>2</b>"),
            run(
                106,
                127,
                667,
                0,
                "<b>31. Dezember 1983 - Gesetz über institutionelle Reformen in Ausführung von </b>",
            ),
            run(113, 200, 300, 0, "<b>Artikel 130 der Verfassung </b>"),
            run(
                121,
                170,
                604,
                1,
                "<i>[BS 18.01.84; abgeändert G. 20.03.07 (BS </i>",
            ),
            run(136, 170, 604, 1, "<i>13.06.07)]</i>"),
            run(
                281,
                106,
                705,
                2,
                r##"<a href="x.html#1">TITEL I - Einleitende Bestimmungen .... 1</a>"##,
            ),
            run(
                923,
                128,
                275,
                0,
                "<b>TITEL I - EINLEITENDE BESTIMMUNGEN </b>",
            ),
            run(952, 128, 58, 0, "<b>Artikel 1</b>"),
            run(
                952,
                185,
                448,
                2,
                " - [Für die Anwendung ist zu verstehen unter: ",
            ),
            run(1011, 128, 418, 2, "1. [Parlament: das Parlament,]"),
            run(1011, 546, 5, 6, "1"),
            run(
                1041,
                106,
                221,
                7,
                "                                           ",
            ),
            run(1059, 106, 5, 6, "<i>1</i>"),
            run(
                1059,
                111,
                271,
                1,
                "<i> Nr. 1 ersetzt durch Art. 14 des G. 27.03.06 </i>",
            ),
            "</page>".to_string(),
            r#"<page number="2" position="absolute" top="0" left="0" height="1188" width="918">"#
                .to_string(),
            run(121, 128, 38, 0, "<b>Art. 2</b>"),
            run(
                121,
                166,
                412,
                2,
                " - Die Gemeinschaft besitzt Rechtspersönlichkeit. ",
            ),
            run(
                130,
                128,
                400,
                0,
                "<b>Art. 3 - Gemischte Aufträge - Verweis auf </b>",
            ),
            run(140, 106, 120, 0, "<b>Artikel 4 bis </b>"),
            run(145, 128, 200, 2, "Text drei. "),
            run(150, 128, 300, 0, "<b>KAPITEL II - Die Regierung </b>"),
            run(165, 128, 38, 0, "<b>Art. 4 bis</b>"),
            run(165, 166, 300, 2, " - §1 - Die Regierung. "),
            run(172, 128, 300, 0, "<b>[Art. 2</b> - Alte Fassung.]"),
            run(176, 128, 300, 2, "Noch alt."),
            run(180, 128, 5, 2, "["),
            run(180, 133, 100, 0, "<i><b>Artikel 4/1 - Eingefügt</b></i>"),
            run(190, 128, 200, 2, "Neuer Text.]"),
            run(200, 128, 58, 0, "<b>Artikel 1</b>"),
            run(200, 185, 300, 2, " - Anlage zum Gesetz. "),
            "</page>".to_string(),
        ]
        .join("\n");
        let articles = dglex_articles(&xml).unwrap();
        let read: Vec<(&str, Option<&str>, &str)> = articles
            .iter()
            .map(|a| (a.num.as_str(), a.title_path.as_deref(), a.texte.as_str()))
            .collect();
        let titled = "TITEL I - EINLEITENDE BESTIMMUNGEN";
        assert_eq!(
            read,
            [
                (
                    "1",
                    Some(titled),
                    "[Für die Anwendung ist zu verstehen unter:\n1. [Parlament: das Parlament,]"
                ),
                (
                    "2",
                    Some(titled),
                    "Die Gemeinschaft besitzt Rechtspersönlichkeit."
                ),
                (
                    "3",
                    Some(titled),
                    "Gemischte Aufträge - Verweis auf Artikel 4 bis\nText drei."
                ),
                (
                    "4bis",
                    Some("TITEL I - EINLEITENDE BESTIMMUNGEN > KAPITEL II - Die Regierung"),
                    "§1 - Die Regierung."
                ),
                (
                    "4/1",
                    Some("TITEL I - EINLEITENDE BESTIMMUNGEN > KAPITEL II - Die Regierung"),
                    "[Eingefügt\nNeuer Text.]"
                ),
            ]
        );
    }

    #[test]
    fn a_sole_article_in_small_capitals_is_read_with_no_moniteur_line() {
        let xml = [
            r#"<page number="1" position="absolute" top="0" left="0" height="1188" width="918">"#
                .to_string(),
            r##"<fontspec id="0" size="12" family="Verdana" color="#000000"/>"##.to_string(),
            r##"<fontspec id="1" size="10" family="Verdana" color="#000000"/>"##.to_string(),
            run(
                55,
                116,
                600,
                0,
                "<b>20. JUNI 2005. DEKRET ZUR ZUSTIMMUNG DES VERTRAGS </b>",
            ),
            run(
                100,
                106,
                500,
                0,
                "Das Parlament hat das Folgende angenommen: ",
            ),
            run(157, 128, 8, 0, "<b>E</b>"),
            run(160, 136, 50, 1, "<b>INZIGER </b>"),
            run(157, 189, 10, 0, "<b>A</b>"),
            run(160, 199, 45, 1, "<b>RTIKEL </b>"),
            run(157, 244, 9, 0, "<b>:</b>"),
            run(
                157,
                258,
                400,
                0,
                "Der Vertrag ist uneingeschränkt wirksam. ",
            ),
            run(172, 106, 300, 0, "Zweite Zeile. "),
            "</page>".to_string(),
        ]
        .join("\n");
        let articles = dglex_articles(&xml).unwrap();
        let read: Vec<(&str, &str)> = articles
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str()))
            .collect();
        assert_eq!(
            read,
            [(
                "unique",
                "Der Vertrag ist uneingeschränkt wirksam.\nZweite Zeile."
            )]
        );
    }

    #[test]
    fn a_listing_page_reads_its_count_and_each_document_without_its_commented_link() {
        let row = |id: &str, title: &str, adopted: &str, published: &str| {
            format!(
                r#"<li class="list-item link-item ws-list-item inst-pdg" data-icon="x">
                <!--<a class="item-link" href="javascript:GetAJAXDokDirectDownload('{id}',6056);">-->
                <a class="item-link" href="/addons/SharepointDokumentsuche/desktop/SharepointDokDetails.aspx?DokID={id}&FileID={id}" target="_blank">
                    {title}
                    </a>
                    <span>
                        <div class="wsdoksubelements"><label class="wsdokdesc">Kategorie: </label><span class="wsdokdata">02. Zustimmungsdekrete</span></div>
                        <div class="wsdoksubelements"><label class="wsdokdesc">Verabschiedungsdatum: </label><span class="wsdokdata">{adopted}</span></div>
                        {published}
                    </span></li>"#
            )
        };
        let html = format!(
            "<div>1.039 Ergebnisse gefunden.</div><ul class=\"result-list\">{}{}</ul>",
            row(
                "4bf10d87-6ff7-4cdd-8e44-9f2c7e268780",
                "2026.06.22, Dekret zur Zustimmung zu dem Abkommen (CETA) &amp; Protokoll",
                "22.06.2026",
                r#"<div class="wsdoksubelements"><label class="wsdokdesc">Veröffentlichungsdatum: </label><span class="wsdokdata">17.07.2026</span></div>"#,
            ),
            row(
                "1acf8b28-bb90-484a-a9d8-2952a2e501d0",
                "2007.12.19. Dekret über die Genehmigungsaufsicht",
                "19.12.2007",
                "",
            ),
        );
        let list = dglex_listing(&html).unwrap();
        assert_eq!(list.total, 1039);
        assert_eq!(
            list.documents,
            [
                DglexListed {
                    document: "4bf10d87-6ff7-4cdd-8e44-9f2c7e268780".into(),
                    file: "4bf10d87-6ff7-4cdd-8e44-9f2c7e268780".into(),
                    title: "Dekret zur Zustimmung zu dem Abkommen (CETA) & Protokoll".into(),
                    adopted: Some("2026-06-22".into()),
                    published: Some("2026-07-17".into()),
                },
                DglexListed {
                    document: "1acf8b28-bb90-484a-a9d8-2952a2e501d0".into(),
                    file: "1acf8b28-bb90-484a-a9d8-2952a2e501d0".into(),
                    title: "Dekret über die Genehmigungsaufsicht".into(),
                    adopted: Some("2007-12-19".into()),
                    published: None,
                },
            ]
        );
    }

    #[test]
    fn a_text_with_no_bold_head_is_read_on_its_plain_heads_followed_by_a_separator() {
        let xml = [
            r#"<page number="1" position="absolute" top="0" left="0" height="1188" width="918">"#
                .to_string(),
            r##"<fontspec id="0" size="12" family="Verdana" color="#000000"/>"##.to_string(),
            run(
                55,
                116,
                600,
                0,
                "<b>2. MAI 2004. ERLASS ZUR UNTERSCHUTZSTELLUNG </b>",
            ),
            run(100, 106, 500, 0, "Die Regierung beschließt: "),
            run(
                120,
                106,
                500,
                0,
                "Artikel 1: Das Haus wird unter Schutz gestellt gemäß ",
            ),
            run(135, 106, 500, 0, "Artikel 2 des Dekretes. "),
            run(
                150,
                106,
                500,
                0,
                "Artikel 2 - Dieser Erlass tritt in Kraft. ",
            ),
            "</page>".to_string(),
        ]
        .join("\n");
        let read: Vec<(String, String)> = dglex_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| (a.num, a.texte))
            .collect();
        assert_eq!(
            read,
            [
                (
                    "1".to_string(),
                    "Das Haus wird unter Schutz gestellt gemäß\nArtikel 2 des Dekretes."
                        .to_string()
                ),
                ("2".to_string(), "Dieser Erlass tritt in Kraft.".to_string()),
            ]
        );
    }
}
