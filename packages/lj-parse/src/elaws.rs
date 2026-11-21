//! Ontario's e-Laws: the versions its sitemaps list, and the articles of one version's page.

use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{Child, ConsolidatedArticle, Tree};

/// The number of recitals coded as a section, in both languages.
const PREAMBLE: &str = "Préambule";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ElawsKind {
    Statute,
    Regulation,
}

impl ElawsKind {
    /// The URL segment of its English pages.
    pub fn english(self) -> &'static str {
        match self {
            Self::Statute => "statute",
            Self::Regulation => "regulation",
        }
    }

    /// The URL segment of its French pages.
    pub fn french(self) -> &'static str {
        match self {
            Self::Statute => "loi",
            Self::Regulation => "reglement",
        }
    }

    fn of(segment: &str) -> Option<Self> {
        match segment {
            "statute" | "loi" => Some(Self::Statute),
            "regulation" | "reglement" => Some(Self::Regulation),
            _ => None,
        }
    }
}

/// A page a sitemap lists, its version `None` when current; the French sitemap gives no title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElawsListed {
    pub kind: ElawsKind,
    pub key: String,
    pub version: Option<u32>,
    pub title: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub lastmod: String,
}

/// `https://www.ontario.ca/laws/statute/92c06/v3`, `https://www.ontario.ca/lois/reglement/900194`.
static LOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://www\.ontario\.ca/(?:laws|lois)/([a-z]+)/([0-9a-z]+)(?:/v(\d+))?/?$")
        .unwrap()
});

/// The consolidated pages of a sitemap, its other pages left out.
pub fn elaws_sitemap(xml: &str) -> Result<Vec<ElawsListed>, String> {
    let tree = Tree::parse(xml)?;
    let mut listed = Vec::new();
    for i in 0..tree.nodes.len() {
        if tree.nodes[i].tag != "url" {
            continue;
        }
        let field = |tag: &str| {
            tree.nodes[i]
                .children
                .iter()
                .find_map(|c| match c {
                    Child::Node(n) if tree.nodes[*n].tag == tag => Some(*n),
                    _ => None,
                })
                .map(|c| tree.text(c, &[]).trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let loc = field("loc").ok_or("a url without its loc")?;
        let Some(c) = LOC.captures(&loc) else {
            continue;
        };
        let Some(kind) = ElawsKind::of(&c[1]) else {
            continue;
        };
        let version = c
            .get(3)
            .map(|v| v.as_str().parse::<u32>())
            .transpose()
            .map_err(|e| format!("{loc}: {e}"))?;
        listed.push(ElawsListed {
            kind,
            key: c[2].to_string(),
            version,
            title: field("title").unwrap_or_default(),
            from: field("datefrom"),
            to: field("dateto"),
            lastmod: field("lastmod").unwrap_or_default(),
        });
    }
    Ok(listed)
}

static BODY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<div class="?WordSection1"?[^>]*>"#).unwrap());
static TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<table\b[^>]*>.*?</table>").unwrap());
static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr>").unwrap());
static CELL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<t[dh]\b[^>]*>(.*?)</t[dh]>").unwrap());
static PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<p class="?([^" >]*)"?[^>]*>(.*?)</p>"#).unwrap());
/// The number a section opens on, bold after its anchor: `<a name="BK3"></a><strong>1.02</strong>`.
static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)^\s*(?:<a\b[^>]*>\s*</a>\s*|<(?:strong|b)>\s*</(?:strong|b)>\s*)*<(?:strong|b)>([^<]*?\S[^<]*?)</(?:strong|b)>",
    )
    .unwrap()
});
/// A section's number unbolded: `12.1 (1) The Minister`.
static LEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)\b").unwrap());
static INLINE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(a|b|em|i|span|strong|sub|sup|u)\b[^>]*>").unwrap());
static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

/// The text of an HTML fragment, whitespace folded.
fn plain(html: &str) -> String {
    let html = BREAK.replace_all(html, " ");
    let html = INLINE_TAG.replace_all(&html, "");
    let bare = TAG.replace_all(&html, " ");
    html_escape::decode_html_entities(&bare)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A body table as rows of cells apart, a table of contents as nothing.
fn rows(table: &str) -> String {
    if table.contains("TOCid") {
        return String::new();
    }
    ROW.captures_iter(table)
        .map(|r| {
            let cells: Vec<String> = CELL
                .captures_iter(&r[1])
                .map(|c| plain(&c[1]))
                .filter(|c| !c.is_empty())
                .collect();
            format!("<p class=\"row\">{}</p>", cells.join(" | "))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Skip,
    /// A part's number or a heading, at its level.
    Heading(usize),
    Headnote,
    Section,
    /// A history note, or a text not yet in force and its notice.
    Note,
    Line,
}

/// A class without its language (`section-e`, `Ssubsection-f0`), then its role: `S` opens a
/// schedule's class, `Y` a text not yet in force.
fn role(class: &str) -> Role {
    let base = class.trim_end_matches('0');
    let base = base
        .strip_suffix("-e")
        .or_else(|| base.strip_suffix("-f"))
        .unwrap_or(base);
    if base.starts_with('Y') || matches!(base, "note" | "Pnote" | "footnoteLeft") {
        return Role::Note;
    }
    let base = base.strip_prefix('S').unwrap_or(base);
    match base {
        "leadText"
        | "MsoNormal"
        | "shorttitle"
        | "chapter"
        | "regnumber"
        | "regtitle"
        | "ConsolidationPeriod"
        | "comment"
        | "toc"
        | "version" => Role::Skip,
        _ if base.starts_with("TOC") => Role::Skip,
        "partnum" | "partnumRepeal" | "partheading" | "schedule" => Role::Heading(0),
        "heading1" | "Pheading1" => Role::Heading(1),
        _ if base.contains("heading") => Role::Heading(2),
        "headnote" | "headnoteitalic" => Role::Headnote,
        "section" | "sectionRepeal" => Role::Section,
        _ => Role::Line,
    }
}

struct Article {
    num: String,
    title_path: Option<String>,
    lines: Vec<String>,
    nota: Vec<String>,
}

fn is_schedule(heading: &str) -> bool {
    let upper = heading.to_uppercase();
    upper.starts_with("SCHEDULE") || upper.starts_with("ANNEXE")
}

/// The articles of one version's page, in page order, a schedule's sections under its heading.
pub fn elaws_articles(html: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let start = BODY
        .find(html)
        .ok_or("a page without its WordSection1 body")?
        .end();
    let body = TABLE.replace_all(&html[start..], |c: &regex::Captures| rows(&c[0]));
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut schedule: Option<String> = None;
    let (mut after_heading, mut after_part) = (false, false);
    let mut pending: Vec<String> = Vec::new();
    let mut articles: Vec<Article> = Vec::new();
    for p in PARAGRAPH.captures_iter(&body) {
        let class = &p[1];
        if class.starts_with("ontario") {
            break;
        }
        let inner = &p[2];
        let text = plain(inner);
        if text.is_empty() {
            continue;
        }
        let role = match role(class) {
            Role::Section if NUMBER.find(inner).is_none() && !LEADING.is_match(&text) => Role::Line,
            role => role,
        };
        match role {
            Role::Skip => {}
            Role::Heading(level) => {
                let title_of_part = class.starts_with("partheading") && after_part;
                let level = match class.starts_with("partheading") {
                    true => level + 1,
                    false => level,
                };
                match headings.last_mut() {
                    Some((_, part)) if title_of_part => *part = format!("{part} - {text}"),
                    _ => {
                        headings.retain(|(l, _)| *l < level);
                        headings.push((level, text.clone()));
                    }
                }
                after_part = class.starts_with("partnum");
                if is_schedule(&text) {
                    schedule = Some(text);
                } else if level == 0 && !title_of_part {
                    schedule = None;
                }
                after_heading = true;
                pending.clear();
            }
            Role::Headnote => {
                pending.push(text);
                after_part = false;
            }
            Role::Section => {
                let (num, rest) = match (NUMBER.captures(inner), LEADING.captures(&text)) {
                    (Some(n), _) => (
                        plain(&n[1]).trim_end_matches('.').trim().to_string(),
                        plain(&inner[n.get(0).expect("a match").end()..]),
                    ),
                    (None, Some(n)) => (n[1].to_string(), text[n[0].len()..].trim().to_string()),
                    (None, None) => unreachable!("a section without a number reads as a line"),
                };
                let num = match (&schedule, class.starts_with('S')) {
                    (Some(s), true) => format!("{s}, {num}"),
                    _ => num,
                };
                let title_path = (!headings.is_empty()).then(|| {
                    headings
                        .iter()
                        .map(|(_, h)| h.as_str())
                        .collect::<Vec<_>>()
                        .join(" > ")
                });
                let mut lines = std::mem::take(&mut pending);
                lines.extend((!rest.is_empty()).then_some(rest));
                articles.push(Article {
                    num,
                    title_path,
                    lines,
                    nota: Vec::new(),
                });
                (after_heading, after_part) = (false, false);
            }
            Role::Note => {
                if let Some(a) = articles.last_mut() {
                    a.nota.push(text);
                }
            }
            Role::Line => {
                if articles.is_empty() || after_heading {
                    let num = headings
                        .last()
                        .map_or_else(|| PREAMBLE.to_string(), |(_, h)| h.clone());
                    articles.push(Article {
                        num,
                        title_path: None,
                        lines: Vec::new(),
                        nota: Vec::new(),
                    });
                    after_heading = false;
                }
                after_part = false;
                let a = articles.last_mut().expect("an article is open");
                a.lines.append(&mut pending);
                a.lines.push(text);
            }
        }
    }
    let mut out: Vec<ConsolidatedArticle> = Vec::new();
    for a in articles {
        let texte = a.lines.join("\n");
        let nota = (!a.nota.is_empty()).then(|| a.nota.join("\n"));
        match out.iter_mut().find(|o| o.num == a.num) {
            Some(first) => {
                first.texte = format!("{}\n{texte}", first.texte);
                first.nota = match (first.nota.take(), nota) {
                    (Some(x), Some(y)) => Some(format!("{x}\n{y}")),
                    (x, y) => x.or(y),
                };
            }
            None => out.push(ConsolidatedArticle {
                num: a.num,
                title_path: a.title_path,
                texte,
                nota,
            }),
        }
    }
    Ok(out)
}

static LEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<p class="?leadText[^>]*>(.*?)</p>"#).unwrap());
static SHORT_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<p class="?shorttitle[^>]*>(.*?)</p>"#).unwrap());
static REG_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<p class="?regtitle[^>]*>(.*?)</p>"#).unwrap());
/// `S.O. 1992, c. 6`, `L.R.O. 1990, chap. C.12`.
static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b((?:R\.S\.O\.|S\.O\.|L\.R\.O\.|L\.O\.) \d{4}, (?:c\.|chap\.) [0-9A-Za-z.]*[0-9A-Za-z])",
    )
    .unwrap()
});

/// A title in capitals in the case of a sentence.
fn sentence(title: &str) -> String {
    if title.chars().any(char::is_lowercase) {
        return title.to_string();
    }
    let lower = title.to_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A page's title and citation in its language: a statute's short title, a regulation's title
/// and number, `(Ontario)` added when the name does not say it.
pub fn elaws_title(html: &str) -> Option<String> {
    let text = |re: &Regex| {
        re.captures(html)
            .map(|c| plain(&c[1]))
            .filter(|t| !t.is_empty())
    };
    let lead = text(&LEAD).unwrap_or_default();
    let (name, citation) = match text(&REG_TITLE) {
        Some(title) => (
            sentence(&title),
            lead.split_once(':').map(|(n, _)| n.trim().to_string()),
        ),
        None => (
            text(&SHORT_TITLE)?,
            CITATION.captures(&lead).map(|c| c[1].to_string()),
        ),
    };
    let place = match name.contains("Ontario") {
        true => "",
        false => " (Ontario)",
    };
    Some(match citation {
        Some(c) => format!("{name}{place}, {c}"),
        None => format!("{name}{place}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r##"<html><body><div class=WordSection1>
<p class="MsoNormal"><a name="Top"></a></p>
<p class="shorttitle"></br>Class Proceedings Act, 1992</p>
<p class="leadText">Class Proceedings Act, 1992, S.O. 1992, c. 6</p>
<p class="version-e">This is the English version of a bilingual regulation.</p>
<table><tr><td><p class="TOCid"><a href="#BK0">1.</a></p></td><td><p class="table">Definitions</p></td></tr></table>
<p class="partnum">PART I</p>
<p class="partheading">CERTIFICATION</p>
<p class="headnote">Definitions</p>
<p class="section"><a name="BK0"></a><strong></strong><strong>1 </strong>(1) In this Act,</p>
<p class="definition">&ldquo;court&rdquo; means the Superior Court of Justice;</p>
<p class="footnoteLeft">1992, c. 6, s. 1; 2020, c. 11, Sched. 4, s. 1.</p>
<p class="heading1">Costs</p>
<p class="headnote">Costs</p>
<p class="section"><b>31</b> (1) In exercising its discretion,</p>
<p class="subsection">  (2) Class members are not liable for costs.</p>
<p class="Yfootnote">Section 31 is amended on a day to be named.</p>
<p class="schedule">SCHEDULE 1</p>
<p class="Ssection-e"><strong>2</strong> The Minister may act.</p>
<p class="ontarioFooter">Back to top</p>
<p class="section"><strong>99</strong> After the footer.</p>
</div></body></html>"##;

    #[test]
    fn a_page_reads_as_its_sections_under_their_headings_a_schedule_s_numbered_under_it() {
        let articles = elaws_articles(PAGE).unwrap();
        let nums: Vec<&str> = articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(nums, ["1", "31", "SCHEDULE 1, 2"]);
        let first = &articles[0];
        assert_eq!(first.title_path.as_deref(), Some("PART I - CERTIFICATION"));
        assert_eq!(
            first.texte,
            "Definitions\n(1) In this Act,\n\u{201c}court\u{201d} means the Superior Court of Justice;"
        );
        assert_eq!(
            first.nota.as_deref(),
            Some("1992, c. 6, s. 1; 2020, c. 11, Sched. 4, s. 1.")
        );
        let costs = &articles[1];
        assert_eq!(
            costs.title_path.as_deref(),
            Some("PART I - CERTIFICATION > Costs")
        );
        assert_eq!(
            costs.texte,
            "Costs\n(1) In exercising its discretion,\n(2) Class members are not liable for costs."
        );
        assert_eq!(
            costs.nota.as_deref(),
            Some("Section 31 is amended on a day to be named.")
        );
        assert_eq!(articles[2].texte, "The Minister may act.");
        assert_eq!(
            elaws_title(PAGE).as_deref(),
            Some("Class Proceedings Act, 1992 (Ontario), S.O. 1992, c. 6")
        );
    }
}
