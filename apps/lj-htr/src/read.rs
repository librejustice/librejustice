//! The words of a line: read by the line reader, read again by the handwriting
//! reader when the first reading is not French, the one the lexicon knows kept.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::detect::Segment;

/// One word read, on the page's pixels, and the line it sits on.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Word {
    pub line: usize,
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The symbol of each class a reader outputs, class 0 being the CTC blank.
pub struct Alphabet(Vec<String>);

impl Alphabet {
    /// The line reader's dictionary: one symbol a line, after the blank and
    /// before the space.
    pub fn paddle(dictionary: &str) -> Alphabet {
        let mut symbols = vec![String::new()];
        symbols.extend(dictionary.lines().map(str::to_string));
        symbols.push(" ".into());
        Alphabet(symbols)
    }

    /// The handwriting reader's table: `symbol class` a line, the space
    /// written `<space>`.
    pub fn laia(table: &str) -> Alphabet {
        let mut symbols = Vec::new();
        for line in table.lines() {
            let Some((symbol, class)) = line.rsplit_once(' ') else {
                continue;
            };
            let class: usize = class.parse().expect("a class number");
            if symbols.len() <= class {
                symbols.resize(class + 1, String::new());
            }
            symbols[class] = match symbol {
                "<space>" => " ".into(),
                "<ctc>" => String::new(),
                other => other.into(),
            };
        }
        Alphabet(symbols)
    }
}

/// One symbol a reader emitted, and the frames it spans.
struct Glyph {
    symbol: usize,
    first: usize,
    last: usize,
    p: f32,
}

/// A greedy CTC reading of `frames` rows of `classes` probabilities.
pub struct Reading {
    glyphs: Vec<Glyph>,
    frames: usize,
}

impl Reading {
    pub fn decode(probs: &[f32], classes: usize) -> Reading {
        let frames = probs.len() / classes;
        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut previous = 0;
        for (t, row) in probs.chunks_exact(classes).enumerate() {
            let (best, p) =
                row.iter()
                    .enumerate()
                    .fold((0, f32::MIN), |b, (i, &p)| if p > b.1 { (i, p) } else { b });
            match glyphs.last_mut() {
                Some(glyph) if best == previous && best != 0 => glyph.last = t,
                _ if best != 0 => glyphs.push(Glyph {
                    symbol: best,
                    first: t,
                    last: t,
                    p,
                }),
                _ => {}
            }
            previous = best;
        }
        Reading { glyphs, frames }
    }

    pub fn confidence(&self) -> f32 {
        match self.glyphs.len() {
            0 => 0.0,
            n => self.glyphs.iter().map(|g| g.p).sum::<f32>() / n as f32,
        }
    }

    /// The words, placed on `segment`, which fills `seen` pixels of an input
    /// `wide` across. A word takes one frame more each side than it fires on.
    pub fn words(&self, alphabet: &Alphabet, segment: &Segment, seen: f32, wide: f32) -> Vec<Word> {
        let stride = wide / self.frames as f32;
        let scale = segment.width / seen;
        let mut words = Vec::new();
        let mut run: Vec<&Glyph> = Vec::new();
        let mut flush = |run: &mut Vec<&Glyph>| {
            if let (Some(first), Some(last)) = (run.first(), run.last()) {
                let x0 = (first.first.saturating_sub(1) as f32 * stride).min(seen);
                let x1 = ((last.last + 2) as f32 * stride).min(seen);
                words.push(Word {
                    line: segment.line,
                    text: run.iter().map(|g| alphabet.0[g.symbol].as_str()).collect(),
                    x: segment.x + x0 * scale,
                    y: segment.y,
                    width: (x1 - x0) * scale,
                    height: segment.height,
                });
            }
            run.clear();
        };
        for glyph in &self.glyphs {
            match alphabet.0[glyph.symbol].as_str() {
                " " => flush(&mut run),
                _ => run.push(glyph),
            }
        }
        flush(&mut run);
        words
    }
}

/// Folded words, the first field of each line of a frequency list.
pub struct Lexicon(HashSet<String>);

impl Lexicon {
    pub fn parse(list: &str) -> Lexicon {
        Lexicon(
            list.lines()
                .filter_map(|l| l.split_whitespace().next())
                .map(lj_fold::fold)
                .collect(),
        )
    }

    /// How many of `words` the lexicon knows, a number counting as known.
    fn known(&self, words: &[String]) -> usize {
        words
            .iter()
            .filter(|w| w.chars().all(|c| c.is_ascii_digit()) || self.0.contains(*w))
            .count()
    }

    /// Share of the words of `text` the lexicon knows, and how many it has.
    pub fn share(&self, text: &str) -> (f32, usize) {
        let words = words(text);
        (
            self.known(&words) as f32 / words.len().max(1) as f32,
            words.len(),
        )
    }
}

/// The folded words of `text`, an elided article dropped.
fn words(text: &str) -> Vec<String> {
    lj_fold::fold(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(|w| w.rsplit('\'').next().unwrap_or(w).to_string())
        .collect()
}

/// French, the handwriting reader's language, and the other languages the
/// line reader reads, which tell a page it cannot read.
pub struct Lexicons {
    pub french: Lexicon,
    pub others: Vec<Lexicon>,
}

/// One line: the line reader's words and confidence, and the handwriting
/// reader's words when it read the line again.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Line {
    pub confidence: f32,
    pub printed: Vec<Word>,
    pub hand: Option<Vec<Word>>,
}

/// The line reader's confidence from which its reading stands alone.
pub const TRUST: f32 = 0.9;

/// How much less of its words the lexicon may know in the handwriting
/// reading, which still wins: its misreadings are French words, the line
/// reader's are not.
pub const MARGIN: f32 = 0.1;

/// Fewer words, and the handwriting reader wins on chance: a misread single
/// word costs a name on a handwritten form.
pub const MIN_WORDS: usize = 2;

/// Share of the doubtful lines the handwriting reader wins from which the
/// page is handwritten, and it takes them all: reading the page whole beats
/// choosing line by line.
pub const HANDWRITTEN: f32 = 0.5;

/// Whether the line reader doubts its reading enough to have the
/// handwriting reader read the line again.
pub fn doubtful(line: &Line) -> bool {
    line.confidence < TRUST
}

fn text(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The words of a page's lines, each line read by the reader the page
/// trusts. The handwriting reader reads French alone: on English handwriting
/// it reads worse than the line reader.
pub fn page(lexicons: &Lexicons, lines: Vec<Line>) -> Vec<Word> {
    let every: Vec<String> = lines
        .iter()
        .flat_map(|l| {
            let hand = l.hand.as_deref().map(text).unwrap_or_default();
            words(&format!("{} {hand}", text(&l.printed)))
        })
        .collect();
    let french = lexicons.french.known(&every);
    let foreign = lexicons.others.iter().any(|l| l.known(&every) > french);

    let doubtful =
        |l: &Line| l.confidence < TRUST && l.hand.as_ref().is_some_and(|h| !h.is_empty());
    let wins = |l: &Line| {
        let Some(hand) = l.hand.as_deref() else {
            return false;
        };
        let (share, _) = lexicons.french.share(&text(&l.printed));
        let (other, count) = lexicons.french.share(&text(hand));
        doubtful(l) && count >= MIN_WORDS && other >= share - MARGIN
    };
    let asked = lines.iter().filter(|l| doubtful(l)).count();
    let won = lines.iter().filter(|l| wins(l)).count();
    let handwritten = asked > 0 && won as f32 >= HANDWRITTEN * asked as f32;

    lines
        .into_iter()
        .flat_map(|l| {
            let take = !foreign && if handwritten { doubtful(&l) } else { wins(&l) };
            match (take, l.hand) {
                (true, Some(hand)) => hand,
                _ => l.printed,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(classes: usize, rows: &[(usize, f32)]) -> Vec<f32> {
        rows.iter()
            .flat_map(|&(best, p)| {
                (0..classes).map(move |c| match c == best {
                    true => p,
                    false => (1.0 - p) / (classes - 1) as f32,
                })
            })
            .collect()
    }

    fn alphabet() -> Alphabet {
        Alphabet::paddle("a\nb\nc")
    }

    #[test]
    fn repeats_merge_and_a_blank_splits_a_double_letter() {
        let reading = Reading::decode(
            &frames(
                5,
                &[
                    (1, 0.9),
                    (1, 0.9),
                    (0, 0.9),
                    (1, 0.8),
                    (4, 0.9),
                    (2, 0.7),
                    (2, 0.9),
                ],
            ),
            5,
        );
        let segment = Segment {
            line: 0,
            x: 0.0,
            y: 0.0,
            width: 56.0,
            height: 8.0,
        };
        assert_eq!(
            text(&reading.words(&alphabet(), &segment, 56.0, 56.0)),
            "aa b"
        );
        assert!((reading.confidence() - (0.9 + 0.8 + 0.9 + 0.7) / 4.0).abs() < 1e-6);
    }

    #[test]
    fn words_split_on_the_space_and_sit_on_their_frames() {
        let blank = (0, 0.9);
        let reading = Reading::decode(
            &frames(
                5,
                &[
                    blank,
                    (1, 0.9),
                    (2, 0.9),
                    (4, 0.9),
                    (3, 0.9),
                    blank,
                    blank,
                    blank,
                    blank,
                    blank,
                ],
            ),
            5,
        );
        let segment = Segment {
            line: 3,
            x: 100.0,
            y: 50.0,
            width: 20.0,
            height: 10.0,
        };
        // Frames of 8 input pixels; the line fills 40 of the 80, at half scale.
        let words = reading.words(&alphabet(), &segment, 40.0, 80.0);
        let placed: Vec<(&str, f32, f32)> = words
            .iter()
            .map(|w| (w.text.as_str(), w.x, w.width))
            .collect();
        assert_eq!(placed, vec![("ab", 100.0, 16.0), ("c", 112.0, 8.0)]);
        assert!(words
            .iter()
            .all(|w| w.line == 3 && w.y == 50.0 && w.height == 10.0));
    }

    fn line(confidence: f32, printed: &str, hand: Option<&str>) -> Line {
        let words = |t: &str| {
            t.split(' ')
                .map(|w| Word {
                    line: 0,
                    text: w.into(),
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                })
                .collect()
        };
        Line {
            confidence,
            printed: words(printed),
            hand: hand.map(words),
        }
    }

    fn lexicons() -> Lexicons {
        Lexicons {
            french: Lexicon::parse("le\nchat\ndort\nsur\nun\nlit\n"),
            others: vec![Lexicon::parse("the\ncat\nsleeps\non\na\nbed\n")],
        }
    }

    fn read(lines: Vec<Line>) -> String {
        text(&page(&lexicons(), lines))
    }

    #[test]
    fn a_doubted_line_goes_to_the_handwriting_reading_the_lexicon_knows() {
        let lines = vec![
            line(0.95, "le chat dort", None),
            line(0.6, "lx chxt dxrt", Some("le chat dort")),
            line(0.6, "sur un lit", Some("sxr xn lxt")),
            line(0.6, "le lit", Some("lx lxt")),
            line(0.97, "sur un lit", None),
        ];
        assert_eq!(
            read(lines),
            "le chat dort le chat dort sur un lit le lit sur un lit"
        );
    }

    #[test]
    fn a_single_word_line_keeps_the_line_reader_on_a_printed_page() {
        let lines = vec![
            line(0.95, "le chat dort", None),
            line(0.6, "Kimanh", Some("m.")),
            line(0.6, "sur un lit", Some("sxr xn lxt")),
        ];
        assert_eq!(read(lines), "le chat dort Kimanh sur un lit");
    }

    #[test]
    fn a_handwritten_page_gives_every_doubted_line_to_the_handwriting_reader() {
        let lines = vec![
            line(0.6, "lx chxt dxrt", Some("le chat dort")),
            line(0.6, "sxr xn", Some("sur un")),
            line(0.6, "Kirnanh", Some("Kimanh")),
        ];
        assert_eq!(read(lines), "le chat dort sur un Kimanh");
    }

    #[test]
    fn a_page_in_another_language_keeps_the_line_reader() {
        let lines = vec![
            line(0.6, "the cat sleeps", Some("le chat dort")),
            line(0.7, "on a bed", Some("un lit")),
            line(0.95, "the cat", None),
        ];
        assert_eq!(read(lines), "the cat sleeps on a bed the cat");
    }

    #[test]
    fn the_lexicon_folds_and_drops_the_elided_article() {
        let lexicon = Lexicon::parse("école 10\nété 5\n");
        assert_eq!(lexicon.share("L'ECOLE"), (1.0, 1));
        assert_eq!(lexicon.share("l\u{2019}été zz"), (0.5, 2));
        assert_eq!(lexicon.share(""), (0.0, 0));
    }
}
