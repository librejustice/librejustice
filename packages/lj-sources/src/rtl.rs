//! The text of a PDF whose lines run right to left, laid out from its glyphs. Poppler reverses
//! an Arabic line character by character, so a glyph mapped to several characters comes out
//! backwards: « ال » for the lam-alef « لا », « هللا » for « الله ».

use lj_pdf::objects::{parse_pdf, ObjectRef};
use lj_pdf::text::{analyze_page_text, Glyph, GlyphLocation};

use crate::error::{Result, SourceError};

/// One glyph drawn: its characters and its place on the page, `y` its middle.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    pub text: String,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub height: f64,
}

/// Every page's lines in reading order, a line right to left when its right-to-left letters
/// outnumber its left-to-right ones.
pub fn rtl_text(bytes: &[u8]) -> Result<String> {
    let parsed = parse_pdf(bytes).map_err(|e| SourceError::Invalid(format!("lj-pdf: {e}")))?;
    let mut pages = Vec::with_capacity(parsed.pages.len());
    for (i, page) in parsed.pages.iter().enumerate() {
        let text = analyze_page_text(&parsed.file, i, page)
            .map_err(|e| SourceError::Invalid(format!("lj-pdf page {i}: {e}")))?;
        pages.push(page_text(drawn(&text.glyphs)));
    }
    Ok(pages.join("\n"))
}

type GlyphKey = (usize, Option<ObjectRef>, usize, usize, usize);

fn key(g: &Glyph) -> GlyphKey {
    match g.location {
        GlyphLocation::Direct {
            operand_index,
            byte_start,
            ..
        } => (
            g.operation_index,
            g.source_form,
            operand_index,
            usize::MAX,
            byte_start,
        ),
        GlyphLocation::Array {
            operand_index,
            element_index,
            byte_start,
            ..
        } => (
            g.operation_index,
            g.source_form,
            operand_index,
            element_index,
            byte_start,
        ),
    }
}

/// The glyphs drawn, the characters one glyph maps to joined.
fn drawn(glyphs: &[Glyph]) -> Vec<Drawn> {
    let mut out: Vec<(GlyphKey, Drawn)> = Vec::new();
    for g in glyphs.iter().filter(|g| g.visible) {
        let k = key(g);
        match out.last_mut() {
            Some((last, d)) if *last == k => d.text.push(g.text),
            _ => {
                let r = g.bbox.normalize();
                out.push((
                    k,
                    Drawn {
                        text: g.text.to_string(),
                        x0: r.x,
                        x1: r.x + r.width,
                        y: r.y + r.height / 2.0,
                        height: r.height.max(1.0),
                    },
                ));
            }
        }
    }
    out.into_iter().map(|(_, d)| d).collect()
}

/// A page's glyphs as lines, from the top: a glyph within 0.45 of its height of a line's
/// middle sits on that line.
pub fn page_text(mut glyphs: Vec<Drawn>) -> String {
    glyphs.sort_by(|a, b| b.y.total_cmp(&a.y));
    let mut lines: Vec<(f64, Vec<Drawn>)> = Vec::new();
    for g in glyphs {
        match lines.last_mut() {
            Some((y, line)) if (*y - g.y).abs() < 0.45 * g.height => line.push(g),
            _ => lines.push((g.y, vec![g])),
        }
    }
    lines
        .into_iter()
        .map(|(_, line)| line_text(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Rtl,
    Ltr,
    Digit,
    Separator,
    Other,
}

fn class(text: &str) -> Class {
    let strong = text.chars().find_map(|c| match c {
        '0'..='9' | '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}' => Some(Class::Digit),
        '\u{060C}' => None,
        '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}' => {
            Some(Class::Rtl)
        }
        c if c.is_alphabetic() => Some(Class::Ltr),
        _ => None,
    });
    strong.unwrap_or_else(|| match text.chars().next() {
        Some('.' | ',' | ':' | '/' | '-' | '\u{2212}') => Class::Separator,
        _ => Class::Other,
    })
}

/// A line's glyphs in reading order: a footnote call (a figure smaller than the line's, raised
/// above it) and a space drawn inside another glyph are dropped, a gap wider than half a
/// glyph's height is a space, and a right-to-left line reads from its right edge, each run of
/// figures (`65.99`) or of left-to-right words in it from its left.
fn line_text(mut line: Vec<Drawn>) -> String {
    line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let inked: Vec<(f64, f64)> = line
        .iter()
        .filter(|d| !d.text.trim().is_empty())
        .map(|d| (d.x0, d.x1))
        .collect();
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or_default()
    };
    let ink = line.iter().filter(|d| !d.text.trim().is_empty());
    let height = median(ink.clone().map(|d| d.height).collect());
    let middle = median(ink.map(|d| d.y).collect());
    line.retain(|d| {
        let mid = (d.x0 + d.x1) / 2.0;
        let call = class(&d.text) == Class::Digit
            && d.height < 0.8 * height
            && d.y - middle > 0.15 * height;
        !call
            && (!d.text.trim().is_empty()
                || !inked.iter().any(|(a, b)| a + 0.5 < mid && mid < b - 0.5))
    });
    let blank = |d: &Drawn| d.text.trim().is_empty();
    let mut visual: Vec<Drawn> = Vec::with_capacity(line.len());
    for d in line {
        if let Some(last) = visual.last() {
            if d.x0 - last.x1 > 0.5 * d.height && !blank(&d) && !blank(last) {
                visual.push(Drawn {
                    text: " ".to_string(),
                    x0: last.x1,
                    x1: d.x0,
                    ..d.clone()
                });
            }
        }
        visual.push(d);
    }
    let mut classes: Vec<Class> = visual.iter().map(|d| class(&d.text)).collect();
    let count = |k: Class| classes.iter().filter(|c| **c == k).count();
    let (rtl, ltr) = (count(Class::Rtl), count(Class::Ltr));
    if rtl == 0 || ltr > rtl {
        return visual.into_iter().map(|d| d.text).collect();
    }
    for i in 1..classes.len().saturating_sub(1) {
        if classes[i] == Class::Separator
            && classes[i - 1] == Class::Digit
            && classes[i + 1] == Class::Digit
        {
            classes[i] = Class::Digit;
        }
    }
    let strong = |k: &Class| matches!(k, Class::Rtl | Class::Ltr | Class::Digit);
    for i in 0..classes.len() {
        if !strong(&classes[i]) {
            let before = classes[..i].iter().rev().find(|k| strong(k));
            let after = classes[i + 1..].iter().find(|k| strong(k));
            if before == Some(&Class::Ltr) && after == Some(&Class::Ltr) {
                classes[i] = Class::Ltr;
            }
        }
    }
    let mut order: Vec<usize> = (0..visual.len()).collect();
    order.sort_by(|&a, &b| visual[b].x1.total_cmp(&visual[a].x1));
    let mut out = String::new();
    let mut i = 0;
    while i < order.len() {
        let k = classes[order[i]];
        let mut j = i + 1;
        if matches!(k, Class::Ltr | Class::Digit) {
            while j < order.len() && classes[order[j]] == k {
                j += 1;
            }
            order[i..j]
                .iter()
                .rev()
                .for_each(|&n| out.push_str(&visual[n].text));
        } else {
            out.push_str(&visual[order[i]].text);
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Glyphs side by side from the left on one line.
    fn row(y: f64, glyphs: &[(&str, f64)]) -> Vec<Drawn> {
        let mut x = 0.0;
        glyphs
            .iter()
            .map(|(text, w)| {
                let d = Drawn {
                    text: text.to_string(),
                    x0: x,
                    x1: x + w,
                    y,
                    height: 16.0,
                };
                x += w;
                d
            })
            .collect()
    }

    #[test]
    fn a_ligature_keeps_its_characters_and_figures_read_left_to_right() {
        let glyphs = row(
            100.0,
            &[
                ("2", 6.0),
                ("6", 6.0),
                (" ", 4.0),
                ("ن", 5.0),
                ("ي", 5.0),
                ("ل", 5.0),
                ("ص", 5.0),
                ("ف", 5.0),
                ("ل", 5.0),
                ("ا", 5.0),
                (" ", 4.0),
                ("ا", 5.0),
                ("م", 5.0),
                ("ي", 5.0),
                ("س", 5.0),
                ("لا", 8.0),
                ("و", 5.0),
            ],
        );
        assert_eq!(page_text(glyphs), "ولاسيما الفصلين 26");
    }

    #[test]
    fn a_dotted_number_and_a_bracket_stay_whole_and_a_latin_line_reads_from_the_left() {
        let mut glyphs = row(
            200.0,
            &[
                ("1", 6.0),
                ("1", 6.0),
                ("(", 4.0),
                (" ", 4.0),
                ("6", 6.0),
                ("5", 6.0),
                (".", 3.0),
                ("9", 6.0),
                ("9", 6.0),
                (" ", 4.0),
                ("م", 5.0),
                ("ق", 5.0),
                ("ر", 5.0),
            ],
        );
        glyphs.extend(row(
            180.0,
            &[("C", 6.0), ("o", 6.0), ("d", 6.0), ("e", 6.0)],
        ));
        assert_eq!(page_text(glyphs), "رقم 65.99 (11\nCode");
    }

    #[test]
    fn a_raised_small_figure_is_a_footnote_call_and_is_dropped() {
        let mut glyphs = row(
            100.0,
            &[("3", 6.0), (" ", 4.0), ("م", 5.0), ("ق", 5.0), ("ر", 5.0)],
        );
        glyphs.insert(
            0,
            Drawn {
                text: "2".to_string(),
                x0: -4.0,
                x1: 0.0,
                y: 104.0,
                height: 10.0,
            },
        );
        assert_eq!(page_text(glyphs), "رقم 3");
    }

    #[test]
    fn a_space_drawn_inside_a_letter_is_dropped() {
        let mut glyphs = row(
            100.0,
            &[("د", 5.0), ("م", 5.0), ("ح", 8.0), ("ل", 4.0), ("ا", 4.0)],
        );
        glyphs.push(Drawn {
            text: " ".to_string(),
            x0: 11.0,
            x1: 14.0,
            y: 100.0,
            height: 16.0,
        });
        assert_eq!(page_text(glyphs), "الحمد");
    }
}
