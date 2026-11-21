//! A coordinated text a word processor laid out, read from its `pdftohtml -xml` runs on its bold article heads.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::ConsolidatedArticle;

static PAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<page number="(\d+)"[^>]*height="(\d+)""#).unwrap());
static FONT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<fontspec id="(\d+)" size="(\d+)""#).unwrap());
static RUN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<text top="(-?\d+)" left="(-?\d+)" width="(-?\d+)" height="(-?\d+)" font="(\d+)">(.*?)</text>"#)
        .unwrap()
});
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<a [^>]*>.*?</a>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static FOLIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[IVXL0-9.]*\d ?/ ?\d+$").unwrap());

/// What a source's texts are written in.
pub(crate) struct Words {
    /// An article's head: its opening bracket, then its number.
    pub(crate) head: &'static LazyLock<Regex>,
    /// The head of a text's sole article, its opening bracket first.
    pub(crate) sole: &'static LazyLock<Regex>,
    /// What follows the last article: the promulgation, the place and day of signature.
    pub(crate) closing: &'static LazyLock<Regex>,
    /// Whether what follows the closing is an annex, never the text's own articles quoting a formula.
    pub(crate) annexed: bool,
    /// The rank of a heading by its first word, deepest last.
    pub(crate) level: fn(&str) -> Option<u8>,
    /// A run of the running header, and one of the footer: what sits on or above the first, on or below the second, is furniture.
    pub(crate) furniture: Option<(&'static LazyLock<Regex>, &'static LazyLock<Regex>)>,
    /// How the note on the text's publications opens, which drops what was read before it.
    pub(crate) restart: Option<&'static str>,
}

struct Run {
    page: u32,
    top: i32,
    left: i32,
    width: i32,
    size: u32,
    bold: bool,
    italic: bool,
    /// Nothing but a link: a note call or a contents entry.
    link: bool,
    text: String,
}

/// The runs of the text proper and the body's size: no running header or footer, note call, note or contents.
fn runs(xml: &str, words: &Words) -> (Vec<Run>, u32) {
    let sizes: HashMap<&str, u32> = FONT
        .captures_iter(xml)
        .filter_map(|c| Some((c.get(1)?.as_str(), c[2].parse().ok()?)))
        .collect();
    let pages: Vec<(usize, u32, i32)> = PAGE
        .captures_iter(xml)
        .filter_map(|c| Some((c.get(0)?.start(), c[1].parse().ok()?, c[2].parse().ok()?)))
        .collect();
    let placed: Vec<(Run, f64)> = RUN
        .captures_iter(xml)
        .map(|c| {
            let at = c.get(0).unwrap().start();
            let inner = &c[6];
            let (page, height) = pages
                .iter()
                .rev()
                .find(|(start, ..)| *start < at)
                .map_or((0, 0), |p| (p.1, p.2));
            let top: i32 = c[1].parse().unwrap_or(0);
            let run = Run {
                page,
                top,
                left: c[2].parse().unwrap_or(0),
                width: c[3].parse().unwrap_or(0),
                size: sizes.get(&c[5]).copied().unwrap_or(0),
                bold: inner.contains("<b>"),
                italic: inner.contains("<i>"),
                link: inner.contains("<a ")
                    && TAG
                        .replace_all(&LINK.replace_all(inner, ""), "")
                        .trim()
                        .is_empty(),
                text: html_escape::decode_html_entities(&TAG.replace_all(inner, "")).into_owned(),
            };
            (run, f64::from(top) / f64::from(height.max(1)))
        })
        .collect();
    let mut furniture: HashMap<u32, (i32, i32)> = HashMap::new();
    if let Some((header, footer)) = words.furniture {
        for (r, share) in &placed {
            let cut = furniture.entry(r.page).or_insert((i32::MIN, i32::MAX));
            if *share < 0.12 && header.is_match(r.text.trim()) {
                cut.0 = cut.0.max(r.top + 3);
            }
            if *share > 0.86 && footer.is_match(r.text.trim()) {
                cut.1 = cut.1.min(r.top - 6);
            }
        }
    }
    let all: Vec<Run> = placed
        .into_iter()
        .map(|(r, _)| r)
        .filter(|r| {
            furniture
                .get(&r.page)
                .is_none_or(|&(header, footer)| r.top > header && r.top < footer)
        })
        .collect();
    let mut weight: HashMap<u32, usize> = HashMap::new();
    for r in &all {
        *weight.entry(r.size).or_default() += r.text.trim().len();
    }
    let body = weight
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map_or(0, |(s, _)| s);
    let mut rule: HashMap<u32, i32> = HashMap::new();
    for r in &all {
        if r.text.trim().is_empty() && r.width >= 150 && r.size != body {
            rule.entry(r.page).or_insert(r.top);
        }
    }
    let mut margin: HashMap<u32, i32> = HashMap::new();
    for r in all
        .iter()
        .filter(|r| r.size == body && !r.text.trim().is_empty())
    {
        let left = margin.entry(r.page).or_insert(r.left);
        *left = (*left).min(r.left);
    }
    for r in &all {
        let number = !r.text.trim().is_empty() && r.text.trim().chars().all(|c| c.is_ascii_digit());
        if number && r.size < body && margin.get(&r.page).is_some_and(|&m| r.left <= m + 2) {
            let cut = rule.entry(r.page).or_insert(r.top);
            *cut = (*cut).min(r.top);
        }
    }
    let kept = all
        .into_iter()
        .filter(|r| r.size >= body || (r.bold && r.text.chars().any(char::is_alphabetic)))
        .filter(|r| !r.link && rule.get(&r.page).is_none_or(|&cut| r.top < cut))
        .collect();
    (kept, body)
}

/// A line of the page: whether it opens in bold, where its text starts, its text.
struct Line {
    bold: bool,
    italic: bool,
    left: i32,
    text: String,
}

/// The lines of the text but the folios, a smaller run a few points off a line being its superscript.
fn lines(runs: Vec<Run>, body: u32) -> Vec<Line> {
    let mut out: Vec<(u32, i32, Line)> = Vec::new();
    for r in runs {
        match out.last_mut() {
            Some((page, top, line))
                if *page == r.page
                    && ((*top - r.top).abs() <= 3
                        || (r.size < body && (*top - r.top).abs() <= 8)) =>
            {
                if line.text.trim().is_empty() {
                    line.left = r.left;
                }
                if line.text.trim().trim_start_matches('[').is_empty() {
                    line.bold = r.bold;
                }
                if !r.text.trim().is_empty() {
                    line.italic &= r.italic;
                }
                line.text.push_str(&r.text);
            }
            _ => out.push((
                r.page,
                r.top,
                Line {
                    bold: r.bold,
                    italic: r.italic || r.text.trim().is_empty(),
                    left: r.left,
                    text: r.text,
                },
            )),
        }
    }
    out.into_iter()
        .map(|(_, _, line)| Line {
            text: line.text.split_whitespace().collect::<Vec<_>>().join(" "),
            ..line
        })
        .filter(|line| !line.text.is_empty() && !FOLIO.is_match(&line.text))
        .collect()
}

/// A text's articles on its bold heads, or on its plain heads followed by a separator when it has none.
pub(crate) fn pdf_articles(xml: &str, words: &Words) -> Result<Vec<ConsolidatedArticle>, String> {
    let (runs, body) = runs(xml, words);
    let lines = lines(runs, body);
    let articles = match articles(&lines, false, words) {
        bold if bold.is_empty() => articles(&lines, true, words),
        bold => bold,
    };
    if articles.is_empty() {
        return Err("no article".to_string());
    }
    Ok(articles)
}

/// An article's head at the start of `line`: its number, bracket, text start, and whether a separator follows.
fn head(line: &str, words: &Words) -> Option<(String, String, usize, bool)> {
    if let Some(c) = words.head.captures(line) {
        let rest = line[c.get(2).unwrap().end()..].trim_start();
        let after = rest.trim_start_matches(['.', '-', '\u{2013}', ':']);
        let num = match c[2].replace(' ', "") {
            num if num.eq_ignore_ascii_case("premier") => "1er".to_string(),
            num => num,
        };
        return Some((
            num,
            c[1].to_string(),
            c.get(0).unwrap().end(),
            rest.is_empty()
                || (after.len() < rest.len() && !after.starts_with(|c: char| c.is_ascii_digit())),
        ));
    }
    words.sole.captures(line).map(|c| {
        (
            "unique".to_string(),
            c[1].to_string(),
            c.get(0).unwrap().end(),
            true,
        )
    })
}

/// An article's number as an integer, `1er` being 1.
fn rank(num: &str) -> Option<u32> {
    num.strip_suffix("er").unwrap_or(num).parse().ok()
}

fn same(a: &str, b: &str) -> bool {
    a == b || rank(a).is_some_and(|r| rank(b) == Some(r))
}

/// An article shown twice keeps its first showing, and a line all in italics is a caption or a note.
fn articles(lines: &[Line], plain: bool, words: &Words) -> Vec<ConsolidatedArticle> {
    let mut headings: Vec<(u8, String)> = Vec::new();
    let mut articles: Vec<ConsolidatedArticle> = Vec::new();
    let mut previous: Option<(bool, i32, bool, bool)> = None;
    let mut shown_again = false;
    let mut opening = true;
    for &Line {
        bold,
        italic,
        left,
        text: ref line,
    } in lines
    {
        let found = head(line, words);
        if let Some((true, previous_left, in_heading, after_head)) = previous {
            let wraps = match in_heading {
                true => !italic && found.is_none() && (words.level)(line).is_none(),
                false => left < previous_left && (after_head || found.is_none()),
            };
            if bold && wraps {
                let open = match in_heading {
                    true => headings.last_mut().map(|(_, h)| h),
                    false => articles.last_mut().map(|a| &mut a.texte),
                };
                if let Some(open) = open {
                    if !open.is_empty() {
                        open.push(' ');
                    }
                    open.push_str(line);
                    continue;
                }
            }
        }
        if !bold
            && std::mem::take(&mut opening)
            && words.restart.is_some_and(|r| line.starts_with(r))
        {
            articles.clear();
            headings.clear();
            shown_again = false;
            previous = None;
            continue;
        }
        previous = Some((bold, left, false, false));
        if words.annexed && !articles.is_empty() && words.closing.is_match(line) {
            break;
        }
        let opened = found.filter(|(.., separated)| bold || (plain && *separated));
        if let Some((num, bracket, end, _)) = opened {
            // An annex numbers its own articles again: the text ends where it starts.
            if articles.first().is_some_and(|a| same(&a.num, &num)) {
                break;
            }
            shown_again = articles.iter().any(|a| a.num == num);
            if shown_again {
                continue;
            }
            let path = headings
                .iter()
                .map(|(_, h)| h.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
            articles.push(ConsolidatedArticle {
                num,
                title_path: (!path.is_empty()).then_some(path),
                texte: format!("{bracket}{}", &line[end..]),
                nota: None,
            });
            previous = Some((bold, left, false, true));
            continue;
        }
        if let Some(rank) = bold.then(|| (words.level)(line)).flatten() {
            headings.retain(|(r, _)| *r < rank);
            headings.push((rank, line.clone()));
            previous = Some((true, left, true, false));
            continue;
        }
        if italic {
            continue;
        }
        if let Some(article) = articles.last_mut().filter(|_| !shown_again) {
            if !article.texte.is_empty() {
                article.texte.push('\n');
            }
            article.texte.push_str(line);
        }
    }
    if let Some(last) = articles.last_mut() {
        if let Some(cut) = last.texte.lines().position(|l| words.closing.is_match(l)) {
            last.texte = last.texte.lines().take(cut).collect::<Vec<_>>().join("\n");
        }
    }
    articles
}
