//! The one fold every comparison key goes through: case, diacritics,
//! ligatures, typographic punctuation and invisible characters. A character can
//! fold to several (`œ` to `oe`) or to none (a combining mark), which is the
//! folding tantivy's `AsciiFoldingFilter` applies to the search index.

/// What one character folds to: up to three characters.
#[derive(Clone, Debug)]
pub struct Folded {
    chars: [char; 3],
    len: u8,
    at: u8,
}

impl Folded {
    #[inline]
    fn none() -> Self {
        Folded {
            chars: ['\0'; 3],
            len: 0,
            at: 0,
        }
    }

    #[inline]
    fn one(c: char) -> Self {
        Folded {
            chars: [c, '\0', '\0'],
            len: 1,
            at: 0,
        }
    }

    #[inline]
    fn of(chars: impl IntoIterator<Item = char>) -> Self {
        let mut out = Folded::none();
        for c in chars {
            out.chars[out.len as usize] = c;
            out.len += 1;
        }
        out
    }
}

impl Iterator for Folded {
    type Item = char;

    #[inline]
    fn next(&mut self) -> Option<char> {
        (self.at < self.len).then(|| {
            self.at += 1;
            self.chars[self.at as usize - 1]
        })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = (self.len - self.at) as usize;
        (left, Some(left))
    }
}

impl ExactSizeIterator for Folded {}

/// Latin-1 Supplement and Latin Extended-A, U+00C0 to U+017F.
const LATIN: [&str; 192] = [
    "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i", "d", "n", "o",
    "o", "o", "o", "o", "×", "o", "u", "u", "u", "u", "y", "th", "ss", "a", "a", "a", "a", "a",
    "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i", "d", "n", "o", "o", "o", "o", "o", "÷",
    "o", "u", "u", "u", "u", "y", "th", "y", "a", "a", "a", "a", "a", "a", "c", "c", "c", "c", "c",
    "c", "c", "c", "d", "d", "d", "d", "e", "e", "e", "e", "e", "e", "e", "e", "e", "e", "g", "g",
    "g", "g", "g", "g", "g", "g", "h", "h", "h", "h", "i", "i", "i", "i", "i", "i", "i", "i", "i",
    "i", "ij", "ij", "j", "j", "k", "k", "k", "l", "l", "l", "l", "l", "l", "l", "l", "l", "l",
    "n", "n", "n", "n", "n", "n", "'n", "n", "n", "o", "o", "o", "o", "o", "o", "oe", "oe", "r",
    "r", "r", "r", "r", "r", "s", "s", "s", "s", "s", "s", "s", "s", "t", "t", "t", "t", "t", "t",
    "u", "u", "u", "u", "u", "u", "u", "u", "u", "u", "u", "u", "w", "w", "y", "y", "y", "z", "z",
    "z", "z", "z", "z", "s",
];

/// Fold one character. Whitespace folds to `' '`; what a caller does with runs
/// of it is the caller's choice.
#[inline]
pub fn fold_char(c: char) -> Folded {
    if c.is_ascii() {
        return Folded::one(fold_ascii(c));
    }
    match c {
        '\u{00c0}'..='\u{017f}' => Folded::of(LATIN[c as usize - 0xc0].chars()),
        '\u{00ad}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'
        | '\u{feff}' => Folded::none(),
        '\u{0300}'..='\u{036f}' => Folded::none(),
        '\u{fb00}' => Folded::of(['f', 'f']),
        '\u{fb01}' => Folded::of(['f', 'i']),
        '\u{fb02}' => Folded::of(['f', 'l']),
        '\u{fb03}' => Folded::of(['f', 'f', 'i']),
        '\u{fb04}' => Folded::of(['f', 'f', 'l']),
        '\u{fb05}' | '\u{fb06}' => Folded::of(['s', 't']),
        '\u{2018}' | '\u{2019}' | '\u{02bc}' | '\u{00b4}' => Folded::one('\''),
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{00ab}' | '\u{00bb}' => Folded::one('"'),
        '\u{2010}'..='\u{2015}' | '\u{2212}' => Folded::one('-'),
        c if c.is_whitespace() => Folded::one(' '),
        c => Folded::of(c.to_lowercase()),
    }
}

#[inline]
fn fold_ascii(c: char) -> char {
    if c.is_ascii_whitespace() {
        ' '
    } else {
        c.to_ascii_lowercase()
    }
}

/// Append the fold of `c` to `out`.
#[inline]
pub fn push_folded(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push(fold_ascii(c));
    } else {
        for f in fold_char(c) {
            out.push(f);
        }
    }
}

/// Fold a string character by character, whitespace kept one for one.
pub fn fold_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_folded(c, &mut out);
    }
    out
}

/// Fold a string into a comparison key: runs of whitespace become one space,
/// none at either end.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_folded(c, &mut out);
        if out.ends_with("  ") || out == " " {
            out.pop();
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The fold reduced to one character per character, for a caller that indexes
/// the folded text by source position. It keeps the first character of an
/// expansion (`œ` gives `o`) and a character the fold drops.
#[inline]
pub fn fold_one(c: char) -> char {
    if c.is_ascii() {
        return fold_ascii(c);
    }
    fold_char(c).next().unwrap_or(c)
}

/// Fold, then every run of what is not an ASCII letter or digit becomes `sep`,
/// none at either end.
pub fn slug(s: &str, sep: char) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(fold_char) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(sep) {
            out.push(sep);
        }
    }
    if out.ends_with(sep) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_case_and_ligatures_fold_to_ascii() {
        assert_eq!(fold("Éric DUPONT-MARTIN"), "eric dupont-martin");
        assert_eq!(fold("SŒUR Cœur ÆSOPE"), "soeur coeur aesope");
        assert_eq!(fold("Straße \u{fb01}n"), "strasse fin");
        assert_eq!(fold("Muñoz Ørsted Łódź Ÿves"), "munoz orsted lodz yves");
    }

    #[test]
    fn invisible_characters_and_combining_marks_vanish() {
        assert_eq!(fold("Du\u{00ad}pont Du\u{200b}pont"), "dupont dupont");
        assert_eq!(fold("e\u{0301}te\u{0301}"), "ete");
    }

    #[test]
    fn typographic_punctuation_and_whitespace_fold_to_ascii() {
        assert_eq!(
            fold("  l\u{2019}affaire\u{00a0}A\u{2013}B\n« x » "),
            "l'affaire a-b \" x \""
        );
        assert_eq!(fold_chars("a\u{00a0}\u{00a0}b"), "a  b");
    }

    #[test]
    fn one_for_one_keeps_the_first_character_of_an_expansion() {
        assert_eq!(fold_one('Œ'), 'o');
        assert_eq!(fold_one('É'), 'e');
        assert_eq!(fold_one('\u{00a0}'), ' ');
        assert_eq!(fold_one('\u{0301}'), '\u{0301}');
    }

    #[test]
    fn slug_joins_ascii_runs() {
        assert_eq!(slug("Cœur d'Alène (Idaho)", '-'), "coeur-d-alene-idaho");
        assert_eq!(slug("  Saint-Étienne ", '_'), "saint_etienne");
    }
}
