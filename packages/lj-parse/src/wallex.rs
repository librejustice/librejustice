//! Wallex, the Walloon law portal: one dated version of an act and the versions its page lists.

use std::sync::LazyLock;

use regex::Regex;

use crate::ConsolidatedArticle;

/// A version of an act as its page lists it, its address absent on the version the page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallexVersion {
    pub from: String,
    pub until: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug)]
pub struct WallexPage {
    pub title: String,
    pub adopted: Option<String>,
    /// The day the Moniteur published it, and its PDF there.
    pub published: Option<String>,
    pub moniteur_url: Option<String>,
    pub versions: Vec<WallexVersion>,
    pub articles: Vec<ConsolidatedArticle>,
    /// Articles the page titles `Art. null.` or by a range, `Art. s 1er à 66.`, `Art. 287à 289.`,
    /// `Art. Art. 229 D à 232 D -.`, left out with their text.
    pub unnumbered: usize,
}

static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div id="title" class="header">(.*?)</div>"#).unwrap());
static DATED_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{1,2})(?:er)?\s+(janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre)\s+(\d{4})\s*-\s*(.*)$").unwrap()
});
static PUBLISHED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"M\.B\. du (\d{2})/(\d{2})/(\d{4})").unwrap());
static MONITEUR_PDF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"href="(https?://www\.ejustice\.just\.fgov\.be/+mopdf/[^"]+)""#).unwrap()
});
static DATE_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div id="date-list"[^>]*>(.*?)</ul>"#).unwrap());
static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<li class="(?:not-)?current">\s*(?:<a href="([^"]+)">)?\s*<span>\s*Du\s+(\d{2})/(\d{2})/(\d{4})\s+au\s+(?:(\d{2})/(\d{2})/(\d{4}))?"#).unwrap()
});
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<div class="legal-context">\s*<div id="[^"]*" class="([^"]+)">(.*?)</div>|<p class="article-title">(.*?)</p>"#).unwrap()
});
static ANNEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<div class="annex" id=""#).unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</h\d>|</li>|</tr>|</table>").unwrap());
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
/// `Art. L1232-12 /1.`, `Art. ( L1123-31 .`, `Article 1er.`, `Art. 2 bis.`, `Art. ( 31quater* .`,
/// `Art. LITXVIIICIII2.` and the text after it. The word « Article » the page sometimes leaves cut
/// or repeated before the number is dropped: `Art. ICLE1er.`, `Art. Art. 16.`.
static ARTICLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Art(?:icle)?\.?\s*(?:[(\[«]\s*)*(?:Art\.\s*)?(?i:a?r?t?icle)?\s*((?:L[IVX]+(?:[TCS][IVX]+(?:bis|ter|quater)?)*|\.|(?:[A-Z]{1,2}[.\-]?\s?)?(?:[IVXLC]+[.\-]?)?)\d+(?:[\s.]?(?:er|ère)?\s?(?:bis|ter|quater|[a-z]{2,16}ies)\b|\s?(?:er|ère)\b)?(?:(?:\.?\s?/\s?\(?\s?|-\s?|\.)\d+(?:\s?(?:bis|ter|quater|[a-z]{2,16}ies)\b)?)*|[DR]\.?[IVX]+\.?I|unique|préliminaire)\*?\.?(?:\s*[-\x{2013}]\s*[^()]*\))?\s*[)\]»]?(?:\s*-?\s*(?:\((?:\d|Coordination[^)]*)\)|\[\s*(?:A\.G\.W\.|D\.|Décret|entr)[^\]]*\]))*(?:,\s*articles\s+\d\S*\s+à\s+\d\S*?)?\s*\.{0,2}(?:\s+(.*))?$").unwrap()
});
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\(?(partie|livre|titre|chapitre|section|sous-section)\s+(?:[IVXLC]+|\d+|premier|première|unique|préliminaire)\b").unwrap()
});
/// The amendment that inserted a heading: `- Décret du 11 avril 2024, art.8).`.
static HEADING_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\s+-\s+|\s*\()(?:Décret|Arrêté|Loi|Ordonnance)\b[^()]*\)\.?\s*$").unwrap()
});
static UNNUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Art\.\s*(?:Art\.\s*)?(?:\(\s*)?(?:null|\d+/|(?:s\s+)?(?:[A-Z]{1,2}\.?)?\d\S*?(?:\s[A-Z])?\s*à\s+(?:[A-Z]{1,2}\.?)?\d\S*?(?:\s[A-Z])?(?:\s?(?:bis|ter|quater|[a-z]{2,16}ies))?)[\s.\-]*$").unwrap()
});
/// A Walloon code's number, `R.II.36-4`, however the page dots it: `RII36-4`, `R.II-36-4`, `R-I36-1`.
static CODE_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([DR])[.\-]?([IVX]+)[.\-]?(\d.*)$").unwrap());
/// `D.IV.I` or `DIVI`, the page's typo for `D.IV.1`.
static CODE_ONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([DR])\.?([IVX]+?)\.?I$").unwrap());
static DOTTED_LATIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d)\.(bis|ter|quater|[a-z]{2,16}ies)\b").unwrap());
static PDF_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\s[^>]*href="[^"]*\.pdf"[^>]*>.*?</a>"#).unwrap());
static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{1,2}\.").unwrap());
static ANNEX_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^Annexer?\s+(?:n°\s*)?([IVXLC]+|\d+|[A-Z])(?:er|ère|re)?(?:\s?(bis|ter|quater|quinquies|sexies|septies|octies|no[nv]ies|decies))?\b").unwrap()
});

const LEVELS: [&str; 6] = ["part", "book", "title", "chapter", "section", "subsection"];
const MONTHS: [&str; 12] = [
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
];

fn iso(day: &str, month: &str, year: &str) -> Option<String> {
    let date = jiff::civil::Date::new(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    date.ok().map(|d| d.to_string())
}

/// The lines of a fragment, its tags gone and its spaces collapsed.
fn lines(fragment: &str) -> Vec<String> {
    let text = BREAK.replace_all(fragment, "\n");
    let text = CELL.replace_all(&text, " ");
    let text = TAG.replace_all(&text, "");
    html_escape::decode_html_entities(&text)
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect()
}

fn heading_label(line: &str) -> String {
    HEADING_NOTE
        .replace(line.trim_start_matches('('), "")
        .trim()
        .to_string()
}

/// The level of a heading line inside a text, none for a line that ends a sentence.
fn heading_level(line: &str) -> Option<usize> {
    if line.len() > 300 || line.ends_with(['.', ';', ':', ',']) && !HEADING_NOTE.is_match(line) {
        return None;
    }
    let word = HEADING.captures(line)?[1].to_lowercase();
    Some(match word.as_str() {
        "partie" => 0,
        "livre" => 1,
        "titre" => 2,
        "chapitre" => 3,
        "section" => 4,
        _ => 5,
    })
}

struct Reader {
    path: Vec<Option<String>>,
    articles: Vec<ConsolidatedArticle>,
    open: bool,
    /// The last heading names only its level, `Titre premier`, its name on the next line.
    bare: bool,
    unnumbered: usize,
}

impl Reader {
    fn head(&mut self, level: usize, label: String) {
        self.bare = HEADING.find(&label).is_some_and(|m| m.end() == label.len());
        self.path.resize(level, None);
        self.path.push(Some(label));
        self.open = false;
    }

    fn article(&mut self, num: &str, first: Option<&str>) {
        let path: Vec<&str> = self.path.iter().flatten().map(String::as_str).collect();
        let mut num: String = num.split_whitespace().collect::<String>().replace('(', "");
        if let Some(bare) = num.strip_prefix('.') {
            // `Art. .47-2.` after `R.47-1`: the page dropped the letter of the article before.
            let letter = self.articles.last().and_then(|a| LETTER.find(&a.num));
            num = format!("{}{bare}", letter.map_or("", |m| m.as_str()));
        }
        let num = CODE_NUM.replace(&num, "$1.$2.$3").replace("./", "/");
        let num = DOTTED_LATIN.replace(&num, "$1$2");
        self.articles.push(ConsolidatedArticle {
            num: CODE_ONE.replace(&num, "$1.$2.1").into_owned(),
            title_path: (!path.is_empty()).then(|| path.join(" > ")),
            texte: first.unwrap_or_default().to_string(),
            nota: None,
        });
        self.open = true;
        self.bare = false;
    }

    fn line(&mut self, line: &str) {
        if let Some(c) = ARTICLE.captures(line) {
            self.article(&c[1], c.get(2).map(|m| m.as_str()));
        } else if let Some(level) = heading_level(line) {
            self.head(level, heading_label(line));
        } else if self.bare {
            if let Some(Some(label)) = self.path.last_mut() {
                label.push(' ');
                label.push_str(&heading_label(line));
            }
            self.bare = false;
        } else if self.open {
            let texte = &mut self.articles.last_mut().expect("an open article").texte;
            if !texte.is_empty() {
                texte.push('\n');
            }
            texte.push_str(line);
        }
    }
}

/// The numbered articles of the act's body, a heading its markup or its text names opening a level.
fn articles(body: &str) -> Result<Reader, String> {
    let mut reader = Reader {
        path: Vec::new(),
        articles: Vec::new(),
        open: false,
        bare: false,
        unnumbered: 0,
    };
    let mut at = 0;
    for block in BLOCK.captures_iter(body) {
        let whole = block.get(0).expect("a match");
        for line in lines(&body[at..whole.start()]) {
            reader.line(&line);
        }
        at = whole.end();
        if let (Some(kind), Some(label)) = (block.get(1), block.get(2)) {
            let level = LEVELS
                .iter()
                .position(|l| *l == kind.as_str())
                .ok_or_else(|| format!("a heading of the unknown kind {:?}", kind.as_str()))?;
            reader.head(level, heading_label(&lines(label.as_str()).join(" ")));
        } else if let Some(title) = block.get(3) {
            let title = lines(title.as_str()).join(" ");
            if UNNUMBERED.is_match(&title) {
                reader.open = false;
                reader.bare = false;
                reader.unnumbered += 1;
                continue;
            }
            let c = ARTICLE
                .captures(&title)
                .ok_or_else(|| format!("an article titled {title:?}"))?;
            reader.article(&c[1], c.get(2).map(|m| m.as_str()));
        }
    }
    for line in lines(&body[at..]) {
        reader.line(&line);
    }
    Ok(reader)
}

/// Each annex with a text of its own, numbered by its heading or else by its place; the label of a
/// link to a PDF is no text.
fn annexes(block: &str) -> Vec<ConsolidatedArticle> {
    let parts: Vec<&str> = ANNEX.split(block).skip(1).collect();
    let alone = parts.len() == 1;
    parts
        .into_iter()
        .enumerate()
        .filter_map(|(i, part)| {
            let content = part.split_once('>').map_or(part, |(_, rest)| rest);
            let mut text = lines(&PDF_LINK.replace_all(content, ""));
            let label = text.first().and_then(|l| ANNEX_LABEL.captures(l));
            let num = match &label {
                Some(c) => match c.get(2) {
                    Some(suffix) => format!("Annexe {} {}", &c[1], suffix.as_str().to_lowercase()),
                    None => format!("Annexe {}", &c[1]),
                },
                None if alone => "Annexe".to_string(),
                None => format!("Annexe {}", i + 1),
            };
            if let Some(end) = label.map(|c| c.get(0).expect("a match").end()) {
                let rest = text[0][end..]
                    .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '-' | '.'))
                    .to_string();
                if rest.is_empty() {
                    text.remove(0);
                } else {
                    text[0] = rest;
                }
            }
            (!text.is_empty()).then(|| ConsolidatedArticle {
                num,
                title_path: None,
                texte: text.join("\n"),
                nota: None,
            })
        })
        .collect()
}

/// An article the page shows twice, in two wordings that apply side by side, keeps both in turn.
fn merged(articles: Vec<ConsolidatedArticle>) -> Vec<ConsolidatedArticle> {
    let mut out: Vec<ConsolidatedArticle> = Vec::with_capacity(articles.len());
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for a in articles {
        match at.get(&a.num) {
            Some(&i) => {
                let texte = &mut out[i].texte;
                texte.push('\n');
                texte.push_str(&a.texte);
            }
            None => {
                at.insert(a.num.clone(), out.len());
                out.push(a);
            }
        }
    }
    out
}

/// An act's page on Wallex: its title, the versions it lists, and the articles of the one it shows.
pub fn wallex_page(html: &str) -> Result<WallexPage, String> {
    let header = TITLE
        .captures(html)
        .map(|c| lines(&c[1]).join(" "))
        .ok_or("the page has no title")?;
    let (title, adopted) = match DATED_TITLE.captures(&header) {
        Some(c) => {
            let month = MONTHS.iter().position(|m| *m == &c[2]).expect("a month") + 1;
            (
                c[4].trim().to_string(),
                iso(&c[1], &month.to_string(), &c[3]),
            )
        }
        None => (header.trim().to_string(), None),
    };
    let listed = DATE_LIST
        .captures(html)
        .ok_or("the page lists no version")?;
    let versions = VERSION
        .captures_iter(&listed[1])
        .map(|c| {
            Ok(WallexVersion {
                from: iso(&c[2], &c[3], &c[4])
                    .ok_or_else(|| format!("a version from {}/{}/{}", &c[2], &c[3], &c[4]))?,
                until: c.get(7).and_then(|_| iso(&c[5], &c[6], &c[7])),
                path: c.get(1).map(|m| m.as_str().to_string()),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if versions.iter().all(|v| v.path.is_some()) {
        return Err("the page lists no version it shows".to_string());
    }
    let start = ["<div class=\"legal-context\">", "class=\"dispositif\""]
        .iter()
        .filter_map(|m| html.find(m))
        .min()
        .ok_or("the page has no body")?;
    let aside = html[start..]
        .find("<aside")
        .map_or(html.len(), |i| start + i);
    let annexes_at = html[start..aside]
        .find("class=\"annexes\"")
        .map(|i| start + i);
    let end = html[start..aside]
        .find("id=\"final\"")
        .map(|i| start + i)
        .or(annexes_at)
        .map_or(aside, |at| html[..at].rfind('<').expect("an open tag"));
    let Reader {
        mut articles,
        unnumbered,
        ..
    } = articles(&html[start..end])?;
    if let Some(at) = annexes_at {
        articles.extend(annexes(&html[at..aside]));
    }
    Ok(WallexPage {
        title,
        adopted,
        published: PUBLISHED
            .captures(html)
            .and_then(|c| iso(&c[1], &c[2], &c[3])),
        moniteur_url: MONITEUR_PDF.captures(html).map(|c| c[1].to_string()),
        versions,
        articles: merged(articles),
        unnumbered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_article_titled_null_is_left_out_with_its_text() {
        let body = r#"<p class="article-title">Art. 5.</p><p>Cinq.</p><p class="article-title">Art. null.</p><p>Sans numéro.</p><p class="article-title">Art. 8.</p><p>Huit.</p>"#;
        let read = articles(body).unwrap();
        let nums: Vec<(&str, &str)> = read
            .articles
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str()))
            .collect();
        assert_eq!(nums, [("5", "Cinq."), ("8", "Huit.")]);
        assert_eq!(read.unnumbered, 1);
    }

    #[test]
    fn a_code_number_is_dotted_one_way_and_a_range_of_articles_is_left_out() {
        let body = [
            "R.II-36-4.",
            "RVIII4-1.",
            "R-I36-1.",
            "R.IV.36-1..",
            "R.I.1.1.",
            "s 1<sup>er</sup> à 66.",
            "287à 289.",
            "Art. 229 D à 232 D -.",
            "L1232-12 /1.",
            "1/.",
            "60 à 63 bis .",
            "( 344 à 380 .",
            "D.IV.I.",
            "DIVI.",
            "255/ (27 \u{2013} AGW du 17juillet 2003, art1er) .",
            "D.VI.24./1.",
            "R.47-1.",
            ".47-2.",
            "[R.304.bis.] [A.G.W. 01.12.2016 - entrée en vigueur au 01.01.2018] .",
            "[R.307bis-14.][A.G.W. 18.07.2019][entre en vigueur le 1er juin 2021] .",
            "[R.320bis-2.](Coordination officieuse du n° de l'annexe) [A.G.W. 12.02.2009] .",
        ]
        .map(|t| format!(r#"<p class="article-title">Art. {t}</p><p>Texte.</p>"#))
        .concat();
        let read = articles(&body).unwrap();
        let nums: Vec<&str> = read.articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(
            nums,
            [
                "R.II.36-4",
                "R.VIII.4-1",
                "R.I.36-1",
                "R.IV.36-1",
                "R.I.1.1",
                "L1232-12/1",
                "D.IV.1",
                "D.IV.1",
                "255/27",
                "D.VI.24/1",
                "R.47-1",
                "R.47-2",
                "R.304bis",
                "R.307bis-14",
                "R.320bis-2"
            ]
        );
        assert_eq!(read.unnumbered, 6);
    }

    #[test]
    fn an_article_number_reads_past_a_cut_article_word_a_star_and_an_ordinal_after_er() {
        for (title, num, text) in [
            ("Art. ICLE1er.", "1er", None),
            ("Art. Aricle 1er.", "1er", None),
            ("Art. Icle1er.", "1er", None),
            ("Art. ( 31quater* .", "31quater", None),
            ("Art. ( 1erbis .", "1erbis", None),
            (
                "Art. 1er bis. Le Gouvernement",
                "1er bis",
                Some("Le Gouvernement"),
            ),
            ("Art. 1er. Le Gouvernement", "1er", Some("Le Gouvernement")),
            ("Art. préliminaire.", "préliminaire", None),
            ("Art. 48quinquiesdecies.", "48quinquiesdecies", None),
            ("Art. Art. 16.", "16", None),
            ("Art. LITXVIIICIII1er.", "LITXVIIICIII1er", None),
            ("Art. [R.42bis.] [ A.G.W. 13.09.2012].", "R.42bis", None),
            ("Art. [R.42ter.] [ A.G.W. 13.09.2012] .", "R.42ter", None),
            ("Art. 152, articles 153 à 222.", "152", None),
            ("Art. LITVCIIISI1er.", "LITVCIIISI1er", None),
            ("Art. LITIIICIXbis1er.", "LITIIICIXbis1er", None),
            (
                "Art. [Art. R.270 bis-10.](1) (1)[A.G.W. 14.07.2005] .",
                "R.270 bis-10",
                None,
            ),
        ] {
            let c = ARTICLE.captures(title).expect(title);
            assert_eq!(
                (&c[1], c.get(2).map(|m| m.as_str())),
                (num, text),
                "{title}"
            );
        }
    }

    #[test]
    fn an_annex_keeps_its_latin_ordinal_and_one_served_as_a_pdf_is_left_out() {
        let html = r#"<div class="annex" id="a"><div class="annex-content"><b>Annexe XIX</b><br>Dix-neuf.</div></div><div class="annex" id="b"><div class="annex-content"><b>Annexe XIX bis</b><br>Insérée.</div></div><div class="annex" id="c"><div class="annex-content"><b>Annexe 3bis</b> : Liste</div></div><div class="annex" id="d"><div class="annex-content"><b>Annexe 4</b>.<br>Quatre.</div></div><div class="annex" id="e"><div class="annex-content"><b><a href="/files/medias/3/4134.pdf" target="_blank">Annexe 5<br>Plan</a></b></div></div><div class="annex" id="f"><div class="annex-content"><b>Annexe 6</b> <a href="/files/6.PDF">Carte</a></div></div>"#;
        let read: Vec<(String, String)> = annexes(html)
            .into_iter()
            .map(|a| (a.num, a.texte))
            .collect();
        assert_eq!(
            read,
            [
                ("Annexe XIX".to_string(), "Dix-neuf.".to_string()),
                ("Annexe XIX bis".to_string(), "Insérée.".to_string()),
                ("Annexe 3 bis".to_string(), "Liste".to_string()),
                ("Annexe 4".to_string(), "Quatre.".to_string()),
            ]
        );
    }

    #[test]
    fn an_article_title_sheds_stray_quotes_and_its_amendment_note() {
        for (title, num) in [
            ("Art. \u{ab}\u{a0}\u{a0}152/5.", "152/5"),
            (
                "Art. ( ( 7 ter \u{2013} Décret du 1er juin 2017, art 4) .",
                "7 ter",
            ),
            ("Art. ( L1123-31 .", "L1123-31"),
            ("Art. ( 81quinties .", "81quinties"),
            ("Art. 81undecies.", "81undecies"),
            (
                "Art. ( (32 \u{2013} AGW du 11 septembre 2008, art 8 ) .",
                "32",
            ),
        ] {
            assert_eq!(&ARTICLE.captures(title).expect(title)[1], num);
        }
    }

    #[test]
    fn a_page_reads_its_versions_headings_articles_and_annexes() {
        let html = r#"<div id="title" class="header"> <span class="text"> 22 avril 2004 - </span> <span class="text">Code de la démocratie locale et de la décentralisation (CDLD) </span> </div>
<div id="date-list" class="data-content"> <ul>
<li class="not-current"> <a href="/eli/loi-decret/2004/04/22/200427184/2027/01/01"> <span> Du 01/01/2027 au ...</span> </a> </li>
<li class="current"> <span> Du 06/09/2026 au 31/12/2026</span> </li>
</ul></div>
<div class="metadata"> <a class="wallex-tooltip" target="_blank" href="http://www.ejustice.just.fgov.be//mopdf/2004/08/31_1.pdf#Page5" title="Lien">Lien</a> </div>
<div class="metadata"> <div> M.B. du 31/08/2004, p. 64709 </div> <div> (Numac : 2004027184)</div> </div>
<div class="dispositif"><div class="legal-context"> <div id="a" class="part"> <span>Partie I<br> LES COMMUNES</span> </div> </div>
<div><p class="article-paragraph"><h3><b>Chapitre premier</b><br>Noms</h3><br><span><b>Art. L1111-1.</b> Le Gouvernement détermine l&#8217;orthographe des noms.</span><br>
<span><b>Art. L1232-12 /1.</b> (§1er. Le gestionnaire public.</span><br><span>§2. Il affiche.</span><br>
<span>(Sous-section 3 Caveaux d'attente - Décret du 11 avril 2024, art.8).</span><br>
<span><b>Art. L1523- 27.</b> ((...) - Décret du 14 mars 2024, art.30)</span><br>
<span><b>Art. L1111-1.</b> Le collège est responsable.</span></p></div>
<div class="body"><div class="article" id="b"><p class="article-title">Art. 1 er. </p><div class="article-paragraphs"><div><p class="article-paragraph">Le présent Livre s'applique.</p></div></div></div></div>
<div class="body"><div class="article" id="e"><p class="article-title">Art. ( L1123-31 .</p><div class="article-paragraphs"><div><p class="article-paragraph">Inséré.</p></div></div></div></div>
</div><div id="final" class="footer"> Namur, le 22 avril 2004. </div>
<div class="annexes"> <div class="annex" id="c"> <div class="annex-content"><b>Annexe 1<sup>ère</sup></b>&nbsp;: Plafonds<br><br>1° plafond 1: € 5.713,47;</div></div>
<div class="annex" id="d"> <div class="annex-content"><b><a href="/files/medias/3/3762.pdf">Annexe II</a></b></div></div></div>
</div><aside class="col-lg-4">"#;
        let page = wallex_page(html).unwrap();
        assert_eq!(
            page.title,
            "Code de la démocratie locale et de la décentralisation (CDLD)"
        );
        assert_eq!(page.adopted.as_deref(), Some("2004-04-22"));
        assert_eq!(page.published.as_deref(), Some("2004-08-31"));
        assert_eq!(
            page.moniteur_url.as_deref(),
            Some("http://www.ejustice.just.fgov.be//mopdf/2004/08/31_1.pdf#Page5")
        );
        assert_eq!(
            page.versions,
            vec![
                WallexVersion {
                    from: "2027-01-01".to_string(),
                    until: None,
                    path: Some("/eli/loi-decret/2004/04/22/200427184/2027/01/01".to_string()),
                },
                WallexVersion {
                    from: "2026-09-06".to_string(),
                    until: Some("2026-12-31".to_string()),
                    path: None,
                },
            ]
        );
        let nums: Vec<&str> = page.articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(
            nums,
            [
                "L1111-1",
                "L1232-12/1",
                "L1523-27",
                "1er",
                "L1123-31",
                "Annexe 1"
            ]
        );
        let a = &page.articles[0];
        assert_eq!(
            a.title_path.as_deref(),
            Some("Partie I LES COMMUNES > Chapitre premier Noms")
        );
        assert_eq!(
            a.texte,
            "Le Gouvernement détermine l\u{2019}orthographe des noms.\nLe collège est responsable."
        );
        assert_eq!(
            page.articles[1].texte,
            "(§1er. Le gestionnaire public.\n§2. Il affiche."
        );
        assert_eq!(
            page.articles[2].title_path.as_deref(),
            Some(
                "Partie I LES COMMUNES > Chapitre premier Noms > Sous-section 3 Caveaux d'attente"
            )
        );
        assert_eq!(page.articles[3].texte, "Le présent Livre s'applique.");
        assert_eq!(
            page.articles[5].texte,
            "Plafonds\n1° plafond 1: € 5.713,47;"
        );
    }
}
