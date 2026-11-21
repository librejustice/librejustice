//! A version of a cantonal act as LexFind serves it, the canton's own PDF, read from the
//! `pdftohtml -xml` rendering of that PDF by the place and the size of each run of text: the
//! body's size, the column it fills, the margin notes beside it, the note calls and the notes
//! below it, each canton laying its acts out in its own way.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;

/// How a canton lays its acts out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexfindLayout {
    /// Geneva: `Art. 1 Principe` in bold, the heading on the article's line.
    Ge,
    /// Zurich: `§ 1.` in roman, the heading in the right margin.
    Zh,
    /// Ticino: the heading in bold on the lines above `Art. 1`.
    Ti,
    /// Neuchâtel: the heading in the left margin.
    Ne,
    /// Schwyz: `§ 1` in bold, the heading on the article's line.
    Sz,
    /// Jura: the heading in the left margin.
    Ju,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Heading {
    OnLine,
    Above,
    Margin,
}

#[derive(Clone, Copy)]
enum Lang {
    Fr,
    De,
    It,
}

impl LexfindLayout {
    fn heading(self) -> Heading {
        match self {
            Self::Ge | Self::Sz => Heading::OnLine,
            Self::Ti => Heading::Above,
            Self::Zh | Self::Ne | Self::Ju => Heading::Margin,
        }
    }

    fn lang(self) -> Lang {
        match self {
            Self::Ge | Self::Ne | Self::Ju => Lang::Fr,
            Self::Zh | Self::Sz => Lang::De,
            Self::Ti => Lang::It,
        }
    }

    fn marker(self) -> &'static Regex {
        match (self, self.lang()) {
            (Self::Zh, _) => &ZH_MARKER,
            (_, Lang::Fr) => &FR_MARKER,
            (_, Lang::De) => &DE_MARKER,
            (_, Lang::It) => &IT_MARKER,
        }
    }

    fn division(self) -> &'static Regex {
        match self.lang() {
            Lang::Fr => &FR_DIVISION,
            Lang::De => &DE_DIVISION,
            Lang::It => &IT_DIVISION,
        }
    }

    /// The running lines of its pages a repetition does not tell.
    fn running(self, line: &str) -> bool {
        match self {
            Self::Ge => GE_RUNNING.is_match(line),
            Self::Zh => ZH_RUNNING.is_match(line),
            _ => false,
        }
    }

    /// The line after its last article: Geneva's table of the act's history, Ticino's reference
    /// of its publication or its transitional notes.
    fn ends(self, line: &str) -> bool {
        match self {
            Self::Ge => GE_HISTORY.is_match(line),
            Self::Ti => TI_END.is_match(line),
            _ => false,
        }
    }
}

const SUFFIX: &str =
    r"(?:\s?(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies)\b)?";

static FR_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:Art\.|Article)\s*(premier|1er|\d+[a-zA-Z]?{SUFFIX})\.?"
    ))
    .unwrap()
});
static IT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^(?:Art\.|Articolo)\s*(\d+[a-z]?{SUFFIX})\.?")).unwrap()
});
static DE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:§|Art\.)\s*(\d+[a-z]?{SUFFIX})\.?")).unwrap());
/// Zurich numbers its articles `§ 1.`, a treaty's `Art. 1.`, a reference in the text `§ 9 Abs. 1`.
static ZH_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:§|Art\.)\s*(\d+[a-z]?{SUFFIX})\.")).unwrap());
static ANY_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:Art\.|Article|Articolo|§)\s*\d").unwrap());
static MARKER_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:Art\.?|Article|Articolo|§)\s*\S+$").unwrap());
static FR_DIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(LIVRE|Livre|PARTIE|Partie|TITRE|Titre|CHAPITRE|Chapitre|SECTION|Section|SOUS-SECTION|Sous-section)\b").unwrap()
});
static IT_DIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(LIBRO|Libro|PARTE|Parte|TITOLO|Titolo|CAPITOLO|Capitolo|SEZIONE|Sezione)\b")
        .unwrap()
});
static DE_DIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d+\.\s+)?(TEIL|Teil|TITEL|Titel|KAPITEL|Kapitel|ABSCHNITT|Abschnitt|UNTERABSCHNITT|Unterabschnitt)\b|^(?:[IVXL]+|[A-Z])\.\s+\S").unwrap()
});
static ANNEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:ANNEXE|Annexe|ANHANG|Anhang|ALLEGATO|Allegato)\b").unwrap());
static GE_RUNNING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^rsGE |^Source SILGENEVE PUBLIC").unwrap());
/// Zurich's edition stamp, `1. 10. 25 - 130`.
static ZH_RUNNING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\. ?\d+\. ?\d+ - \d+$").unwrap());
static GE_HISTORY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^RSG\s+Intitulé").unwrap());
static TI_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:Pubblicat[oa] nel BU\b|N\.B\. NORME TRANSITORIE)").unwrap());

static FONTSPEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<fontspec id="(\d+)" size="([\d.]+)""#).unwrap());
static PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<page number="\d+"[^>]*height="(\d+)" width="(\d+)">(.*?)</page>"#).unwrap()
});
static TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<text top="(-?\d+)" left="(-?\d+)" width="(\d+)" height="(\d+)" font="(\d+)">(.*?)</text>"#).unwrap()
});
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
/// A note call: `(12)` in Geneva, `12)` in Neuchâtel and Jura, a bare `12` elsewhere.
static NOTE_CALLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(?\d+[a-z]?\)").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[a-z]*$").unwrap());
static NOTE_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\)?$").unwrap());
/// A note call set in the body's own size, glued to the word it annotates.
static GLUED_NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\S)(?:\(\d{1,3}\))+").unwrap());
static LEADING_NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\(\d{1,3}\)\s*)+").unwrap());
/// A paragraph number set in the body's own size.
static PARA_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\d{1,2})(?:bis|ter|quater|[a-z])?\s+[A-ZÀ-ÖØ-Þ«"]"#).unwrap());
static LEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)").unwrap());
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[a-z]{1,2}\)|[a-z]\.|\d+\.|[\x{2013}\x{2014}-]\s|[ivx]+\))").unwrap()
});
static DIGITS_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\d.\s/-]+$").unwrap());
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

/// A run of text the PDF draws in one font.
struct Run {
    top: f64,
    left: f64,
    right: f64,
    height: f64,
    size: f64,
    bold: bool,
    text: String,
}

impl Run {
    fn center(&self) -> f64 {
        self.top + self.height / 2.0
    }
}

struct Page {
    width: f64,
    runs: Vec<Run>,
    /// The left and right edges of the body's column.
    lo: f64,
    hi: f64,
}

/// A line of the body.
struct Line {
    page: usize,
    top: f64,
    left: f64,
    right: f64,
    /// Every run of the body's size is bold.
    bold: bool,
    first_bold: bool,
    /// The line opens on a paragraph number.
    para: bool,
    /// Where the text after an article's number starts.
    after_marker: Option<f64>,
    text: String,
}

struct MarginBlock {
    page: usize,
    top: f64,
    bottom: f64,
    text: String,
}

fn pages(xml: &str) -> Vec<Page> {
    let sizes: HashMap<&str, f64> = FONTSPEC
        .captures_iter(xml)
        .filter_map(|c| Some((c.get(1)?.as_str(), c[2].parse().ok()?)))
        .collect();
    PAGE.captures_iter(xml)
        .map(|p| Page {
            width: p[2].parse().unwrap_or(0.0),
            runs: TEXT
                .captures_iter(&p[3])
                .filter_map(|t| {
                    let raw = &t[6];
                    let text =
                        html_escape::decode_html_entities(&TAG.replace_all(raw, "")).into_owned();
                    if text.trim().is_empty() {
                        return None;
                    }
                    let left: f64 = t[2].parse().ok()?;
                    Some(Run {
                        top: t[1].parse().ok()?,
                        left,
                        right: left + t[3].parse::<f64>().ok()?,
                        height: t[4].parse().ok()?,
                        size: *sizes.get(&t[5])?,
                        bold: raw.contains("<b>"),
                        text,
                    })
                })
                .collect(),
            lo: 0.0,
            hi: 0.0,
        })
        .collect()
}

/// The size that sets the most characters.
fn body_size(pages: &[Page]) -> Option<f64> {
    let mut chars: HashMap<u64, usize> = HashMap::new();
    for r in pages.iter().flat_map(|p| &p.runs) {
        *chars.entry(r.size.to_bits()).or_default() += r.text.trim().chars().count();
    }
    chars
        .into_iter()
        .max_by_key(|(size, n)| (*n, *size))
        .map(|(size, _)| f64::from_bits(size))
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

fn glue(a: &str, b: &str) -> String {
    let mut chars = a.chars().rev();
    if chars.next() == Some('-')
        && chars.next().is_some_and(char::is_lowercase)
        && b.chars().next().is_some_and(char::is_lowercase)
    {
        format!("{}{b}", &a[..a.len() - 1])
    } else {
        format!("{a} {b}")
    }
}

/// A line's text from its runs, left to right: a note call left out, a paragraph number kept
/// apart from its text, and whether the line opens on one.
fn join(runs: &[&Run], small: f64) -> (String, bool) {
    let all_bold = runs.iter().filter(|r| r.size >= small).all(|r| r.bold);
    let mut out = String::new();
    let mut para = false;
    let mut prev: Option<&Run> = None;
    for &r in runs {
        let is_small = r.size < small;
        let mut s = r.text.trim().to_string();
        let called = is_small && NOTE_CALLS.is_match(&s);
        if called {
            s = NOTE_CALLS.replace_all(&s, "").trim().to_string();
        }
        if is_small && s.chars().all(|c| c == '(' || c == ')') {
            prev = Some(r);
            continue;
        }
        if is_small && NUMBER.is_match(&s) {
            // A bare number touching the word before it calls a note.
            let touching = !called
                && !out.is_empty()
                && !r.text.starts_with(char::is_whitespace)
                && prev.is_some_and(|p| {
                    !p.text.ends_with(char::is_whitespace) && r.left - p.right <= 1.0
                });
            let before = out.trim();
            if !touching
                && !(r.bold && !all_bold)
                && (before.is_empty() || MARKER_ONLY.is_match(before))
            {
                if !before.is_empty() {
                    out.push(' ');
                }
                out.push_str(&s);
                out.push(' ');
                para = true;
            }
            prev = Some(r);
            continue;
        }
        if prev.is_some_and(|p| r.left - p.right > 1.5)
            && !out.ends_with(' ')
            && !r.text.starts_with(' ')
        {
            out.push(' ');
        }
        out.push_str(&r.text);
        prev = Some(r);
    }
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (GLUED_NOTE.replace_all(&text, "$1").into_owned(), para)
}

/// A page's left edge, the leftmost start of a fair share of its lines' characters, and the
/// characters of the lines opening there. A heading does not count, nor a line opened by a
/// number, a note's below the body or a paragraph's: a page of notes and headings would set its
/// edge left of the column.
fn left_edge(page: &Page, small: f64, tolerance: f64) -> Option<(i64, usize)> {
    let mut body: Vec<&Run> = page.runs.iter().filter(|r| r.size >= small).collect();
    body.sort_by(|a, b| a.center().total_cmp(&b.center()));
    let mut lines: Vec<(f64, Vec<&Run>)> = Vec::new();
    for r in body {
        match lines.last_mut() {
            Some((center, runs)) if (r.center() - *center).abs() <= tolerance => runs.push(r),
            _ => lines.push((r.center(), vec![r])),
        }
    }
    let numbered = |r: &Run| {
        page.runs.iter().any(|x| {
            x.size < small
                && NOTE_NUMBER.is_match(x.text.trim())
                && (0.0..=8.0).contains(&(r.left - x.right))
                && (x.center() - r.center()).abs() <= 0.45 * r.height
        })
    };
    let mut lefts: HashMap<i64, usize> = HashMap::new();
    for (_, runs) in &lines {
        let Some(first) = runs.iter().min_by(|a, b| a.left.total_cmp(&b.left)) else {
            continue;
        };
        if first.bold || numbered(first) {
            continue;
        }
        *lefts.entry(first.left.round() as i64).or_default() += runs
            .iter()
            .map(|r| r.text.trim().chars().count())
            .sum::<usize>();
    }
    let near = |edge: i64| -> usize {
        lefts
            .iter()
            .filter(|(l, _)| (**l - edge).abs() <= 3)
            .map(|(_, n)| *n)
            .sum()
    };
    let most = lefts.keys().map(|l| near(*l)).max()? as f64;
    let edge = lefts
        .keys()
        .copied()
        .filter(|l| near(*l) as f64 >= 0.15 * most)
        .min()?;
    Some((edge, lefts.values().sum()))
}

/// The body's lines of every page and the margin notes beside them, the notes below the body and
/// the running heads and feet left out.
fn lines(pages: &mut [Page], size: f64, line_height: f64) -> (Vec<Line>, Vec<MarginBlock>) {
    let small = 0.9 * size;
    let tolerance = 0.45 * line_height;
    // A document sets its column once for its odd pages and once for its even ones, at the edge
    // its pages open the most characters on.
    let mut edges: [HashMap<i64, usize>; 2] = Default::default();
    for (index, page) in pages.iter().enumerate() {
        if let Some((edge, chars)) = left_edge(page, small, tolerance) {
            *edges[index % 2].entry(edge).or_default() += chars;
        }
    }
    let column = edges.map(|e| {
        e.into_iter()
            .max_by_key(|(edge, chars)| (*chars, -*edge))
            .map(|(edge, _)| edge as f64)
    });
    let mut margins: Vec<(usize, f64, f64, String)> = Vec::new();
    let mut by_page: Vec<Vec<Line>> = Vec::with_capacity(pages.len());
    for (index, page) in pages.iter_mut().enumerate() {
        let body: Vec<&Run> = page.runs.iter().filter(|r| r.size >= small).collect();
        if body.is_empty() {
            by_page.push(Vec::new());
            continue;
        }
        let lo = column[index % 2]
            .or(column[1 - index % 2])
            .unwrap_or_else(|| body.iter().map(|r| r.left).fold(f64::INFINITY, f64::min));
        // The few lines overrunning the column would swallow a margin note set just right of it.
        let mut rights: Vec<f64> = body
            .iter()
            .filter(|r| r.left >= lo - 3.0)
            .map(|r| r.right)
            .collect();
        rights.sort_by(f64::total_cmp);
        let hi = match rights.len() {
            0 => body.iter().map(|r| r.right).fold(lo, f64::max),
            n => rights[(0.95 * (n - 1) as f64) as usize],
        };
        page.lo = lo;
        page.hi = hi;
        // A note below the body opens on its number, left of the column, beside its text.
        let cut = page
            .runs
            .iter()
            .filter(|x| {
                x.size < small
                    && NOTE_NUMBER.is_match(x.text.trim())
                    && x.right <= lo - 10.0
                    && body.iter().any(|b| {
                        (b.center() - x.center()).abs() <= 0.45 * b.height
                            && (0.0..=15.0).contains(&(b.left - x.right))
                    })
            })
            .map(|x| x.top - 2.0)
            .fold(f64::INFINITY, f64::min);
        let mut rest: Vec<&Run> = Vec::new();
        for r in page.runs.iter().filter(|r| r.top < cut) {
            if r.size < small && (r.right <= lo - 3.0 || r.left >= hi + 3.0) {
                if !NOTE_NUMBER.is_match(r.text.trim()) {
                    margins.push((index, r.top, r.top + r.height, r.text.trim().to_string()));
                }
            } else {
                rest.push(r);
            }
        }
        let mut groups: Vec<(f64, bool, Vec<&Run>)> = Vec::new();
        let mut big: Vec<&Run> = rest.iter().copied().filter(|r| r.size >= small).collect();
        big.sort_by(|a, b| a.center().total_cmp(&b.center()));
        for r in big {
            match groups.last_mut() {
                Some((center, _, runs)) if (r.center() - *center).abs() <= tolerance => {
                    runs.push(r)
                }
                _ => groups.push((r.center(), false, vec![r])),
            }
        }
        for r in rest.iter().copied().filter(|r| r.size < small) {
            let nearest = groups
                .iter_mut()
                .filter(|(_, is_small, _)| !is_small)
                .min_by(|a, b| {
                    (a.0 - r.center())
                        .abs()
                        .total_cmp(&(b.0 - r.center()).abs())
                });
            match nearest {
                Some((center, _, runs)) if (*center - r.center()).abs() <= tolerance => {
                    runs.push(r)
                }
                _ => groups.push((r.center(), true, vec![r])),
            }
        }
        groups.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut page_lines = Vec::new();
        for (_, is_small, mut runs) in groups {
            if is_small {
                continue;
            }
            runs.sort_by(|a, b| a.left.total_cmp(&b.left));
            let (text, para) = join(&runs, small);
            if text.is_empty() {
                continue;
            }
            let after_marker = runs
                .iter()
                .find(|r| r.size >= small && r.left > runs[0].right + 2.0)
                .map(|r| r.left);
            page_lines.push(Line {
                page: index,
                top: runs.iter().map(|r| r.top).fold(f64::INFINITY, f64::min),
                left: runs[0].left,
                right: runs.iter().map(|r| r.right).fold(0.0, f64::max),
                bold: runs.iter().filter(|r| r.size >= small).all(|r| r.bold),
                first_bold: runs[0].bold,
                para,
                after_marker,
                text,
            });
        }
        by_page.push(page_lines);
    }
    // A running head swaps its title and its number from one page to the next.
    let normal = |t: &str| {
        let t = DIGITS.replace_all(&t.to_lowercase(), "#").into_owned();
        let mut words: Vec<&str> = t.split_whitespace().collect();
        words.sort_unstable();
        words.join(" ")
    };
    let mut repeated: HashMap<String, usize> = HashMap::new();
    for ls in &by_page {
        let n = ls.len();
        for l in ls
            .iter()
            .take(3)
            .chain(ls.iter().skip(n.saturating_sub(3).max(3)))
        {
            *repeated.entry(normal(&l.text)).or_default() += 1;
        }
    }
    let several = pages.len() > 1;
    let mut kept = Vec::new();
    for ls in by_page {
        let n = ls.len();
        for (k, l) in ls.into_iter().enumerate() {
            let edge = k < 3 || k + 3 >= n;
            let outer = k < 2 || k + 2 >= n;
            if edge
                && (DIGITS_ONLY.is_match(&l.text)
                    || (several && outer && repeated.get(&normal(&l.text)) > Some(&1)))
            {
                continue;
            }
            kept.push(l);
        }
    }
    margins.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap());
    let mut blocks: Vec<MarginBlock> = Vec::new();
    for (page, top, bottom, text) in margins {
        match blocks.last_mut() {
            Some(b) if b.page == page && top - b.bottom <= 0.9 * line_height => {
                b.bottom = bottom;
                b.text = glue(&b.text, &text);
            }
            _ => blocks.push(MarginBlock {
                page,
                top,
                bottom,
                text,
            }),
        }
    }
    (kept, blocks)
}

fn centered(l: &Line, page: &Page) -> bool {
    ((l.left + l.right) / 2.0 - (page.lo + page.hi) / 2.0).abs() < 0.08 * page.width
}

/// The margin note beside an article's first line, or the one just above it with no other
/// article between.
fn margin_heading(
    blocks: &[MarginBlock],
    lines: &[Line],
    at: usize,
    line_height: f64,
) -> Option<String> {
    let line = &lines[at];
    let page = blocks.iter().filter(|b| b.page == line.page);
    if let Some(b) = page
        .clone()
        .find(|b| b.top <= line.top + line_height && b.bottom >= line.top - 1.0)
    {
        return Some(b.text.clone());
    }
    let b = page
        .filter(|b| b.bottom <= line.top + 2.0)
        .max_by(|a, b| a.top.total_cmp(&b.top))?;
    let between = lines[..at]
        .iter()
        .filter(|s| s.page == line.page && s.top >= b.top - 2.0 && s.top < line.top)
        .any(|s| ANY_MARKER.is_match(&s.text));
    (!between && line.top - b.bottom < 7.0 * line_height).then(|| b.text.clone())
}

fn rank(keyword: &str) -> usize {
    match keyword.to_lowercase().as_str() {
        "livre" | "libro" | "partie" | "parte" | "teil" => 0,
        "titre" | "titolo" | "titel" => 1,
        "section" | "sezione" | "abschnitt" => 3,
        "sous-section" | "unterabschnitt" => 4,
        _ => 2,
    }
}

struct Open {
    num: String,
    heading: Option<String>,
    paragraphs: Vec<String>,
    path: Option<String>,
    last: usize,
    after_marker: Option<f64>,
    paragraph: u32,
}

/// The articles of one version, in document order, each under the divisions above it, its
/// heading first, then one line per paragraph and per point. An article with no text has left
/// the act; the text after an annex, or after an article numbered twice, is not the act's own.
pub fn lexfind_articles(
    xml: &str,
    layout: LexfindLayout,
) -> Result<Vec<ConsolidatedArticle>, String> {
    let mut pages = pages(xml);
    if pages.is_empty() {
        return Err("the rendering holds no page".to_string());
    }
    let Some(size) = body_size(&pages) else {
        return Ok(Vec::new());
    };
    let line_height = median(
        pages
            .iter()
            .flat_map(|p| &p.runs)
            .filter(|r| r.size == size)
            .map(|r| r.height)
            .collect(),
    );
    let (all, blocks) = lines(&mut pages, size, line_height);
    let seq: Vec<Line> = all
        .into_iter()
        .filter(|l| !layout.running(&l.text))
        .collect();
    let marker = layout.marker();
    let division = layout.division();
    let mut out = Vec::new();
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut open: Option<Open> = None;
    let mut seen: HashSet<String> = HashSet::new();
    let mut above: Vec<String> = Vec::new();
    let mut above_last: Option<usize> = None;
    let close = |open: &mut Option<Open>, out: &mut Vec<ConsolidatedArticle>| {
        if let Some(a) = open.take().filter(|a| !a.paragraphs.is_empty()) {
            let texte = a
                .heading
                .into_iter()
                .chain(a.paragraphs)
                .collect::<Vec<_>>();
            out.push(ConsolidatedArticle {
                num: a.num,
                title_path: a.path,
                texte: texte.join("\n"),
                nota: None,
            });
        }
    };
    let mut i = 0;
    while i < seq.len() {
        let l = &seq[i];
        let t = l.text.as_str();
        let page = &pages[l.page];
        let set_apart = l.bold
            || t.split_whitespace()
                .next()
                .is_some_and(|w| w.chars().any(char::is_alphabetic) && w == w.to_uppercase())
            || centered(l, page);
        if layout.ends(t) || (ANNEX.is_match(t) && set_apart) {
            break;
        }
        let at_marker = marker.captures(t).filter(|m| {
            let end = m.get(0).unwrap().end();
            t[end..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '(')
        });
        if let Some(m) = at_marker.filter(|_| l.first_bold || layout == LexfindLayout::Zh) {
            let num = match m[1].replace(' ', "").as_str() {
                "premier" | "1er" => "1".to_string(),
                n => n.to_string(),
            };
            if !seen.insert(num.clone()) {
                break;
            }
            close(&mut open, &mut out);
            let rest = t[m.get(0).unwrap().end()..].trim();
            let (heading, paragraphs) = match layout.heading() {
                Heading::OnLine => (
                    Some(LEADING_NOTE.replace(rest, "").into_owned()).filter(|h| !h.is_empty()),
                    Vec::new(),
                ),
                Heading::Above => (
                    Some(above.join(" ")).filter(|h| !h.is_empty()),
                    Vec::from_iter((!rest.is_empty()).then(|| rest.to_string())),
                ),
                Heading::Margin => (
                    margin_heading(&blocks, &seq, i, line_height),
                    Vec::from_iter((!rest.is_empty()).then(|| rest.to_string())),
                ),
            };
            let paragraph = paragraphs
                .first()
                .filter(|_| l.para)
                .and_then(|p| LEADING_NUMBER.captures(p))
                .and_then(|n| n[1].parse().ok())
                .unwrap_or(0);
            open = Some(Open {
                num,
                heading,
                paragraphs,
                path: (!path.is_empty()).then(|| {
                    path.iter()
                        .map(|(_, label)| label.as_str())
                        .collect::<Vec<_>>()
                        .join(" > ")
                }),
                last: i,
                after_marker: l.after_marker,
                paragraph,
            });
            above.clear();
            i += 1;
            continue;
        }
        if let Some(d) = division.captures(t).filter(|_| set_apart && t.len() < 150) {
            close(&mut open, &mut out);
            let rank = rank(d.get(1).map_or("", |k| k.as_str()));
            let mut label = t.to_string();
            i += 1;
            if t.split_whitespace().count() <= 3 {
                if let Some(next) = seq.get(i).filter(|n| {
                    n.bold
                        && !marker.is_match(&n.text)
                        && !division.is_match(&n.text)
                        && (layout.heading() != Heading::Above || centered(n, &pages[n.page]))
                }) {
                    label = format!("{label} {}", next.text);
                    i += 1;
                }
            }
            path.retain(|(r, _)| *r < rank);
            path.push((rank, label.replace('>', "›")));
            above.clear();
            continue;
        }
        // A bold line inside an article heads its next paragraphs, not the next article.
        let heads_article = || {
            seq[i + 1..]
                .iter()
                .find(|n| !n.bold || marker.is_match(&n.text))
                .is_none_or(|n| marker.is_match(&n.text) || division.is_match(&n.text))
        };
        if layout.heading() == Heading::Above
            && l.bold
            && !l.para
            && (open.is_none() || heads_article())
        {
            if above.is_empty() {
                close(&mut open, &mut out);
            }
            let far = above_last
                .is_some_and(|a| seq[a].page != l.page || l.top - seq[a].top > 1.6 * line_height);
            if centered(l, page) || far {
                above.clear();
            }
            if !centered(l, page) {
                above.push(t.to_string());
                above_last = Some(i);
            }
            i += 1;
            continue;
        }
        if let Some(a) = open.as_mut() {
            let continues_heading = layout.heading() == Heading::OnLine
                && a.heading.is_some()
                && a.paragraphs.is_empty()
                && !l.para
                && (a.after_marker.is_some_and(|x| (l.left - x).abs() <= 3.0)
                    || (l.bold && seq[a.last].bold));
            if continues_heading {
                a.heading = a.heading.as_deref().map(|h| glue(h, t));
                a.last = i;
                i += 1;
                continue;
            }
            let last = &seq[a.last];
            let gap = if last.page == l.page {
                l.top - last.top
            } else {
                0.0
            };
            let numbered = PARA_NUMBER
                .captures(t)
                .filter(|_| l.left <= page.lo + 3.0)
                .and_then(|n| n[1].parse::<u32>().ok())
                .filter(|n| *n == a.paragraph + 1);
            if l.para || numbered.is_some() {
                if let Some(n) = LEADING_NUMBER.captures(t).and_then(|n| n[1].parse().ok()) {
                    a.paragraph = n;
                }
            }
            let starts = a.paragraphs.is_empty()
                || l.para
                || numbered.is_some()
                || LIST_ITEM.is_match(t)
                || gap > 1.9 * line_height;
            match a.paragraphs.last_mut() {
                Some(p) if !starts => *p = glue(p, t),
                _ => a.paragraphs.push(t.to_string()),
            }
            a.last = i;
        }
        i += 1;
    }
    close(&mut open, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(runs: &[(i32, i32, i32, i32, u8, &str)]) -> String {
        let texts: String = runs
            .iter()
            .map(|(top, left, width, height, font, text)| {
                format!(r#"<text top="{top}" left="{left}" width="{width}" height="{height}" font="{font}">{text}</text>"#)
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"<page number="1" position="absolute" top="0" left="0" height="1262" width="892">
<fontspec id="0" size="15" family="ArialMT"/>
<fontspec id="1" size="10" family="ArialMT"/>
<fontspec id="2" size="12" family="ArialMT"/>
{texts}
</page>"#
        )
    }

    #[test]
    fn a_geneva_article_reads_its_heading_paragraphs_and_points_without_note_calls() {
        let xml = page(&[
            (54, 81, 500, 17, 0, "rsGE I 2 30: Loi sur les ventes"),
            (
                465,
                81,
                335,
                20,
                0,
                "<b>Chapitre I          Dispositions générales </b>",
            ),
            (503, 81, 135, 17, 0, "<b>Art. 1        Principe </b>"),
            (523, 81, 5, 11, 1, "1"),
            (
                523,
                90,
                725,
                17,
                0,
                "Toute vente volontaire doit être faite par un huis-",
            ),
            (540, 81, 300, 17, 0, "sier judiciaire."),
            (561, 81, 5, 11, 1, "2"),
            (
                560,
                90,
                241,
                17,
                0,
                "L’huissier peut se faire assister par le département",
            ),
            (581, 331, 12, 8, 1, "(10)"),
            (578, 343, 172, 17, 0, " (ci-après : département)."),
            (598, 91, 22, 17, 0, "a)  "),
            (598, 115, 400, 17, 0, "d’une publicité suffisante;"),
            (612, 81, 148, 17, 0, "<b>Art. 2        Exception </b>"),
            (
                632,
                81,
                400,
                17,
                0,
                "Les corporations peuvent être autorisées.",
            ),
            (650, 81, 120, 17, 0, "<b>Art. 3        Abrogé </b>"),
            (
                700,
                93,
                150,
                17,
                0,
                "<b>RSG                     Intitulé </b>",
            ),
            (720, 93, 150, 17, 0, "I 2 30 L sur les ventes"),
            (1193, 600, 217, 17, 0, "Source SILGENEVE PUBLIC, 1"),
        ]);
        let arts = lexfind_articles(&xml, LexfindLayout::Ge).unwrap();
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].num, "1");
        assert_eq!(
            arts[0].title_path.as_deref(),
            Some("Chapitre I Dispositions générales")
        );
        assert_eq!(
            arts[0].texte,
            "Principe\n1 Toute vente volontaire doit être faite par un huissier judiciaire.\n2 L’huissier peut se faire assister par le département (ci-après : département).\na) d’une publicité suffisante;"
        );
        assert_eq!(
            arts[1].texte,
            "Exception\nLes corporations peuvent être autorisées."
        );
    }

    #[test]
    fn a_margin_note_heads_the_article_beside_it_and_a_footnote_is_left_out() {
        let xml = page(&[
            (581, 230, 183, 20, 0, "CHAPITRE PREMIER "),
            (606, 230, 202, 20, 0, "<b>Dispositions générales </b>"),
            (644, 360, 6, 11, 1, "1"),
            (645, 109, 87, 16, 2, "Dénomination "),
            (645, 230, 117, 20, 0, "<b>Article premier</b>"),
            (645, 366, 447, 20, 0, "Il est créé une entité de formation."),
            (660, 109, 88, 16, 2, "statut et siège "),
            (710, 230, 6, 11, 1, "2"),
            (
                711,
                235,
                573,
                20,
                0,
                "Le CNIP est un établissement de droit public.",
            ),
            (786, 109, 56, 16, 2, "Missions "),
            (786, 230, 42, 20, 0, "<b>Art. 2</b>"),
            (785, 272, 9, 11, 1, "3)"),
            (786, 292, 302, 20, 0, "Le CNIP a pour missions de former."),
            (1079, 230, 8, 10, 1, "1)"),
            (1080, 238, 84, 16, 2, "  RS 412.10  "),
        ]);
        let arts = lexfind_articles(&xml, LexfindLayout::Ne).unwrap();
        assert_eq!(arts.len(), 2);
        assert_eq!(
            arts[0].title_path.as_deref(),
            Some("CHAPITRE PREMIER Dispositions générales")
        );
        assert_eq!(
            arts[0].texte,
            "Dénomination statut et siège\n1 Il est créé une entité de formation.\n2 Le CNIP est un établissement de droit public."
        );
        assert_eq!(arts[1].num, "2");
        assert_eq!(
            arts[1].texte,
            "Missions\nLe CNIP a pour missions de former."
        );
    }

    #[test]
    fn a_ticino_heading_above_its_article_and_a_note_below_the_body_are_told_apart() {
        let xml = page(&[
            (377, 418, 97, 17, 0, "Capitolo primo"),
            (395, 391, 150, 17, 0, "<b>Disposizioni generali</b>"),
            (429, 99, 46, 17, 0, "<b>Scopo</b>"),
            (446, 139, 38, 17, 0, "<b>Art. 1</b>"),
            (446, 213, 7, 13, 2, "1"),
            (
                446,
                219,
                579,
                17,
                0,
                "È costituito l’Istituto di previdenza del Cantone Ticino,",
            ),
            (
                464,
                139,
                659,
                17,
                0,
                "ente di diritto pubblico autonomo con personalità giuridica",
            ),
            (482, 139, 300, 17, 0, "e sede a Bellinzona."),
            (500, 139, 7, 13, 2, "2"),
            (
                500,
                146,
                652,
                17,
                0,
                "L’Istituto è iscritto nel registro di commercio e",
            ),
            (
                518,
                139,
                659,
                17,
                0,
                "pubblica i suoi conti ogni anno nel Foglio ufficiale del",
            ),
            (536, 139, 200, 17, 0, "Cantone."),
            (1110, 96, 5, 11, 1, "1"),
            (1109, 101, 300, 17, 0, "  Vedi L 20.10.2020."),
        ]);
        let arts = lexfind_articles(&xml, LexfindLayout::Ti).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(
            arts[0].title_path.as_deref(),
            Some("Capitolo primo Disposizioni generali")
        );
        assert_eq!(
            arts[0].texte,
            "Scopo\n1 È costituito l’Istituto di previdenza del Cantone Ticino, ente di diritto pubblico autonomo con personalità giuridica e sede a Bellinzona.\n2 L’Istituto è iscritto nel registro di commercio e pubblica i suoi conti ogni anno nel Foglio ufficiale del Cantone."
        );
    }

    #[test]
    fn an_annex_ends_the_articles() {
        let xml = page(&[
            (100, 81, 135, 17, 0, "<b>Art. 1        Objet </b>"),
            (120, 81, 400, 17, 0, "Le présent règlement fixe les tarifs."),
            (160, 81, 200, 20, 0, "<b>ANNEXE (art. 1)</b>"),
            (180, 81, 400, 17, 0, "Tarif des émoluments."),
        ]);
        let arts = lexfind_articles(&xml, LexfindLayout::Ge).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(
            arts[0].texte,
            "Objet\nLe présent règlement fixe les tarifs."
        );
    }
}
