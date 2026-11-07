//! Folding applied to the document and to the query, which must be the same
//! function on both sides or a typed name never meets a typeset one.

/// Whitespace handling, the one folding choice that has to stay open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spaces {
    /// Runs of whitespace become a single space.
    Collapse,
    /// Whitespace is dropped entirely, so a name a producer let-spaced
    /// (`D U P O N T`) or glued (`AUDIENCEPUBLIQUE`) still matches.
    Strip,
}

/// Append the folded form of `c` to `out`, which can be several characters
/// (ligatures) or none (combining marks, zero-width joiners). Whitespace folds
/// to `' '`; [`Spaces`] is applied by the caller, which is what lets one pass
/// over the source feed both indexes.
pub fn fold_char(c: char, out: &mut Vec<char>) {
    out.extend(lj_fold::fold_char(c));
}

/// [`fold_char`] knowing the character that follows: a comma before a blank
/// folds to a blank, so `Pascal, VERDIER` reads as `Pascal VERDIER`.
pub fn fold_char_before(c: char, next: Option<char>, out: &mut Vec<char>) {
    let start = out.len();
    fold_char(c, out);
    if out[start..] == [','] && next.is_some_and(char::is_whitespace) {
        out[start] = ' ';
    }
}

/// Fold a whole query string.
pub fn fold_query(query: &str, spaces: Spaces) -> Vec<char> {
    let mut out = Vec::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        fold_char_before(c, chars.peek().copied(), &mut out);
    }
    match spaces {
        Spaces::Strip => out.retain(|c| *c != ' '),
        Spaces::Collapse => collapse(&mut out),
    }
    out
}

/// Reduce runs of spaces to one, and drop leading and trailing space.
pub fn collapse(chars: &mut Vec<char>) {
    let mut previous_space = true;
    chars.retain(|c| {
        let space = *c == ' ';
        let keep = !(space && previous_space);
        previous_space = space;
        keep
    });
    if chars.last() == Some(&' ') {
        chars.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(s: &str) -> String {
        fold_query(s, Spaces::Collapse).into_iter().collect()
    }

    fn fold_stripped(s: &str) -> String {
        fold_query(s, Spaces::Strip).into_iter().collect()
    }

    #[test]
    fn accents_case_and_spacing_fold_away() {
        assert_eq!(fold("Éric DUPONT-MAREUIL"), "eric dupont-mareuil");
        assert_eq!(fold("  Maître   Noël  "), "maitre noel");
        assert_eq!(fold("Ça ÎLE ŸN"), "ca ile yn");
    }

    #[test]
    fn ligatures_expand_so_a_typed_name_matches_a_typeset_one() {
        assert_eq!(fold("\u{fb01}n"), "fin");
        assert_eq!(fold("SŒUR"), "soeur");
        assert_eq!(fold("Straße"), "strasse");
    }

    #[test]
    fn invisible_characters_never_break_a_match() {
        assert_eq!(fold("Du\u{00ad}pont"), "dupont");
        assert_eq!(fold("Du\u{200b}pont"), "dupont");
        assert_eq!(fold("e\u{0301}te\u{0301}"), "ete");
    }

    #[test]
    fn stripping_spaces_matches_letter_spaced_and_glued_text() {
        assert_eq!(fold_stripped("D U P O N T"), "dupont");
        assert_eq!(fold_stripped("Dupont"), "dupont");
    }

    #[test]
    fn typographic_punctuation_folds_onto_its_ascii_form() {
        assert_eq!(fold("l\u{2019}affaire"), "l'affaire");
        assert_eq!(fold("A\u{2013}B"), "a-b");
        assert_eq!(fold("a\u{00a0}b"), "a b");
    }

    #[test]
    fn a_comma_before_a_blank_is_a_blank_and_one_inside_a_word_stays() {
        assert_eq!(fold("Pascal, VERDIER"), "pascal verdier");
        assert_eq!(fold_stripped("Pascal,\nVERDIER"), "pascalverdier");
        assert_eq!(fold("1,5 m"), "1,5 m");
        assert_eq!(fold("fin,"), "fin,");
    }
}
