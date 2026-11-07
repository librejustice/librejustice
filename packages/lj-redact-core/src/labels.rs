//! What replaces a named string, as the judicial open data writes it: each word
//! of a name becomes a letter, the same wherever the word recurs (`M. [B]`),
//! and the other families a numbered label (`[Adresse 1]`).

use std::cmp::Reverse;
use std::collections::HashMap;
use std::ops::Range;

use anyhow::{anyhow, bail, Result};

use crate::detect::{Kind, CIVILITIES};
use crate::fold::{fold_char_before, fold_query, Spaces};
use crate::index::Anchor;

/// The family the page files a string the user typed under.
pub const MANUAL: &str = "manual";

/// A string to replace, taken in the order letters and numbers are given in.
pub struct Needle<'a> {
    pub text: &'a str,
    /// The family the detector gave it, `None` for a string the user typed,
    /// which is labelled as a name.
    pub kind: Option<Kind>,
    /// The label the user set, which wins over the computed one. Empty erases.
    pub label: Option<&'a str>,
    /// A string an earlier pass replaced and the user no longer ticks: it
    /// keeps its letters and its number from every other string, and is
    /// neither searched nor labelled.
    pub held: bool,
}

impl<'a> Needle<'a> {
    /// `family` is [`MANUAL`] or a [`Kind::name`].
    pub fn filed(text: &'a str, family: &str, label: Option<&'a str>, held: bool) -> Result<Self> {
        let kind = match family {
            MANUAL => None,
            name => Some(Kind::named(name).ok_or_else(|| anyhow!("no family {name:?}"))?),
        };
        Ok(Self {
            text,
            kind,
            label,
            held,
        })
    }
}

/// Runs of a span that go, by character index, each with its replacement.
pub type Pieces = Vec<(Range<usize>, String)>;

/// The label of every needle, and what replaces each span it matches.
pub struct Labels {
    plans: Vec<Plan>,
    shown: Vec<String>,
}

enum Plan {
    /// The whole span becomes this text.
    Whole(String),
    /// Each word of the span becomes its letter.
    Words(Words),
}

/// A needle folded as the search folds it, spaces stripped, each folded
/// character tagged with the word it belongs to.
struct Words {
    folded: Vec<char>,
    tags: Vec<Tag>,
    /// The letter of each word, which [`Tag::Word`] indexes.
    letters: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tag {
    Clear,
    Word(usize),
}

impl Labels {
    /// Label `needles`. A needle the user labelled still takes its letters and
    /// its number, so editing one label never moves another, and so does a
    /// held one. Needles are then addressed by their rank among those not
    /// held.
    pub fn assign(needles: &[Needle]) -> Self {
        let mut letters: HashMap<String, usize> = HashMap::new();
        let mut numbers: HashMap<(&str, String), usize> = HashMap::new();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut plans = Vec::with_capacity(needles.len());
        let mut shown = Vec::with_capacity(needles.len());
        for needle in needles {
            let computed = match needle.kind.and_then(family) {
                Some(name) => {
                    let key = fold_query(needle.text, Spaces::Strip).into_iter().collect();
                    let number = *numbers.entry((name, key)).or_insert_with(|| {
                        let count = counts.entry(name).or_default();
                        *count += 1;
                        *count
                    });
                    Plan::Whole(format!("[{name} {number}]"))
                }
                None => Plan::Words(Words::split(needle.text, |key| {
                    let next = letters.len();
                    letter(*letters.entry(key).or_insert(next))
                })),
            };
            if needle.held {
                continue;
            }
            let plan = match needle.label {
                Some(label) => Plan::Whole(label.to_owned()),
                None => computed,
            };
            shown.push(plan.render(needle.text).expect("a needle folds to itself"));
            plans.push(plan);
        }
        Self { plans, shown }
    }

    /// The label of `needle`, as the list shows it: `M. [B]` for `M. Dupont`.
    pub fn label(&self, needle: usize) -> &str {
        &self.shown[needle]
    }

    /// What replaces `span`, the source characters an occurrence of `needle`
    /// covers. They fold to the needle but may differ from it: a name keeps the
    /// separators of the source, `Jean  Dupont` giving `[A]  [B]`. A control
    /// character of the span, a line break or a tab, stays after the label.
    pub fn render(&self, needle: usize, span: &str) -> Result<String> {
        self.plans[needle].render(span)
    }

    /// [`Self::render`] as the runs of `span` that give way, so an edit
    /// leaves what stays where it stands: each word of a name gives way to
    /// its letter, and a label takes the first run of the span, the runs
    /// after its line breaks going.
    pub fn pieces(&self, needle: usize, span: &str) -> Result<Pieces> {
        let chars: Vec<char> = span.chars().collect();
        self.plans[needle].pieces(&chars)
    }

    /// Each label with the text it stands for, `texts` being the needles not
    /// held, in order: a name's letter with its word as the needle writes it,
    /// particles included, and any other label with its whole needle. A label
    /// met again keeps the text it was first given.
    pub fn legend(&self, texts: &[String]) -> Result<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (plan, text) in self.plans.iter().zip(texts) {
            let pairs = match plan {
                Plan::Whole(label) => vec![(label.clone(), text.clone())],
                Plan::Words(words) => words.legend(text)?,
            };
            for (label, value) in pairs {
                if !label.is_empty() && !out.iter().any(|(known, _)| *known == label) {
                    out.push((label, value));
                }
            }
        }
        Ok(out)
    }
}

/// The text of `feed` with each occurrence of `needles`, the needles not held,
/// under its label. Occurrences go leftmost then longest first, and one that
/// overlaps an earlier one is left to it.
pub fn labelled(feed: &[(char, Anchor)], needles: &[String], labels: &Labels) -> Result<String> {
    let mut occurrences = crate::locate(feed, needles);
    occurrences.sort_by_key(|o| (o.source.start, Reverse(o.source.end)));
    let text = |range: Range<usize>| feed[range].iter().map(|(c, _)| *c).collect::<String>();
    let mut out = String::with_capacity(feed.len());
    let mut at = 0;
    for occurrence in occurrences {
        if occurrence.source.start < at {
            continue;
        }
        out.push_str(&text(at..occurrence.source.start));
        out.push_str(&labels.render(occurrence.needle, &text(occurrence.source.clone()))?);
        at = occurrence.source.end;
    }
    out.push_str(&text(at..feed.len()));
    Ok(out)
}

impl Plan {
    fn pieces(&self, chars: &[char]) -> Result<Pieces> {
        match self {
            Plan::Whole(text) => Ok(runs(chars.iter().map(|c| (!c.is_control()).then_some(())))
                .into_iter()
                .enumerate()
                .map(|(k, (range, ()))| (range, if k == 0 { text.clone() } else { String::new() }))
                .collect()),
            Plan::Words(words) => words.pieces(chars),
        }
    }

    fn render(&self, span: &str) -> Result<String> {
        let chars: Vec<char> = span.chars().collect();
        let mut out = String::with_capacity(span.len());
        let mut at = 0;
        for (range, text) in self.pieces(&chars)? {
            out.extend(&chars[at..range.start]);
            out.push_str(&text);
            at = range.end;
        }
        out.extend(&chars[at..]);
        Ok(out)
    }
}

/// Maximal runs of consecutive characters sharing a key, those without one
/// left out.
fn runs<K: PartialEq>(keys: impl IntoIterator<Item = Option<K>>) -> Vec<(Range<usize>, K)> {
    let mut out: Vec<(Range<usize>, K)> = Vec::new();
    for (at, key) in keys.into_iter().enumerate() {
        let Some(key) = key else { continue };
        match out.last_mut() {
            Some((range, last)) if range.end == at && *last == key => range.end += 1,
            _ => out.push((at..at + 1, key)),
        }
    }
    out
}

/// The name of a numbered family, `None` for a name.
fn family(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Person => None,
        Kind::Address => Some("Adresse"),
        Kind::Email => Some("Courriel"),
        Kind::Phone => Some("Téléphone"),
        Kind::Identifier => Some("Numéro identifiant"),
        Kind::Docket => Some("Numéro dossier"),
        Kind::BirthDate => Some("Date naissance"),
        Kind::BirthPlace => Some("Localité"),
    }
}

/// `A` to `Z`, then `AA`, `AB`.
fn letter(mut index: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    out.iter().rev().map(|b| char::from(*b)).collect()
}

/// Particles, folded, that make one word with the name word they lead to.
/// `d'` is elided onto its word, `d'Estaing` keyed as `estaing`.
const PARTICLES: [&str; 21] = [
    "de", "du", "des", "la", "le", "les", "van", "von", "der", "den", "di", "da", "del", "della",
    "dos", "das", "ben", "ibn", "bin", "el", "al",
];

/// Merge each run of particles into the name word that follows it, keyed on
/// that word: `de la fontaine` and `fontaine` share a key. A particle with no
/// name word after it stays a word. Returns the keys and, for each old word,
/// its new index.
fn join_particles(keys: Vec<String>) -> (Vec<String>, Vec<usize>) {
    let particle = |key: &str| PARTICLES.contains(&key);
    let mut joined = Vec::with_capacity(keys.len());
    let mut group = vec![0; keys.len()];
    let mut at = 0;
    while at < keys.len() {
        let head = (at..keys.len())
            .find(|&w| !particle(&keys[w]))
            .unwrap_or(at);
        for slot in &mut group[at..=head] {
            *slot = joined.len();
        }
        let key = &keys[head];
        joined.push(key.strip_prefix("d'").unwrap_or(key).to_owned());
        at = head + 1;
    }
    (joined, group)
}

impl Words {
    /// Cut `text` into words, each lettered by `letter_of` from its folded key,
    /// left to right.
    ///
    /// A word is a run of letters and digits, a hyphen or an apostrophe between
    /// two of them included: `Jean-Pierre` is one word. A civility opening the
    /// needle stays in clear unless nothing follows it, a run of particles
    /// joins the name word after it, and a needle without a letter or a digit
    /// is one word.
    fn split(text: &str, letter_of: impl FnMut(String) -> String) -> Words {
        let mut folded = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            fold_char_before(c, chars.peek().copied(), &mut folded);
        }
        let mut tags = vec![Tag::Clear; folded.len()];
        let mut count = 0;
        let mut open = false;
        for (at, c) in folded.iter().enumerate() {
            let joins = matches!(c, '-' | '\'')
                && open
                && folded.get(at + 1).is_some_and(|n| n.is_alphanumeric());
            if c.is_alphanumeric() || joins {
                if !open {
                    count += 1;
                    open = true;
                }
                tags[at] = Tag::Word(count - 1);
            } else {
                open = false;
            }
        }
        let (folded, mut tags): (Vec<char>, Vec<Tag>) = folded
            .into_iter()
            .zip(tags)
            .filter(|(c, _)| *c != ' ')
            .unzip();
        let mut keys = vec![String::new(); count];
        for (c, tag) in folded.iter().zip(&tags) {
            if let Tag::Word(w) = tag {
                keys[*w].push(*c);
            }
        }
        if count == 0 && !folded.is_empty() {
            tags = vec![Tag::Word(0); folded.len()];
            keys = vec![folded.iter().collect()];
        }
        let civilities = keys
            .iter()
            .take(keys.len().saturating_sub(1))
            .take_while(|key| CIVILITIES.contains(&key.as_str()))
            .count();
        if civilities > 0 {
            keys.drain(..civilities);
            for tag in &mut tags {
                if let Tag::Word(w) = *tag {
                    *tag = w.checked_sub(civilities).map_or(Tag::Clear, Tag::Word);
                }
            }
        }
        let (keys, group) = join_particles(keys);
        for tag in &mut tags {
            if let Tag::Word(w) = *tag {
                *tag = Tag::Word(group[w]);
            }
        }
        Words {
            folded,
            tags,
            letters: keys.into_iter().map(letter_of).collect(),
        }
    }

    /// Each word's label with the word as `text`, the needle itself, writes it.
    fn legend(&self, text: &str) -> Result<Vec<(String, String)>> {
        let chars: Vec<char> = text.chars().collect();
        let tags = self.tags(&chars)?;
        let mut out = Vec::with_capacity(self.letters.len());
        for (w, letter) in self.letters.iter().enumerate() {
            let Some(first) = tags.iter().position(|tag| *tag == Tag::Word(w)) else {
                continue;
            };
            let mut last = first;
            for (at, tag) in tags.iter().enumerate().skip(first) {
                match *tag {
                    Tag::Word(v) if v == w => last = at,
                    Tag::Word(_) => break,
                    Tag::Clear => {}
                }
            }
            out.push((format!("[{letter}]"), chars[first..=last].iter().collect()));
        }
        Ok(out)
    }

    fn pieces(&self, chars: &[char]) -> Result<Pieces> {
        let tags = self.tags(chars)?;
        let mut written = vec![false; self.letters.len()];
        Ok(runs(tags.iter().map(|tag| match *tag {
            Tag::Word(w) => Some(w),
            Tag::Clear => None,
        }))
        .into_iter()
        .map(|(range, w)| {
            let text = match std::mem::replace(&mut written[w], true) {
                false => format!("[{}]", self.letters[w]),
                true => String::new(),
            };
            (range, text)
        })
        .collect())
    }

    /// The tag of each character of `chars`, which fold to the needle.
    fn tags(&self, chars: &[char]) -> Result<Vec<Tag>> {
        let mut folded = Vec::new();
        let mut owners = Vec::new();
        let mut buffer = Vec::new();
        let mut blank = vec![false; chars.len()];
        for (at, c) in chars.iter().enumerate() {
            buffer.clear();
            fold_char_before(*c, chars.get(at + 1).copied(), &mut buffer);
            blank[at] = buffer == [' '];
            for f in buffer.iter().filter(|f| **f != ' ') {
                folded.push(*f);
                owners.push(at);
            }
        }
        let n = self.folded.len();
        // A match can start or end inside a ligature, whose other half folds
        // outside the needle.
        let Some(offset) = (0..=folded.len().saturating_sub(n))
            .find(|&at| folded.get(at..at + n) == Some(&self.folded[..]))
        else {
            let span: String = chars.iter().collect();
            bail!("{span:?} is not an occurrence of the needle");
        };
        let mut assigned: Vec<Option<Tag>> = vec![None; chars.len()];
        for (k, at) in owners[offset..offset + n].iter().enumerate() {
            assigned[*at].get_or_insert(self.tags[k]);
        }

        let mut resolved: Vec<Tag> = Vec::with_capacity(chars.len());
        for (at, c) in chars.iter().enumerate() {
            let before = resolved.last().copied();
            let tag = match assigned[at] {
                Some(tag) => tag,
                None if c.is_control() => Tag::Clear,
                None if blank[at] => {
                    let after = assigned[at + 1..].iter().flatten().next().copied();
                    match (before, after) {
                        (Some(Tag::Word(a)), Some(Tag::Word(b))) if a == b => Tag::Word(a),
                        _ => Tag::Clear,
                    }
                }
                None => before.unwrap_or(Tag::Clear),
            };
            resolved.push(tag);
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(kind: Option<Kind>, text: &str) -> Needle<'_> {
        Needle {
            text,
            kind,
            label: None,
            held: false,
        }
    }

    fn person(text: &str) -> Needle<'_> {
        of(Some(Kind::Person), text)
    }

    fn shown(needles: &[Needle]) -> Vec<String> {
        let labels = Labels::assign(needles);
        (0..needles.len())
            .map(|n| labels.label(n).to_string())
            .collect()
    }

    #[test]
    fn each_word_of_a_name_takes_a_letter_in_order_of_first_appearance() {
        let needles = [
            person("Jean Dupont"),
            person("Marie Martin"),
            person("Dupont"),
            person("Marie"),
        ];
        assert_eq!(shown(&needles), ["[A] [B]", "[C] [D]", "[B]", "[C]"]);
    }

    #[test]
    fn a_word_keeps_its_letter_whatever_its_case_and_accents() {
        let needles = [person("Éric DUPONT"), person("eric Dupont"), person("ÉRIC")];
        assert_eq!(shown(&needles), ["[A] [B]", "[A] [B]", "[A]"]);
    }

    #[test]
    fn a_civility_opening_a_name_stays_in_clear() {
        let needles = [
            person("Jean Dupont"),
            person("M. Dupont"),
            person("Me Nina LORVAL"),
            person("Madame Martin"),
            person("Maître"),
        ];
        assert_eq!(
            shown(&needles),
            ["[A] [B]", "M. [B]", "Me [C] [D]", "Madame [E]", "[F]"]
        );
    }

    #[test]
    fn a_particle_makes_one_word_with_the_name_word_it_leads_to() {
        let needles = [
            person("Jean de La Fontaine"),
            person("M. de La Fontaine"),
            person("La Fontaine"),
            person("Ludwig VAN DER Rohe"),
            person("Rohe"),
            person("Valéry d'Estaing"),
            person("Estaing"),
            person("Ali Ibn Sina"),
        ];
        assert_eq!(
            shown(&needles),
            ["[A] [B]", "M. [B]", "[B]", "[C] [D]", "[D]", "[E] [F]", "[F]", "[G] [H]"]
        );
    }

    #[test]
    fn a_particle_without_a_name_word_after_it_is_a_name_word() {
        let needles = [
            person("M. Le"),
            person("Le"),
            person("Jean de"),
            person("de La"),
        ];
        assert_eq!(shown(&needles), ["M. [A]", "[A]", "[B] [C]", "[C] [D]"]);
    }

    /// What a model reads of a document, and what `restore` gives back from it:
    /// a label put back stands for the text its needle first gave it.
    #[test]
    fn the_legend_puts_back_what_the_labels_replaced() {
        let needles = [
            person("M. Jean de La Fontaine"),
            of(Some(Kind::Address), "3 rue des Lilas"),
            person("FONTAINE"),
        ];
        let texts: Vec<String> = needles.iter().map(|n| n.text.to_string()).collect();
        let labels = Labels::assign(&needles);
        let source = "M. Jean de La Fontaine demeure 3 rue des Lilas. FONTAINE signe.";
        let hidden = labelled(&crate::tests::glyphs(source), &texts, &labels).unwrap();
        assert_eq!(hidden, "M. [A] [B] demeure [Adresse 1]. [B] signe.");

        let legend = labels.legend(&texts).unwrap();
        let pairs: Vec<(&str, &str)> = legend
            .iter()
            .map(|(label, value)| (label.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("[A]", "Jean"),
                ("[B]", "de La Fontaine"),
                ("[Adresse 1]", "3 rue des Lilas")
            ]
        );
        let back: Vec<Needle> = legend
            .iter()
            .map(|(label, value)| Needle {
                text: label,
                kind: None,
                label: Some(value),
                held: false,
            })
            .collect();
        let shown: Vec<String> = legend.iter().map(|(label, _)| label.clone()).collect();
        let restored = labelled(
            &crate::tests::glyphs(&hidden),
            &shown,
            &Labels::assign(&back),
        )
        .unwrap();
        assert_eq!(
            restored,
            "M. Jean de La Fontaine demeure 3 rue des Lilas. de La Fontaine signe."
        );
    }

    #[test]
    fn a_particle_group_keeps_the_source_separators_around_it() {
        let labels = Labels::assign(&[person("Jean de La Fontaine")]);
        let render = |span: &str| labels.render(0, span).unwrap();
        assert_eq!(render("Jean  de\u{a0}La Fontaine"), "[A]  [B]");
        assert_eq!(render("JEAN DE LA\nFONTAINE"), "[A] [B]\n");
    }

    #[test]
    fn a_comma_between_the_words_of_a_name_stays_between_their_letters() {
        let labels = Labels::assign(&[person("Pascal VERDIER")]);
        assert_eq!(labels.render(0, "Pascal, VERDIER").unwrap(), "[A], [B]");
    }

    #[test]
    fn a_hyphenated_first_name_is_one_word() {
        let needles = [
            person("Jean-Pierre Dupont"),
            person("Jean Dupont"),
            person("Jean\u{2013}Pierre"),
            person("Jean - Pierre"),
        ];
        assert_eq!(shown(&needles), ["[A] [B]", "[C] [B]", "[A]", "[C] - [D]"]);
    }

    #[test]
    fn a_string_the_user_typed_is_labelled_as_a_name() {
        let needles = [
            person("Jean Dupont"),
            of(None, "Dupont"),
            of(None, "Société Alpha"),
            of(None, "***"),
        ];
        assert_eq!(shown(&needles), ["[A] [B]", "[B]", "[C] [D]", "[E]"]);
    }

    #[test]
    fn after_z_comes_aa() {
        let expected = [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (27, "AB"),
            (51, "AZ"),
            (52, "BA"),
            (701, "ZZ"),
            (702, "AAA"),
        ];
        for (index, text) in expected {
            assert_eq!(letter(index), text);
        }
        let names: Vec<String> = (1..=27).map(|n| format!("N{n}")).collect();
        let needles: Vec<Needle> = names.iter().map(|n| of(None, n)).collect();
        assert_eq!(shown(&needles)[26], "[AA]");
    }

    #[test]
    fn other_families_are_numbered_per_family_and_the_same_text_keeps_its_number() {
        let needles = [
            of(Some(Kind::Address), "12 rue des Lilas 75011 Paris"),
            of(Some(Kind::Phone), "+33 1 23 45 67 89"),
            of(Some(Kind::Address), "3 place Carnot"),
            of(Some(Kind::Phone), "+33123456789"),
            of(Some(Kind::Address), "12 RUE DES LILAS 75011 PARIS"),
            of(Some(Kind::BirthPlace), "Tessalit"),
            of(Some(Kind::BirthDate), "1er juin 1985"),
            of(Some(Kind::Email), "c.dubois@exemple.fr"),
            of(Some(Kind::Identifier), "1 85 07 75 123 456 78"),
            of(Some(Kind::Docket), "21/01234"),
        ];
        assert_eq!(
            shown(&needles),
            [
                "[Adresse 1]",
                "[Téléphone 1]",
                "[Adresse 2]",
                "[Téléphone 1]",
                "[Adresse 1]",
                "[Localité 1]",
                "[Date naissance 1]",
                "[Courriel 1]",
                "[Numéro identifiant 1]",
                "[Numéro dossier 1]",
            ]
        );
    }

    #[test]
    fn a_label_the_user_set_replaces_the_computed_one_and_moves_no_other() {
        let needles = [
            Needle {
                label: Some("[Requérant]"),
                ..person("Jean Dupont")
            },
            person("Dupont"),
            Needle {
                label: Some(""),
                ..of(Some(Kind::Address), "3 place Carnot")
            },
            of(Some(Kind::Address), "12 rue des Lilas"),
        ];
        assert_eq!(shown(&needles), ["[Requérant]", "[B]", "", "[Adresse 2]"]);
        let labels = Labels::assign(&needles);
        assert_eq!(labels.render(0, "JEAN DUPONT").unwrap(), "[Requérant]");
        assert_eq!(labels.render(2, "3 place Carnot").unwrap(), "");
    }

    #[test]
    fn a_held_string_keeps_its_letters_and_its_number_from_the_others() {
        let held = |needle: Needle<'static>| Needle {
            held: true,
            ..needle
        };
        let needles = [
            held(person("Jean Dupont")),
            person("Paul Martin"),
            held(of(Some(Kind::Address), "3 place Carnot")),
            person("Jean Durand"),
            of(Some(Kind::Address), "12 rue des Lilas"),
        ];
        let labels = Labels::assign(&needles);
        let shown: Vec<&str> = (0..3).map(|n| labels.label(n)).collect();
        assert_eq!(shown, ["[C] [D]", "[A] [E]", "[Adresse 2]"]);
        assert_eq!(labels.render(1, "Jean Durand").unwrap(), "[A] [E]");
    }

    #[test]
    fn a_matched_span_keeps_its_separators_and_loses_its_words() {
        let labels = Labels::assign(&[
            person("Jean Dupont"),
            person("M. Dupont"),
            person("Dupont"),
            person("Léa"),
            person("Fin"),
        ]);
        let render = |needle: usize, span: &str| labels.render(needle, span).unwrap();
        assert_eq!(render(0, "Jean  Dupont"), "[A]  [B]");
        assert_eq!(render(0, "JEAN\u{a0}DUPONT"), "[A]\u{a0}[B]");
        assert_eq!(render(0, "JEANDUPONT"), "[A][B]");
        assert_eq!(render(0, "Jean\nDupont"), "[A]\n[B]");
        assert_eq!(render(1, "m.\u{a0}DUPONT"), "m.\u{a0}[B]");
        assert_eq!(render(2, "D U P O N T"), "[B]");
        assert_eq!(render(2, "DU\nPONT"), "[B]\n");
        assert_eq!(render(3, "LÉA"), "[C]");
        assert_eq!(render(3, "Le\u{301}a"), "[C]");
        assert_eq!(render(3, "Le\u{ad}a"), "[C]");
        assert_eq!(render(4, "\u{fb03}n"), "[D]");
    }

    #[test]
    fn a_span_that_does_not_fold_to_its_needle_is_refused() {
        let labels = Labels::assign(&[person("Jean Dupont")]);
        assert!(labels.render(0, "Marie Martin").is_err());
        assert!(labels.render(0, "Jean").is_err());
    }

    #[test]
    fn a_numbered_label_replaces_the_whole_span_and_keeps_its_line_breaks() {
        let labels = Labels::assign(&[of(Some(Kind::Address), "12 rue des Lilas 75011 Paris")]);
        assert_eq!(
            labels.render(0, "12 rue des Lilas\n75011 PARIS").unwrap(),
            "[Adresse 1]\n"
        );
    }
}
