//! What a search query matched in a text, cut and folded as the body index
//! does, to highlight a decision read after a search. No `regex`: the wasm
//! bundle would carry its tables for this alone.

use std::ops::Range;

use crate::body_tok::is_stopword;

#[derive(Debug, Clone, PartialEq)]
enum Term {
    Word(String),
    Prefix(String),
    /// Consecutive tokens, `None` standing for a stopword: the index drops it
    /// and keeps its position, so any token fills the slot.
    Phrase(Vec<Option<String>>),
    /// Words within `slop` positions of each other (`PROCHE`, `"…"~N`).
    Near(Vec<String>, usize),
}

/// The terms of a query to find in a text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Highlight {
    terms: Vec<Term>,
}

struct Tok {
    /// Codepoints.
    span: Range<usize>,
    folded: String,
}

impl Highlight {
    /// The terms of `query` as the search reads it: `ET`/`OU`/`AND`/`OR`
    /// connect, `SAUF`/`NOT` and a leading `-` exclude, `PROCHEn`/`NEARn` join
    /// their operands, `*` ends a prefix, quotes hold a phrase.
    pub fn parse(query: &str) -> Self {
        let items = lex(query);
        let mut units = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            i += 1;
            let unit = match item {
                Item::Bare(w) if matches!(w.as_str(), "ET" | "OU" | "AND" | "OR") => Unit::Gap,
                Item::Bare(w) if matches!(w.as_str(), "SAUF" | "NOT") => {
                    i = skip_operand(&items, i);
                    Unit::Gap
                }
                Item::Bare(w) if w.len() > 1 && w.starts_with('-') => {
                    i = skip_operand(&items, i - 1);
                    Unit::Gap
                }
                Item::Bare(w) => match proximity_slop(w) {
                    Some(slop) => Unit::Near(slop),
                    None => bare_term(w).map_or(Unit::Gap, Unit::Term),
                },
                Item::Phrase(p, slop) => phrase_term(p, *slop).map_or(Unit::Gap, Unit::Term),
            };
            units.push(unit);
        }

        let mut terms: Vec<Term> = Vec::new();
        let mut k = 0;
        while k < units.len() {
            if let Unit::Term(first) = &units[k] {
                let mut near: Option<(Vec<String>, usize)> = None;
                while let (Some(Unit::Near(slop)), Some(Unit::Term(next))) =
                    (units.get(k + 1), units.get(k + 2))
                {
                    let (words, max) = near.get_or_insert_with(|| (words_of(first), 0));
                    words.extend(words_of(next));
                    *max = (*max).max(*slop);
                    k += 2;
                }
                let term = near.map_or_else(|| first.clone(), |(w, s)| Term::Near(w, s));
                if !terms.contains(&term) {
                    terms.push(term);
                }
            }
            k += 1;
        }
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The codepoint ranges of `text` the terms match, sorted and disjoint. A
    /// phrase is one range, from its first token to its last.
    pub fn marks(&self, text: &str) -> Vec<Range<usize>> {
        if self.terms.is_empty() {
            return Vec::new();
        }
        let toks = scan(text);
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for term in &self.terms {
            match term {
                Term::Word(w) => ranges.extend(
                    toks.iter()
                        .filter(|t| t.folded == *w)
                        .map(|t| t.span.clone()),
                ),
                Term::Prefix(p) => ranges.extend(
                    toks.iter()
                        .filter(|t| t.folded.starts_with(p.as_str()))
                        .map(|t| t.span.clone()),
                ),
                Term::Phrase(slots) => {
                    for w in toks.windows(slots.len()) {
                        let hit = slots
                            .iter()
                            .zip(w)
                            .all(|(s, t)| s.as_ref().is_none_or(|s| *s == t.folded));
                        if hit {
                            ranges.push(w[0].span.start..w[w.len() - 1].span.end);
                        }
                    }
                }
                Term::Near(words, slop) => {
                    let window = slop + words.len();
                    let at: Vec<Vec<usize>> = words
                        .iter()
                        .map(|w| (0..toks.len()).filter(|&i| toks[i].folded == *w).collect())
                        .collect();
                    for (wi, own) in at.iter().enumerate() {
                        for &p in own {
                            let joined = at.iter().enumerate().all(|(wj, other)| {
                                wj == wi || other.iter().any(|&q| q.abs_diff(p) <= window)
                            });
                            if joined {
                                ranges.push(toks[p].span.clone());
                            }
                        }
                    }
                }
            }
        }
        ranges.sort_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for r in ranges {
            match merged.last_mut() {
                Some(last) if r.start < last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        merged
    }
}

enum Item {
    Bare(String),
    Phrase(String, Option<usize>),
}

enum Unit {
    Term(Term),
    Near(usize),
    Gap,
}

/// Quoted phrases, with the `~N` glued after them, and whitespace-separated
/// words.
fn lex(query: &str) -> Vec<Item> {
    let chars: Vec<char> = query.chars().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
        } else if chars[i] == '"' {
            let end = (i + 1..chars.len())
                .find(|&j| chars[j] == '"')
                .unwrap_or(chars.len());
            let phrase: String = chars[i + 1..end].iter().collect();
            i = end + 1;
            let mut slop = None;
            if chars.get(i) == Some(&'~') {
                let digits = (i + 1..chars.len())
                    .take_while(|&j| chars[j].is_ascii_digit())
                    .count();
                if digits > 0 {
                    slop = chars[i + 1..i + 1 + digits]
                        .iter()
                        .collect::<String>()
                        .parse()
                        .ok();
                    i += 1 + digits;
                }
            }
            items.push(Item::Phrase(phrase, slop));
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '"' {
                i += 1;
            }
            items.push(Item::Bare(chars[start..i].iter().collect()));
        }
    }
    items
}

/// The index after the operand that starts at `start`, a parenthesised group
/// running to its closing parenthesis.
fn skip_operand(items: &[Item], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    while let Some(item) = items.get(i) {
        i += 1;
        if let Item::Bare(w) = item {
            depth += w.matches('(').count() as i32 - w.matches(')').count() as i32;
        }
        if depth <= 0 {
            break;
        }
    }
    i
}

fn proximity_slop(word: &str) -> Option<usize> {
    let digits = word
        .strip_prefix("PROCHE")
        .or_else(|| word.strip_prefix("NEAR"))?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn bare_term(word: &str) -> Option<Term> {
    let word = word.trim_start_matches(['+', '(']).trim_end_matches(')');
    let prefix = word.ends_with('*');
    let toks = folded_words(word.trim_end_matches('*'));
    if prefix {
        return toks.last().map(|t| Term::Prefix(t.clone()));
    }
    slots_term(toks, None)
}

fn phrase_term(phrase: &str, slop: Option<usize>) -> Option<Term> {
    slots_term(folded_words(phrase), slop)
}

fn slots_term(toks: Vec<String>, slop: Option<usize>) -> Option<Term> {
    let mut slots: Vec<Option<String>> = toks
        .into_iter()
        .map(|t| (!is_stopword(&t)).then_some(t))
        .collect();
    while slots.last().is_some_and(Option::is_none) {
        slots.pop();
    }
    let lead = slots.iter().take_while(|s| s.is_none()).count();
    slots.drain(..lead);
    match (slots.len(), slop) {
        (0, _) => None,
        (1, _) => slots.pop().flatten().map(Term::Word),
        (_, Some(slop)) => Some(Term::Near(slots.into_iter().flatten().collect(), slop)),
        (_, None) => Some(Term::Phrase(slots)),
    }
}

fn words_of(term: &Term) -> Vec<String> {
    match term {
        Term::Word(w) | Term::Prefix(w) => vec![w.clone()],
        Term::Phrase(slots) => slots.iter().flatten().cloned().collect(),
        Term::Near(words, _) => words.clone(),
    }
}

fn folded_words(text: &str) -> Vec<String> {
    scan(text)
        .into_iter()
        .map(|t| t.folded)
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect()
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || c == '-'
}

/// The tokens of `text` as [`crate::body_tok::TOKEN_PATTERN`] cuts them, lone
/// hyphens included: the index counts their positions.
fn scan(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_word_char(chars[i]) {
            i += 1;
            continue;
        }
        let end = number_end(&chars, i).unwrap_or_else(|| {
            (i..chars.len())
                .find(|&j| !is_word_char(chars[j]))
                .unwrap_or(chars.len())
        });
        let raw: String = chars[i..end].iter().collect();
        toks.push(Tok {
            span: i..end,
            folded: lj_fold::fold_chars(&raw),
        });
        i = end;
    }
    toks
}

/// The end of `\p{N}+(?:[./-]\p{N}+)+` from `start`, the pattern's first
/// alternative, which wins wherever it matches.
fn number_end(chars: &[char], start: usize) -> Option<usize> {
    let digits = |from: usize| {
        (from..chars.len())
            .find(|&j| !chars[j].is_numeric())
            .unwrap_or(chars.len())
    };
    let mut end = digits(start);
    if end == start {
        return None;
    }
    let mut groups = 0;
    while matches!(chars.get(end), Some('.' | '/' | '-'))
        && chars.get(end + 1).is_some_and(|c| c.is_numeric())
    {
        end = digits(end + 1);
        groups += 1;
    }
    (groups > 0).then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(query: &str, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        Highlight::parse(query)
            .marks(text)
            .into_iter()
            .map(|r| chars[r].iter().collect())
            .collect()
    }

    #[test]
    fn scan_cuts_as_the_index_pattern() {
        let re = regex::Regex::new(crate::body_tok::TOKEN_PATTERN).unwrap();
        let text = "Vu l'article L. 1234-5 du code ; arrêt n° 19-20.312, 2008/115/CE, \
                    3.000 euros, l'aide-soignante - salariée, 12-ab, 1.2.3., Œuvre, ½, x²";
        let ours: Vec<String> = scan(text)
            .into_iter()
            .map(|t| text.chars().skip(t.span.start).take(t.span.len()).collect())
            .collect();
        let theirs: Vec<String> = re.find_iter(text).map(|m| m.as_str().to_string()).collect();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn words_fold_and_stopwords_drop() {
        assert_eq!(
            marked(
                "licenciement du salarié",
                "Le Licenciement du salarie protégé"
            ),
            vec!["Licenciement", "salarie"]
        );
    }

    #[test]
    fn a_star_widens_the_word_it_ends() {
        assert_eq!(
            marked("licenci*", "licenciement, licenciée, licence"),
            vec!["licenciement", "licenciée"]
        );
    }

    #[test]
    fn a_phrase_is_one_mark_and_its_stopwords_take_any_word() {
        assert_eq!(
            marked(
                "\"rupture du contrat\"",
                "la rupture du contrat, la rupture abusive contrat, la rupture contrat"
            ),
            vec!["rupture du contrat", "rupture abusive contrat"]
        );
    }

    #[test]
    fn excluded_terms_are_not_marked() {
        assert_eq!(
            marked("congés SAUF maladie", "congés de maladie"),
            vec!["congés"]
        );
        assert_eq!(
            marked("congés -maladie", "congés de maladie"),
            vec!["congés"]
        );
        assert_eq!(
            marked(
                "bail SAUF (habitation OU rural) loyer",
                "bail rural, loyer d'habitation"
            ),
            vec!["bail", "loyer"]
        );
    }

    #[test]
    fn proximity_marks_only_the_words_found_close() {
        assert_eq!(
            marked(
                "bail PROCHE3 commercial",
                "le bail est commercial, puis il est question longtemps d'autre chose que \
                 du fonds commercial"
            ),
            vec!["bail", "commercial"]
        );
    }

    #[test]
    fn a_bare_word_holding_punctuation_is_a_phrase() {
        assert_eq!(
            marked("d'acte", "donner acte, d'acte"),
            vec!["acte", "acte"]
        );
        assert_eq!(
            marked("art.1240", "art. 1240 et article 1240"),
            vec!["art. 1240"]
        );
    }

    #[test]
    fn operators_alone_mark_nothing() {
        assert!(Highlight::parse("ET OU SAUF").is_empty());
        assert!(Highlight::parse("le la").is_empty());
    }
}
