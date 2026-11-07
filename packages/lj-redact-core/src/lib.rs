//! Search a document for the strings a user names, remove them, and prove it by
//! re-reading what was produced. The promise is never "we found everything to
//! hide"; it is "what was named has gone", and [`Outcome::leaks`] is the proof.

pub mod detect;
pub mod fold;
pub mod index;
pub mod labels;
#[cfg(feature = "pdf")]
pub mod layout;
#[cfg(feature = "office")]
pub mod office;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod scan;

pub use detect::{suggest, Kind, Suggestion};
pub use fold::Spaces;
pub use index::{Anchor, TextIndex};

/// What one redaction pass produced.
pub struct Outcome {
    /// The redacted document.
    pub bytes: Vec<u8>,
    /// Occurrences located across every needle.
    pub found: usize,
    /// Glyphs the backend removed.
    pub removed: usize,
    /// Needles still readable in `bytes`. Non-empty means refuse to deliver.
    pub leaks: Vec<String>,
    /// Extractable characters the source carried. Zero means no text layer, so
    /// nothing here says anything about what the pages show.
    pub source_chars: usize,
}

/// One located occurrence: which needle matched, and the glyphs it covers.
pub struct Occurrence {
    /// Index into the needles handed to [`locate`].
    pub needle: usize,
    /// The source glyphs the match covers, in reading order.
    pub anchors: Vec<Anchor>,
    /// Where the match sits in the source feed, which is what a text view
    /// highlights on.
    pub source: std::ops::Range<usize>,
}

/// A run of characters the reader sees as one paragraph or one page, addressed
/// by its position in the source feed. Both backends separate them the same
/// way, so one text view serves a PDF and a package alike.
pub struct Block {
    pub start: usize,
    pub text: String,
}

/// Cut the source feed into what a reader sees as separate blocks.
pub fn blocks(source: &[(char, Anchor)]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut text = String::new();
    for (at, (c, _)) in source.iter().enumerate() {
        if *c == '\n' {
            if !text.trim().is_empty() {
                out.push(Block {
                    start,
                    text: std::mem::take(&mut text),
                });
            }
            text.clear();
            start = at + 1;
            continue;
        }
        if text.is_empty() {
            start = at;
        }
        text.push(*c);
    }
    if !text.trim().is_empty() {
        out.push(Block { start, text });
    }
    out
}

/// Fold `glyphs` both ways and return one entry per distinct occurrence.
///
/// The two folds overlap: a name written plainly is found by both, and counting
/// it twice would tell the user two names were removed where there was one. Two
/// hits covering the same source characters are the same occurrence. A hit
/// stands on word boundaries wherever the needle starts or ends with a letter
/// or a digit: `Marx` is not in `marxisme`. Across blanks the needle lacks, a
/// hit is a let-spaced `D U P O N T` only when no letter touches it at all,
/// never `Organisation s` of `Organisation syndicale`.
pub fn locate(glyphs: &[(char, Anchor)], needles: &[String]) -> Vec<Occurrence> {
    let mut occurrences: Vec<Occurrence> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for spaces in [Spaces::Collapse, Spaces::Strip] {
        let index = TextIndex::build(glyphs.iter().copied(), spaces);
        for (needle, text) in needles.iter().enumerate() {
            let spaced = text.chars().any(char::is_whitespace);
            for hit in index.find(text) {
                let mut anchors = index.anchors(hit.clone()).to_vec();
                let source = index.source(hit);
                if inside_word(glyphs, &source) {
                    continue;
                }
                let across_blanks = spaces == Spaces::Strip
                    && !spaced
                    && glyphs[source.clone()]
                        .iter()
                        .any(|(c, _)| c.is_whitespace());
                if across_blanks {
                    let word = |at: Option<usize>| {
                        at.and_then(|at| glyphs.get(at))
                            .is_some_and(|(c, _)| c.is_alphanumeric())
                    };
                    if word(source.start.checked_sub(1)) || word(Some(source.end)) {
                        continue;
                    }
                    // Its blanks go with it, as the highlight of `source` shows it.
                    anchors = glyphs[source.clone()].iter().map(|(_, a)| *a).collect();
                }
                if seen.insert((source.start, source.end)) {
                    occurrences.push(Occurrence {
                        needle,
                        anchors,
                        source,
                    });
                }
            }
        }
    }
    occurrences
}

/// Whether a hit on `source` continues a word of `glyphs` at either end: a
/// letter beside a letter, a digit beside a digit. A letter beside a digit
/// is a boundary, as a note call glued to a name (`Dupont1`), and so is a
/// rise from lower to upper case, as in text a layout glued together
/// (`MonsieurDUPONT`), and so is the pen moved between two PDF glyphs, a word
/// space set by position rather than written; characters that fold to nothing
/// are skipped.
fn inside_word(glyphs: &[(char, Anchor)], source: &std::ops::Range<usize>) -> bool {
    let shown = |at: &usize| lj_fold::fold_char(glyphs[*at].0).len() > 0;
    let edge = |outer: Option<&usize>, inner: Option<&usize>, rising: bool| {
        let (Some(&outer), Some(&inner)) = (outer, inner) else {
            return false;
        };
        let left = match rising {
            true => inner - 1,
            false => inner,
        };
        if pen_moved(&glyphs[left].1, &glyphs[left + 1].1) {
            return false;
        }
        let (o, i) = (glyphs[outer].0, glyphs[inner].0);
        let case_edge = match rising {
            true => o.is_lowercase() && i.is_uppercase(),
            false => i.is_uppercase() && o.is_lowercase(),
        };
        let letters = o.is_alphabetic() && i.is_alphabetic();
        let digits = o.is_numeric() && i.is_numeric();
        (letters && !case_edge) || digits
    };
    let before = (0..source.start).rev().find(shown);
    let first = source.clone().find(shown);
    let last = source.clone().rev().find(shown);
    let after = (source.end..glyphs.len()).find(shown);
    edge(before.as_ref(), first.as_ref(), true) || edge(after.as_ref(), last.as_ref(), false)
}

#[cfg(feature = "pdf")]
fn pen_moved(left: &Anchor, right: &Anchor) -> bool {
    match (left.pdf(), right.pdf()) {
        (Some((p, a)), Some((q, b))) => {
            p == q && layout::word_space(&layout::bounds(&a), &layout::bounds(&b))
        }
        _ => false,
    }
}

#[cfg(not(feature = "pdf"))]
fn pen_moved(_: &Anchor, _: &Anchor) -> bool {
    false
}

/// Which needles a document still yields, after folding it both ways.
pub fn survivors(glyphs: &[(char, Anchor)], needles: &[String]) -> Vec<String> {
    let mut found: Vec<usize> = locate(glyphs, needles)
        .into_iter()
        .map(|o| o.needle)
        .collect();
    found.sort_unstable();
    found.dedup();
    found.into_iter().map(|n| needles[n].clone()).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One anchor per character, numbered by its position in `text`.
    pub(crate) fn glyphs(text: &str) -> Vec<(char, Anchor)> {
        text.chars()
            .enumerate()
            .map(|(i, c)| (c, anchor(i)))
            .collect()
    }

    #[cfg(feature = "pdf")]
    fn anchor(at: usize) -> Anchor {
        let x = at as f64;
        let quad = [[x, 0.0], [x + 1.0, 0.0], [x + 1.0, 1.0], [x, 1.0]];
        Anchor::PdfGlyph { page: 0, quad }
    }

    #[cfg(not(feature = "pdf"))]
    fn anchor(at: usize) -> Anchor {
        Anchor::Office(lj_office::Anchor::XmlText {
            part: 0,
            node: 0,
            offset: at as u32,
        })
    }

    /// The position [`glyphs`] numbered `anchor` with.
    pub(crate) fn position(anchor: &Anchor) -> usize {
        match *anchor {
            #[cfg(feature = "pdf")]
            Anchor::PdfGlyph { quad, .. } => quad[0][0] as usize,
            #[cfg(feature = "office")]
            Anchor::Office(lj_office::Anchor::XmlText { offset, .. }) => offset as usize,
            #[cfg(feature = "office")]
            Anchor::Office(lj_office::Anchor::DocChar { cp, .. }) => cp as usize,
            #[cfg(feature = "office")]
            Anchor::Office(lj_office::Anchor::DocValue { offset, .. }) => offset as usize,
        }
    }

    #[test]
    fn a_name_written_plainly_is_one_occurrence_not_one_per_fold() {
        let found = locate(
            &glyphs("Claire DUBOIS demeure ici, et Claire DUBOIS signe."),
            &["Claire DUBOIS".to_string()],
        );
        assert_eq!(found.len(), 2);
    }

    fn sources(text: &str, needle: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        locate(&glyphs(text), &[needle.to_string()])
            .into_iter()
            .map(|o| chars[o.source].iter().collect())
            .collect()
    }

    #[test]
    fn a_needle_is_found_as_whole_words_never_inside_a_longer_one() {
        let text = "Marx, le marxisme de MARX et le MARXISME des Marxistes, (Marx).";
        assert_eq!(sources(text, "Marx"), ["Marx", "MARX", "Marx"]);
        assert_eq!(sources("12 rue du Pont, Dupont", "pont"), ["Pont"]);
        assert_eq!(sources("Jean-Pierre et Jeanne", "Jean"), ["Jean"]);
        assert_eq!(sources("l'Oréal, d'Estaing", "Estaing"), ["Estaing"]);
    }

    #[test]
    fn a_needle_edged_with_punctuation_is_bounded_only_where_it_has_letters() {
        assert_eq!(sources("M. Dupont, Dupontel", "M. Dupont"), ["M. Dupont"]);
        assert_eq!(sources("(Dupont)x", "(Dupont)"), ["(Dupont)"]);
    }

    #[test]
    fn a_rise_to_capitals_is_a_word_boundary_where_a_layout_glued_two_words() {
        assert_eq!(sources("MonsieurDUPONTdemeure", "Dupont"), ["DUPONT"]);
        assert_eq!(sources("Du\u{00ad}pont", "pont"), Vec::<String>::new());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn a_word_space_set_by_moving_the_pen_between_pdf_glyphs_is_a_word_boundary() {
        let mut x = 0.0;
        let mut placed = Vec::new();
        for word in ["Monsieur", "Dupont", "demeure"] {
            for c in word.chars() {
                let quad = [[x, 0.0], [x + 6.0, 0.0], [x + 6.0, 12.0], [x, 12.0]];
                placed.push((c, Anchor::PdfGlyph { page: 0, quad }));
                x += 6.0;
            }
            x += 2.4;
        }
        let found = locate(&placed, &["Dupont".to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, 8..14);
        assert!(locate(&glyphs("Dupontdemeure"), &["Dupont".to_string()]).is_empty());
    }

    #[test]
    fn a_digit_beside_a_letter_is_a_word_boundary_and_beside_a_digit_is_not() {
        assert_eq!(sources("Dupont1 signe", "Dupont"), ["Dupont"]);
        assert_eq!(sources("750012 et 75001", "75001"), ["75001"]);
    }

    #[test]
    fn a_comma_between_two_words_of_the_document_does_not_part_a_name() {
        let text = "Pascal, VERDIER et Pascal VERDIER, puis Pascal,VERDIER";
        assert_eq!(
            sources(text, "Pascal VERDIER"),
            ["Pascal, VERDIER", "Pascal VERDIER"]
        );
    }

    #[test]
    fn a_needle_let_spaced_is_found_whole_and_one_straddling_two_words_is_not() {
        let needles = ["Dupont".to_string(), "Organisations".to_string()];
        let text = "M. D U P O N T, l'Organisation syndicale.";
        let found = locate(&glyphs(text), &needles);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].needle, 0);
        let spaced: String = text.chars().skip(3).take(11).collect();
        assert_eq!(spaced, "D U P O N T");
        assert_eq!(found[0].source, 3..14);
        assert_eq!(found[0].anchors.len(), 11);
    }
}
