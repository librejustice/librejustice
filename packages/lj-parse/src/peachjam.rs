//! A legislation page of a Peachjam site (SenegalLII): its Laws.Africa work and expression, its
//! dated expressions, its cover dates, and its articles when it renders the Akoma Ntoso text.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;

/// One work as a sitemap lists it: its latest expression's path and its modification day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeachjamListed {
    /// The expression's path as the site serves it, `/fr/akn/sn/act/1965/60/fra@1965-09-06`.
    pub path: String,
    /// The work's FRBR URI, decoded: `/akn/aa-ohada/act/2010/droit-commercial-général`.
    pub work: String,
    /// The place the work's URI names: `sn`, `aa-ohada`.
    pub place: String,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeachjamPage {
    pub work: String,
    pub expression_date: String,
    pub language: String,
    /// `decree`, `mo`; none for an act.
    pub subtype: Option<String>,
    pub year: String,
    /// The number the work's URI gives, decoded: `572`, `2009-1411`, `droit-commercial-général`.
    pub number: String,
    pub title: String,
    /// The work's expressions in this one's language, `(day, path)`, this one included.
    pub expressions: Vec<(String, String)>,
    pub assent: Option<String>,
    pub commencement: Option<String>,
    /// The source PDF's path when the page shows it in place of the text.
    pub pdf: Option<String>,
    pub articles: Vec<ConsolidatedArticle>,
    /// The text of a body that numbers no article.
    pub body: String,
}

static SITEMAP_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<loc>https?://[^/<]+(/[^<]*?(/akn/([^/<]+)/[^<]+?)/[a-z]{3}@[^<]*)</loc>(?:\s*<lastmod>(\d{4}-\d{2}-\d{2})[^<]*</lastmod>)?").unwrap()
});
static PROPERTIES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script id="track-page-properties" type="application/json">(.*?)</script>"#)
        .unwrap()
});
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<h1 id="main-page-heading"[^>]*>(.*?)</h1>"#).unwrap());
static EXPRESSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a class="dropdown-item[^"]*"\s*href="([^"]*/([a-z]{3})@(\d{4}-\d{2}-\d{2}))""#)
        .unwrap()
});
static DISPLAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"data-display-type="([a-z]+)""#).unwrap());
static PDF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"data-pdf="([^"]+)""#).unwrap());
static SOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="(/[^"]+@\d{4}-\d{2}-\d{2}/source)""#).unwrap());
static COVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<li class="(assent-date|commencement-date)">(.*?)</li>"#).unwrap()
});
static COVER_DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2}(?:er)?\s+\p{L}+\.?\s+\d{4})").unwrap());
static SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<section class="akn-([A-Za-z]+)" id="([^"]+)"[^>]*>"#).unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^\s*<h2>(.*?)</h2>").unwrap());
/// A head and its title past a dash or a dot (`Article 8-2. Intervention`), a number spaced
/// around its dash read whole: `Article 245 - 11` is 245-11.
static ARTICLE_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:article|art\.)\s+((?:(?i:premier|unique)|\d+)(?:\s*[-\x{2013}]\s*\d+)*(?:\s*(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|decies)\b)?|\S+)[\s.:\-\x{2013}]*(.*)$").unwrap()
});
static NUM_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[-\x{2013}]\s*").unwrap());
static NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<span class="akn-num">(.*?)</span>"#).unwrap());
/// Where a block starts: a paragraph, a list item, a heading, a table row, a break.
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<span class="akn-(?:p|li|alinea|intro|wrapUp|listIntroduction|listWrapUp|crossHeading|subheading)"|<br\s*/?>|<tr\b|<p\b|<div\b|<h[1-6]\b|</h[1-6]>"#)
        .unwrap()
});
static CELL_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]>").unwrap());
static BREAK_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
/// An inline element leaves its text glued to its neighbours: `2001</a>]` reads `2001]`.
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(a|abbr|b|em|i|span|strong|sub|sup|u)\b[^>]*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

/// `%C3%A9` read as `é`.
fn decoded(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| s.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The text of an HTML fragment on one line, whitespace folded.
fn plain(html: &str) -> String {
    let html = INLINE_TAG.replace_all(html, "");
    let bare = TAG.replace_all(&html, " ");
    html_escape::decode_html_entities(&bare)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A fragment one line per block, a numbering alone on its line (`a)`, `1°`) joined to the next.
fn lines(html: &str) -> Vec<String> {
    let html = NUM.replace_all(html, "\u{2}$1\u{3}");
    let html = CELL_END.replace_all(&html, " ");
    let html = BLOCK.replace_all(&html, "\n$0");
    let mut out: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in html.split('\n').map(plain).filter(|l| !l.is_empty()) {
        let numbering = line.starts_with('\u{2}') && line.ends_with('\u{3}');
        let line = line.replace(['\u{2}', '\u{3}'], "");
        let line = match pending.take() {
            Some(num) => format!("{num} {line}"),
            None => line,
        };
        if numbering {
            pending = Some(line);
        } else {
            out.push(line);
        }
    }
    out.extend(pending);
    out
}

/// The works a Peachjam legislation sitemap lists.
pub fn peachjam_sitemap(xml: &str) -> Vec<PeachjamListed> {
    SITEMAP_URL
        .captures_iter(xml)
        .map(|c| PeachjamListed {
            path: c[1].to_string(),
            work: decoded(&c[2]),
            place: c[3].to_string(),
            modified: c.get(4).map(|m| m.as_str().to_string()),
        })
        .collect()
}

/// An article's number and the title its heading gives after it.
fn head(heading: &str) -> (String, Option<String>) {
    match ARTICLE_HEAD.captures(heading) {
        Some(h) => (
            NUM_DASH.replace_all(&h[1], "-").into_owned(),
            h.get(2)
                .map(|t| t.as_str().to_string())
                .filter(|t| !t.is_empty()),
        ),
        None => (heading.to_string(), None),
    }
}

/// The articles of an Akoma Ntoso body, each under the containers its id descends from, the
/// sections an article holds read as its text.
fn articles(body: &str) -> Vec<ConsolidatedArticle> {
    let sections: Vec<_> = SECTION.captures_iter(body).collect();
    let mut labels: HashMap<&str, String> = HashMap::new();
    let mut out: Vec<(&str, ConsolidatedArticle)> = Vec::new();
    for (i, c) in sections.iter().enumerate() {
        let until = sections
            .get(i + 1)
            .map_or(body.len(), |next| next.get(0).unwrap().start());
        let segment = &body[c.get(0).unwrap().end()..until];
        let (kind, id) = (&c[1], c.get(2).unwrap().as_str());
        // An article inside another is a markup slip, unless an amendment quotes it.
        let held = kind != "article" || id.contains("__qstr");
        if let Some((_, article)) = out.last_mut().filter(|(within, _)| {
            held && id.strip_prefix(within).is_some_and(|r| r.starts_with("__"))
        }) {
            let text = lines(segment).join("\n");
            article.texte = match article.texte.is_empty() {
                true => text,
                false => format!("{}\n{text}", article.texte),
            };
            continue;
        }
        let heading = HEADING.captures(segment);
        let rest = heading
            .as_ref()
            .map_or(segment, |h| &segment[h.get(0).unwrap().end()..]);
        let heading = heading
            .map(|h| plain(&BREAK_TAG.replace_all(&h[1], " \u{2013} ")))
            .unwrap_or_default();
        if kind != "article" {
            labels.insert(id, heading);
            continue;
        }
        let (num, title) = head(&heading);
        let mut text = lines(rest);
        if let Some(title) = title {
            text.insert(0, title);
        }
        let path: Vec<&str> = id
            .match_indices("__")
            .filter_map(|(at, _)| labels.get(&id[..at]))
            .map(String::as_str)
            .filter(|l| !l.is_empty())
            .collect();
        out.push((
            id,
            ConsolidatedArticle {
                num,
                title_path: (!path.is_empty()).then(|| path.join(" > ")),
                texte: text.join("\n"),
                nota: None,
            },
        ));
    }
    out.into_iter().map(|(_, a)| a).collect()
}

/// The properties a page's path gives when the page carries none, as an expression with no
/// text renders: `/fr/akn/sn/act/decree/1964/572/fra@2002-06-23`.
fn path_properties(path: &str) -> Result<serde_json::Value, String> {
    let akn = path
        .find("/akn/")
        .map(|at| &path[at..])
        .ok_or_else(|| format!("{path:?} names no work"))?;
    let (work, _) = akn
        .rsplit_once('/')
        .ok_or_else(|| format!("{path:?} names no expression"))?;
    let named: Vec<&str> = work.split('/').skip(4).collect();
    let (subtype, year, number) = match named[..] {
        [subtype, year, number] => (Some(subtype), year, number),
        [year, number] => (None, year, number),
        _ => return Err(format!("{path:?} names no year and number")),
    };
    Ok(serde_json::json!({
        "work_frbr_uri": work,
        "expression_frbr_uri": akn,
        "frbr_uri_subtype": subtype,
        "frbr_uri_date": year,
        "frbr_uri_number": number,
    }))
}

/// The page at `path`; one carrying no properties, as an expression with no text renders, read
/// from its path, its source PDF from its download link.
pub fn peachjam_page(html: &str, path: &str) -> Result<PeachjamPage, String> {
    let carried = PROPERTIES.captures(html);
    let props: serde_json::Value = match &carried {
        Some(props) => {
            serde_json::from_str(&props[1]).map_err(|e| format!("page properties: {e}"))?
        }
        None => path_properties(path)?,
    };
    let field = |name: &str| props[name].as_str().map(str::to_string);
    let work = field("work_frbr_uri").ok_or("no work_frbr_uri")?;
    let expression = field("expression_frbr_uri").ok_or("no expression_frbr_uri")?;
    let (language, expression_date) = expression
        .rsplit_once('/')
        .and_then(|(_, e)| e.split_once('@'))
        .map(|(l, d)| (l.to_string(), d.to_string()))
        .ok_or_else(|| format!("expression {expression:?} carries no date"))?;
    let title = TITLE
        .captures(html)
        .map(|t| plain(&t[1]))
        .filter(|t| !t.is_empty())
        .ok_or("the page has no title")?;
    let mut expressions: Vec<(String, String)> = Vec::new();
    for c in EXPRESSION.captures_iter(html) {
        if c[2] == language && !expressions.iter().any(|(d, _)| d == &c[3]) {
            expressions.push((c[3].to_string(), c[1].to_string()));
        }
    }
    let cover = |kind: &str| {
        COVER
            .captures_iter(html)
            .filter(|c| &c[1] == kind)
            .find_map(|c| COVER_DAY.captures(&plain(&c[2])).map(|d| d[1].to_string()))
            .and_then(|d| lj_core::dates::parse_date_fr(&d))
            .map(|d| d.to_string())
    };
    let pdf = match carried {
        Some(_) => DISPLAY
            .captures(html)
            .is_some_and(|d| &d[1] == "pdf")
            .then(|| PDF.captures(html).map(|p| p[1].to_string()))
            .flatten(),
        None => SOURCE.captures(html).map(|s| s[1].to_string()),
    };
    let body = match (pdf.is_none(), html.find(r#"class="akn-body""#)) {
        (true, Some(start)) => {
            let body = &html[start..];
            let end = [
                r#"class="akn-conclusions""#,
                r#"class="akn-attachments""#,
                "</la-akoma-ntoso>",
            ]
            .iter()
            .filter_map(|mark| body.find(mark))
            .min()
            .unwrap_or(body.len());
            &body[..end]
        }
        _ => "",
    };
    let articles = articles(body);
    let text = match articles.is_empty() {
        true => lines(body.split_once('>').map_or(body, |(_, b)| b)).join("\n"),
        false => String::new(),
    };
    Ok(PeachjamPage {
        work: decoded(&work),
        expression_date,
        language,
        subtype: field("frbr_uri_subtype"),
        year: field("frbr_uri_date").unwrap_or_default(),
        number: decoded(&field("frbr_uri_number").unwrap_or_default()),
        title,
        expressions,
        assent: cover("assent-date"),
        commencement: cover("commencement-date"),
        pdf,
        articles,
        body: text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><h1 id="main-page-heading" class="doc-title d-flex">
      Décret Portant Code de Procédure Civile, 1964</h1>
      <script id="track-page-properties" type="application/json">{"work_frbr_uri": "/akn/sn/act/decree/1964/572", "expression_frbr_uri": "/akn/sn/act/decree/1964/572/fra@2013-10-13", "frbr_uri_subtype": "decree", "frbr_uri_date": "1964", "frbr_uri_number": "572"}</script>
      <ul class="dropdown-menu"><li><a class="dropdown-item active"
        href="/fr/akn/sn/act/decree/1964/572/fra@2013-10-13">13 octobre 2013</a></li>
      <li><a class="dropdown-item "
        href="/fr/akn/sn/act/decree/1964/572/fra@2002-06-23">23 juin 2002</a></li></ul>
      <div id="document-content" data-display-type="akn">
      <la-akoma-ntoso><li class="assent-date">
            Acceptée 30 Juillet 1964
          </li><li class="commencement-date">Commencé le 1 Novembre 1964</li>
      <span class="akn-body"><section class="akn-article" id="art_premier" data-eid="art_premier"><h2>Article premier</h2><span class="akn-content"><span class="akn-p">Sauf en matière pénale, la procédure est réglée par le présent Code.</span></span></section><section class="akn-part" id="part_1"><h2>Partie première &#8211; De la procédure ordinaire</h2><section class="akn-book" id="part_1__book_1"><h2>Livre premier<br>Des tribunaux</h2><section class="akn-article" id="part_1__book_1__art_2"><h2>Article 2 &#8211; Définitions</h2><span class="akn-intro"><span class="akn-p">On entend par:</span></span><section class="akn-paragraph" id="part_1__book_1__art_2__para_a"><span class="akn-num">a)</span><span class="akn-content"><span class="akn-p">« avis » : un avis oral ou écrit;</span></span></section><section class="akn-paragraph" id="part_1__book_1__art_2__para_b"><span class="akn-num">b)</span><span class="akn-content"><span class="akn-p">« écrit » : une suite de lettres.</span><span class="akn-p">Elle est datée.</span></span></section></section><section class="akn-article" id="part_1__book_1__art_56bis"><h2>Article 56 bis</h2><span class="akn-content"><span class="akn-p">Inséré.</span></span></section></section></section></span></la-akoma-ntoso></html>"#;

    #[test]
    fn an_akoma_ntoso_page_reads_its_expressions_cover_and_articles_under_their_containers() {
        let page = peachjam_page(PAGE, "/fr/akn/sn/act/decree/1964/572/fra@2013-10-13").unwrap();
        assert_eq!(page.work, "/akn/sn/act/decree/1964/572");
        assert_eq!(page.expression_date, "2013-10-13");
        assert_eq!(page.title, "Décret Portant Code de Procédure Civile, 1964");
        assert_eq!(
            page.expressions
                .iter()
                .map(|(d, _)| d.as_str())
                .collect::<Vec<_>>(),
            ["2013-10-13", "2002-06-23"]
        );
        assert_eq!(page.assent.as_deref(), Some("1964-07-30"));
        assert_eq!(page.commencement.as_deref(), Some("1964-11-01"));
        assert_eq!(page.pdf, None);
        let path = "Partie première \u{2013} De la procédure ordinaire > Livre premier \u{2013} Des tribunaux";
        let got: Vec<_> = page
            .articles
            .iter()
            .map(|a| (a.num.as_str(), a.title_path.as_deref(), a.texte.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "premier",
                    None,
                    "Sauf en matière pénale, la procédure est réglée par le présent Code."
                ),
                (
                    "2",
                    Some(path),
                    "Définitions\nOn entend par:\na) « avis » : un avis oral ou écrit;\n\
                     b) « écrit » : une suite de lettres.\nElle est datée."
                ),
                ("56 bis", Some(path), "Inséré."),
            ]
        );
    }

    #[test]
    fn a_page_without_properties_is_read_from_its_path_and_its_source_pdf() {
        let html = r#"<h1 id="main-page-heading" class="doc-title d-flex">
          <span>Décret Portant Code de Procédure Civile, 1964</span></h1>
          <a class="dropdown-item "
            href="/fr/akn/sn/act/decree/1964/572/fra@2013-10-13">13 octobre 2013</a>
          <a href="/fr/akn/sn/act/decree/1964/572/fra@2002-06-23/source"
            class="dropdown-item" target="_blank">Télécharger PDF</a>"#;
        let page = peachjam_page(html, "/fr/akn/sn/act/decree/1964/572/fra@2002-06-23").unwrap();
        assert_eq!(page.work, "/akn/sn/act/decree/1964/572");
        assert_eq!(
            (page.language.as_str(), page.expression_date.as_str()),
            ("fra", "2002-06-23")
        );
        assert_eq!(
            (
                page.subtype.as_deref(),
                page.year.as_str(),
                page.number.as_str()
            ),
            (Some("decree"), "1964", "572")
        );
        assert_eq!(page.expressions.len(), 1);
        assert_eq!(
            page.pdf.as_deref(),
            Some("/fr/akn/sn/act/decree/1964/572/fra@2002-06-23/source")
        );
        let act = peachjam_page(html, "/fr/akn/sn/act/1966/70/fra@1994-08-28").unwrap();
        assert_eq!((act.subtype, act.number.as_str()), (None, "70"));
    }

    #[test]
    fn a_heading_gives_the_number_whole_and_the_title_after_it() {
        let head = |h: &str| {
            let (num, title) = head(&h.replace('~', "\u{2013}"));
            (num, title.unwrap_or_default())
        };
        let want = |n: &str, t: &str| (n.to_string(), t.to_string());
        assert_eq!(head("Article 1-1"), want("1-1", ""));
        assert_eq!(head("Article 56 bis"), want("56 bis", ""));
        assert_eq!(head("Article premier-12"), want("premier-12", ""));
        assert_eq!(head("Article 2 ~ Définitions"), want("2", "Définitions"));
        assert_eq!(
            head("Article 8-2. Intervention volontaire"),
            want("8-2", "Intervention volontaire")
        );
        assert_eq!(head("Article 245 ~ 11"), want("245-11", ""));
        assert_eq!(
            head("Article 8 ~ 4 - Pluralité de contrats"),
            want("8-4", "Pluralité de contrats")
        );
    }

    #[test]
    fn a_sitemap_entry_names_its_work_decoded_and_its_place() {
        let xml = "<url><loc>https://senlii.org/fr/akn/aa-ohada/act/2010/droit-commercial-g%C3%A9n%C3%A9ral/fra@2011-02-15</loc><lastmod>2025-11-14</lastmod></url>";
        assert_eq!(
            peachjam_sitemap(xml),
            [PeachjamListed {
                path: "/fr/akn/aa-ohada/act/2010/droit-commercial-g%C3%A9n%C3%A9ral/fra@2011-02-15"
                    .to_string(),
                work: "/akn/aa-ohada/act/2010/droit-commercial-général".to_string(),
                place: "aa-ohada".to_string(),
                modified: Some("2025-11-14".to_string()),
            }]
        );
    }
}
