//! What a court of another legal order cites in its own words: an article closed by the
//! abbreviation of its law (« Art. 8 Abs. 2 BV », « art. 95 let. a LTF », « art. 29 e 30
//! Cost. », « artikel 1675/7 Ger.W. »), and the numbers of the courts it cites
//! (« 4A_123/2020 », « ATF 140 III 86 », « C.19.0123.F »).

use std::collections::HashMap;
use std::sync::LazyLock;

use jiff::civil::Date;
use lj_core::address::CaseAddress;
use regex::Regex;

use crate::compiled::{CompiledCase, CompiledCitation};
use crate::extract::normalize_article;
use crate::link::{LinkSnapshot, LinkTarget};

/// An article numeral of an enumeration, and the abbreviation the enumeration closes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignArticle {
    pub s: usize,
    pub e: usize,
    pub surface: String,
    pub num_key: String,
    pub sigle: Option<Sigle>,
    /// Where a law's Dutch name may begin, after « van de ».
    pub law_at: Option<usize>,
    /// Its article word opens a line, as an annex sets an article under its law's name.
    pub heads_line: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sigle {
    pub s: usize,
    pub e: usize,
    pub text: String,
    /// The law's name it is defined after, in brackets: « de la loi sur le Tribunal fédéral (LTF) ».
    pub defined: Option<String>,
}

/// The source text with its byte offsets mapped to char offsets.
struct Text {
    text: String,
    byte2char: Vec<usize>,
}

impl Text {
    fn new(chars: &[char]) -> Self {
        let text: String = chars.iter().collect();
        let mut byte2char = Vec::with_capacity(text.len() + 1);
        for (ci, c) in chars.iter().enumerate() {
            byte2char.extend(std::iter::repeat_n(ci, c.len_utf8()));
        }
        byte2char.push(chars.len());
        Text { text, byte2char }
    }
}

static RE_ART_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:[Aa]rt(?:ikelen|ikel|icolo|icoli|icles|icle|t)?\b\.?|ART\.)|§§?").unwrap()
});
static RE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(\d{1,5}(?:er|re)?(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies|[a-z](?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies)?)?(?:[./:]\d{1,4}[a-z]?)*)",
    )
    .unwrap()
});
/// A latin ordinal written apart from its numeral, « 12 bis ».
static RE_SPACED_ORDINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ (?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|novies|decies)\b")
        .unwrap()
});
/// What subdivides an article between its numeral and its law: paragraph, letter, figure,
/// sentence, the « and following ».
static RE_QUALIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*,?\s*(?:
            (?:Abs|Absatz|UAbs|Unterabs|al|alin[ée]a|alin|cpv|capoverso|cap|let|lit|lett|Bst|Buchst|Buchstabe|ch|chiffre|Ziff|Ziffer|cifra|n|num|Nr|par|para|paragraphe|Satz|phrase|frase|lid|leden|punt|sub|onder|onderdeel)\.?
              \s*(?:\d{1,3}[a-z]?°?|[a-z]\)?|[ivxIVX]{1,4}\b|bis|ter)
              (?:\s*(?:(?:et|und|e|en|bis)\s+|[-\x{2013}]\s*)(?:\d[a-z]?\b°?|[a-z]\)?(?:\s|,|$)))*
          | (?:eerste|tweede|derde|vierde|vijfde|zesde|zevende|achtste|negende|tiende|elfde|twaalfde)
              (?:\s*(?:,|en|tot\s+en\s+met)\s*(?:eerste|tweede|derde|vierde|vijfde|zesde|zevende|achtste|negende|tiende|elfde|twaalfde))*
              \s+(?:lid|leden|volzin|zin)\b
          | aanhef\b
          | en\s+(?:onder|sub)\s+(?:\d{1,3}[a-z]?°|\d{1,3}[a-z]?\b|[a-z]\b)
          | §\s*\d{1,3}(?:er|bis|ter|quater)?\b
          | (?:ss|ff|f|segg|sv|s)\.?(?:\s|,|$)
          | et\s+s(?:s|uivants?)\.?(?:\s|,|$)
          | \(\s*[^()]{1,20}\)
        )",
    )
    .unwrap()
});
/// What joins two numerals of one enumeration, up to the second's first digit.
static RE_ENUM_SEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:,\s*(?:(?:und|e|et|en)\s+)?|und|u\.|e|ed|et|en|ou|oder|o|bis|à|\x{2013}|-)\s*\d",
    )
    .unwrap()
});
/// A genitive after the article: its law follows, by name or by abbreviation.
static RE_GENITIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*,?\s*(?:des|der|dem|del|della|dello|dell['’]|degli|delle|de\s+la|de\s+l['’]|du|de|d['’]|van\s+de|van\s+het|van|of\s+the|of)\b",
    )
    .unwrap()
});
/// A law named in words, then its abbreviation defined in brackets: « der Strafprozessordnung
/// [StPO; SR 312.0] », « de la loi sur le Tribunal fédéral (LTF) ».
static RE_DEFINED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*((?:[\p{L}'-]+\s+){0,8}?[\p{L}'-]+)\s*[\[(]\s*(?:(?:hierna|verder)\s*:\s*(?:de\s+|het\s+)?)?([an]?[A-ZÄÖÜ][A-Za-zÄÖÜäöü]{1,11})\s*[;,)\]]")
        .unwrap()
});
static RE_SIGLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*,?\s*((?:oud|nieuw)\s+BW|Ger\.\s?W\.|W\.\s?Venn\.|C\.\s?(?:civ|jud|pén|pen|com|soc|[iI]\.\s?[cC]r)\.|[A-Z](?:\.[A-Z]){1,5}\.?|[an]?[A-ZÄÖÜ][A-Za-zÄÖÜäöüéè]{0,11}(?:[-/][A-Z]{1,4})?\.?)",
    )
    .unwrap()
});
/// Uppercase words that close an article numeral without naming a law.
const NOT_SIGLES: &[&str] = &[
    "SR", "RS", "RSV", "BBl", "FF", "AS", "RO", "CHF", "EUR", "BGE", "ATF", "DTF", "AG", "SA",
    "GmbH", "KG", "SàRL", "Sàrl", "LU", "VD", "GE", "ZH", "BE", "TI", "OK",
];

/// The article enumerations of a text, each numeral with the abbreviation that closes its
/// enumeration when one does, `known` the words the court's place carries as abbreviations.
pub(crate) fn foreign_articles(
    chars: &[char],
    breaks: &[bool],
    known: &dyn Fn(&str) -> bool,
) -> Vec<ForeignArticle> {
    let t = Text::new(chars);
    let text = t.text.as_str();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(m) = RE_ART_WORD.find_at(text, from) {
        from = m.end();
        let heads_line = t.byte2char[m.start()]
            .checked_sub(1)
            .is_none_or(|i| breaks[i]);
        let mut pos = m.end();
        let mut group: Vec<ForeignArticle> = Vec::new();
        while let Some(c) = RE_NUM.captures(&text[pos..]) {
            let g = c.get(1).expect("numeral");
            let (mut bs, mut be) = (pos + g.start(), pos + g.end());
            if let Some(o) = RE_SPACED_ORDINAL.find(&text[be..]) {
                if !text[be + o.end()..]
                    .trim_start()
                    .starts_with(|c: char| c.is_ascii_digit())
                {
                    be += o.end();
                }
            }
            if text[be..].starts_with(|c: char| c.is_alphanumeric()) {
                break;
            }
            while text[bs..be].starts_with(' ') {
                bs += 1;
            }
            let surface = text[bs..be].to_string();
            let num_key = normalize_article(&surface);
            if num_key.is_empty() {
                break;
            }
            group.push(ForeignArticle {
                s: t.byte2char[bs],
                e: t.byte2char[be],
                surface,
                num_key,
                sigle: None,
                law_at: None,
                heads_line,
            });
            pos = be;
            while let Some(q) = RE_QUALIFIER.find(&text[pos..]) {
                if q.end() == 0 {
                    break;
                }
                pos += q.end();
                while text[..pos].ends_with([' ', ',']) {
                    pos -= 1;
                }
            }
            match RE_ENUM_SEP.find(&text[pos..]) {
                Some(sep) => pos += sep.end() - 1,
                None => break,
            }
        }
        if group.is_empty() {
            continue;
        }
        from = from.max(pos);
        let genitive = RE_GENITIVE.find(&text[pos..]);
        let after = genitive.map_or(pos, |g| pos + g.end());
        if genitive.is_some_and(|g| {
            g.as_str()
                .trim_start_matches(|c: char| c == ',' || c.is_whitespace())
                .starts_with("van")
        }) {
            for a in &mut group {
                a.law_at = Some(t.byte2char[after]);
            }
        }
        let found = sigle_at(text, after, known).map(|f| (f, None)).or_else(|| {
            (after > pos)
                .then(|| RE_DEFINED.captures(&text[after..]))
                .flatten()
                .and_then(|c| {
                    let g = c.get(2).expect("sigle");
                    sigle_at(text, after + g.start(), &|w| {
                        !NOT_SIGLES.contains(&w.trim_end_matches('.'))
                    })
                    .map(|f| (f, Some(c[1].to_string())))
                })
        });
        if let Some(((ss, se), defined)) = found {
            let sigle = Sigle {
                s: t.byte2char[ss],
                e: t.byte2char[se],
                text: text[ss..se].to_string(),
                defined,
            };
            for a in &mut group {
                a.sigle = Some(sigle.clone());
            }
            from = from.max(se);
        }
        out.extend(group);
    }
    out
}

/// The abbreviation starting at `pos`, a sentence's closing dot left out: one the place
/// carries (`Sr`, the Dutch `RO`), else two capitals at least or a word its dot shortens
/// (`Cst.`, `Sw.`), never a roman numeral.
fn sigle_at(text: &str, pos: usize, known: &dyn Fn(&str) -> bool) -> Option<(usize, usize)> {
    let c = RE_SIGLE.captures(&text[pos..])?;
    let g = c.get(1).expect("sigle");
    let (ss, mut se) = (pos + g.start(), pos + g.end());
    if let Some(word) = text[ss..se].strip_suffix('.') {
        if word.ends_with(char::is_uppercase) && !word.contains('.') {
            se -= 1;
        }
    }
    let raw = &text[ss..se];
    let word = raw.trim_end_matches('.');
    let roman = word.chars().all(|c| matches!(c, 'I' | 'V' | 'X'));
    (!text[se..].starts_with(|c: char| c.is_alphanumeric())
        && (known(raw)
            || (!NOT_SIGLES.contains(&word)
                && !roman
                && (reads_as_sigle(raw) || raw.ends_with('.')))))
    .then_some((ss, se))
}

/// Whether an unresolved word reads as an abbreviation rather than as prose: two capitals
/// at least.
fn reads_as_sigle(sigle: &str) -> bool {
    sigle.chars().filter(|c| c.is_uppercase()).count() >= 2
}

/// Reads the articles of a court at `place` in its own words over what the composer found:
/// an article its abbreviation closes takes the text the abbreviation names at that place,
/// or none; the abbreviation takes no instrument line of its own. Then every target the
/// court does not apply is dropped, unless the citation names that text's country, and a
/// text that replaced another under its name is read by the decision's `date`.
pub(crate) fn apply(
    out: &mut Vec<CompiledCitation>,
    chars: &[char],
    breaks: &[bool],
    snap: &LinkSnapshot,
    place: &str,
    date: Option<Date>,
) {
    let known = |w: &str| snap.carries_abbreviation(place, w);
    let articles = foreign_articles(chars, breaks, &known);
    let linked_at = |out: &[CompiledCitation], a: &ForeignArticle| {
        out.iter()
            .find(|c| c.char_start == a.s && c.char_end == a.e)
            .and_then(|c| c.target.ref_text_uid.clone())
    };
    // An abbreviation a Dutch decision defines names the law it follows: that name's text,
    // else the place's own unless its title names another law (« (Iw) » after the Instellingswet).
    let mut defined: HashMap<String, Option<String>> = HashMap::new();
    for a in &articles {
        let Some(s) = &a.sigle else { continue };
        let (Some(name), Some(at)) = (&s.defined, a.law_at) else {
            continue;
        };
        if defined.contains_key(s.text.trim_end_matches('.')) {
            continue;
        }
        let uid = law_named_at(chars, at, snap, place, &a.num_key)
            .map(|(uid, _)| uid.to_string())
            .or_else(|| linked_at(out, a))
            .or_else(|| {
                snap.place_abbreviation(place, &s.text, Some(&a.num_key))
                    .filter(|uid| !snap.names_another_law(uid, name))
                    .map(str::to_string)
            });
        defined.insert(s.text.trim_end_matches('.').to_string(), uid);
    }
    let mut heads: Vec<usize> = articles
        .iter()
        .filter(|a| a.heads_line)
        .map(|a| line_start(breaks, a.s))
        .collect();
    heads.dedup();
    for a in articles {
        // « 6:162 BW » is article 162 of book 6, a text of its own abbreviated `BW6`.
        let in_book = a.surface.split_once(':').and_then(|(book, n)| {
            let s = a.sigle.as_ref()?;
            let key = normalize_article(n);
            snap.place_abbreviation(place, &format!("{}{book}", s.text), Some(&key))
                .map(|uid| (uid, key))
        });
        let num_key = in_book.as_ref().map_or(&a.num_key, |(_, key)| key);
        let alias = a.sigle.as_ref().and_then(|s| {
            in_book
                .as_ref()
                .map(|(uid, _)| *uid)
                .or_else(|| snap.place_abbreviation(place, &s.text, Some(&a.num_key)))
        });
        let by_sigle = a.sigle.as_ref().map(|s| {
            let uid = match defined.get(s.text.trim_end_matches('.')) {
                Some(uid) if in_book.is_none() => uid.as_deref(),
                _ => alias,
            };
            (uid, s.clone())
        });
        let composed = out
            .iter()
            .any(|c| c.char_start == a.s && c.char_end == a.e && c.target.ref_text_uid.is_some());
        let (uid, sigle) = match by_sigle {
            Some((Some(uid), s)) if s.defined.is_none() => (Some(uid), s),
            by_sigle => match a
                .law_at
                .filter(|_| !composed)
                .and_then(|at| law_named_at(chars, at, snap, place, &a.num_key))
            {
                Some((uid, name)) => (Some(uid), name),
                None => match by_sigle.filter(|(uid, s)| {
                    uid.is_some()
                        || reads_as_sigle(&s.text)
                        || defined.contains_key(s.text.trim_end_matches('.'))
                }) {
                    Some(found) => found,
                    None => match a
                        .heads_line
                        .then(|| annex_law(a.s, &heads, chars, breaks, snap, place))
                        .flatten()
                    {
                        Some((uid, name)) => (Some(uid), name),
                        None => continue,
                    },
                },
            },
        };
        let under_heading = sigle.e < a.s;
        let sigle = &sigle;
        if uid.is_none()
            && !reads_as_sigle(&sigle.text)
            && !defined.contains_key(sigle.text.trim_end_matches('.'))
        {
            continue;
        }
        // An unknown abbreviation defined after the law's name leaves the composer's link to the name.
        if alias.is_none() && sigle.defined.is_some() && composed {
            continue;
        }
        // The composer read this numeral with other bounds (« 16-3 » where this reads « 16 »):
        // its reading stands, since the spans of a document never overlap.
        if out.iter().any(|c| {
            c.char_start < a.e
                && a.s < c.char_end
                && !(c.article.is_some() && c.char_start == a.s && c.char_end == a.e)
        }) {
            continue;
        }
        let key = sigle.text.trim_end_matches('.').to_string();
        if !under_heading {
            out.retain(|c| c.article.is_some() || c.char_end <= sigle.s || c.char_start >= sigle.e);
        }
        let existing = out
            .iter_mut()
            .find(|c| c.article.is_some() && c.char_start == a.s && c.char_end == a.e);
        let target = match uid {
            Some(uid) => LinkTarget {
                ref_text_uid: Some(uid.to_string()),
                ref_num_key: snap.num_key_for(uid, Some(num_key)),
            },
            None => LinkTarget::default(),
        };
        match existing {
            Some(c) => {
                if uid.is_some()
                    || c.instrument != sigle.text
                    || defined.contains_key(sigle.text.trim_end_matches('.'))
                {
                    c.target = target;
                }
                c.instrument = sigle.text.clone();
                c.text_key = key;
                c.bare = false;
            }
            None => out.push(CompiledCitation {
                char_start: a.s,
                char_end: a.e,
                instrument: sigle.text.clone(),
                article: Some(a.surface.clone()),
                text_key: key,
                article_key: Some(a.num_key.clone()),
                target,
                suivants: false,
                bare: false,
            }),
        }
    }
    for c in out.iter_mut() {
        let Some(uid) = c.target.ref_text_uid.as_deref() else {
            continue;
        };
        if snap.applies_at(place, uid) {
            continue;
        }
        let named = crate::link::KeyAnalysis::new(&c.text_key)
            .signals()
            .jurisdiction
            .is_some_and(|j| {
                snap.foreign_territory(uid)
                    .is_some_and(|t| t.split('-').next() == Some(j))
            });
        if !named {
            c.target = LinkTarget::default();
        }
    }
    for c in out.iter_mut() {
        let Some(uid) = c.target.ref_text_uid.as_deref() else {
            continue;
        };
        if names_version(&c.instrument) {
            continue;
        }
        c.target = match lj_norm::text_successions::version_on(uid, date) {
            Some(read) if read == uid => continue,
            Some(read) if snap.foreign_territory(read).is_some() => LinkTarget {
                ref_text_uid: Some(read.to_string()),
                ref_num_key: snap.num_key_for(read, c.article_key.as_deref()),
            },
            _ => LinkTarget::default(),
        };
    }
}

/// The law named in Dutch words from `at`, its first word capitalised: the longest run of
/// words that is a title the place carries. The composer reads the names in other languages.
fn law_named_at<'s>(
    chars: &[char],
    at: usize,
    snap: &'s LinkSnapshot,
    place: &str,
    article_key: &str,
) -> Option<(&'s str, Sigle)> {
    let mut i = at;
    while chars.get(i) == Some(&' ') {
        i += 1;
    }
    if !chars.get(i).is_some_and(|c| c.is_uppercase()) {
        return None;
    }
    let start = i;
    let mut ends = Vec::new();
    while ends.len() < 16 {
        let w = i;
        while chars
            .get(i)
            .is_some_and(|c| c.is_alphanumeric() || *c == '-')
        {
            i += 1;
        }
        if i == w {
            break;
        }
        ends.push(i);
        if chars.get(i) != Some(&' ') {
            break;
        }
        i += 1;
    }
    ends.into_iter().rev().find_map(|e| {
        let text: String = chars[start..e].iter().collect();
        snap.place_title(place, &text, Some(article_key))
            .map(|uid| {
                (
                    uid,
                    Sigle {
                        s: start,
                        e,
                        text,
                        defined: None,
                    },
                )
            })
    })
}

fn line_start(breaks: &[bool], mut p: usize) -> usize {
    while p > 0 && !breaks[p - 1] {
        p -= 1;
    }
    p
}

/// The law an annex sets the article heading at `at` under: the nearest line above an article
/// heading that closes on no punctuation, when that line is a law's name in full, an
/// abbreviation in brackets or « e.v. » aside.
fn annex_law<'s>(
    at: usize,
    heads: &[usize],
    chars: &[char],
    breaks: &[bool],
    snap: &'s LinkSnapshot,
    place: &str,
) -> Option<(&'s str, Sigle)> {
    let here = line_start(breaks, at);
    for &head in heads.iter().rev().filter(|&&h| h <= here) {
        if here - head > 5000 || head == 0 {
            return None;
        }
        let mut end = head - 1;
        let mut above = line_start(breaks, end);
        while above > 0 && chars[above..end].iter().all(|c| *c == ' ') {
            end = above - 1;
            above = line_start(breaks, end);
        }
        if heads.binary_search(&above).is_ok() || end - above > 120 {
            continue;
        }
        let line: String = chars[above..end].iter().collect();
        let name = line.split(" (").next().unwrap_or("").trim_end();
        let name = name.strip_suffix(" e.v.").unwrap_or(name);
        if name.ends_with(['.', ';', ':', ',']) {
            continue;
        }
        let name_end = above + name.chars().count();
        return law_named_at(chars, above, snap, place, "").filter(|(_, law)| law.e == name_end);
    }
    None
}

/// « révisé », or a year: the citation names which of two texts sharing a name it reads.
fn names_version(instrument: &str) -> bool {
    let folded = lj_fold::fold(instrument);
    folded.contains("revis")
        || folded
            .split(|c: char| !c.is_ascii_digit())
            .any(|n| n.len() == 4)
}

static RE_CH_DOCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2}[A-H]_\d{1,4}/\d{4}|\d[A-Z]{1,2}\.\d{1,4}/\d{4})\b").unwrap()
});
const REPORT: &str = r"(\d{1,3})\s+(Ia|Ib|IV|V|III|II|I)\s+(\d{1,4})";
static RE_CH_REPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\b(?:ATF|BGE|DTF)\s+({REPORT})\b")).unwrap());
/// The consideration and the page that may follow a report before the next one of its list.
const REPORT_TAIL: &str = r"(?:\s*(?:E\.|consid\.|c\.|Erw\.)\s*[\d.a-z/]+(?:\s+(?:und|et|e)\s+[\d.a-z/]+)*)?(?:\s*(?:S\.|p\.)\s*\d+(?:\s*f+\.?|\s*s+\.?)?)?\s*[;,]\s*(?:(?:und|et|e)\s+)?";
/// The next report of a list, written whole.
static RE_CH_REPORT_NEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{REPORT_TAIL}({REPORT})\b")).unwrap());
/// The next report of a list in the same volume, written by its page alone, « 86 E. 2, 115 E. 2 ».
static RE_CH_PAGE_NEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^{REPORT_TAIL}(\d{{1,4}})\s+(?:E\.|consid\.|c\.|Erw\.)"
    ))
    .unwrap()
});
static RE_CH_FAC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-F]-\d{1,5}/\d{4})\b|\b(?:BVGE|ATAF|DTAF)\s+(\d{4}/\d{1,3})\b").unwrap()
});
static RE_CH_FPC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b((?:BB|BG|BH|BE|BP|CA|CN|CR|RR|SK)\.\d{4}\.\d{1,4})\b").unwrap()
});
static RE_BE_CASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]\.\d{2}\.\d{4}\.[FNDE])\b").unwrap());
static RE_ECLI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bECLI:([A-Z]{2}):([A-Z]+):\d{4}:[0-9A-Z]+\b").unwrap());

/// The case numbers a court at `place` writes in the forms of its own legal order.
pub(crate) fn foreign_cases(chars: &[char], place: &str) -> Vec<CompiledCase> {
    let t = Text::new(chars);
    let text = t.text.as_str();
    let mut out = Vec::new();
    let mut push = |bs: usize, be: usize, court: &str, docket: &str| {
        out.push(CompiledCase {
            char_start: t.byte2char[bs],
            char_end: t.byte2char[be],
            address: CaseAddress::new(court, docket, ""),
        });
    };
    match place.split('-').next().unwrap_or(place) {
        "CH" => {
            for c in RE_CH_DOCKET.captures_iter(text) {
                let g = c.get(1).expect("docket");
                push(g.start(), g.end(), "ch_cour_supreme", g.as_str());
            }
            for c in RE_CH_REPORT.captures_iter(text) {
                let g = c.get(1).expect("report");
                let (volume, section) = (&c[2], &c[3]);
                push(g.start(), g.end(), "ch_cour_supreme", g.as_str());
                let mut end = g.end();
                loop {
                    if let Some(n) = RE_CH_REPORT_NEXT.captures(&text[end..]) {
                        let g = n.get(1).expect("report");
                        push(
                            end + g.start(),
                            end + g.end(),
                            "ch_cour_supreme",
                            g.as_str(),
                        );
                        end += g.end();
                    } else if let Some(n) = RE_CH_PAGE_NEXT.captures(&text[end..]) {
                        let g = n.get(1).expect("page");
                        let docket = format!("{volume} {section} {}", g.as_str());
                        push(end + g.start(), end + g.end(), "ch_cour_supreme", &docket);
                        end += g.end();
                    } else {
                        break;
                    }
                }
            }
            for c in RE_CH_FAC.captures_iter(text) {
                let g = c.get(1).or_else(|| c.get(2)).expect("docket");
                push(
                    g.start(),
                    g.end(),
                    "ch_tribunal_administratif_federal",
                    g.as_str(),
                );
            }
            for c in RE_CH_FPC.captures_iter(text) {
                let g = c.get(1).expect("docket");
                push(
                    g.start(),
                    g.end(),
                    "ch_tribunal_premiere_instance_penal_federal",
                    g.as_str(),
                );
            }
        }
        "BE" => {
            for c in RE_BE_CASS.captures_iter(text) {
                let g = c.get(1).expect("role number");
                push(g.start(), g.end(), "be_cour_cassation", g.as_str());
            }
        }
        "NL" => {
            for c in RE_ECLI.captures_iter(text) {
                let g = c.get(0).expect("ecli");
                let court = match (&c[1], &c[2]) {
                    ("NL", court) => lj_parse::dutch_court(court).map_or("", |d| d.code),
                    ("EU", "C") => "cjue",
                    ("CE", "ECHR") => "cedh",
                    _ => "",
                };
                push(g.start(), g.end(), court, g.as_str());
            }
        }
        _ => {}
    }
    out.sort_by_key(|c| c.char_start);
    let mut end = 0;
    out.retain(|c| {
        let apart = c.char_start >= end;
        if apart {
            end = c.char_end;
        }
        apart
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Vec<(String, Option<String>)> {
        let chars: Vec<char> = text.chars().collect();
        let breaks: Vec<bool> = chars.iter().map(|c| *c == '\n').collect();
        foreign_articles(&chars, &breaks, &|w| matches!(w, "Vw" | "Sr" | "RO"))
            .into_iter()
            .map(|a| (a.surface, a.sigle.map(|s| s.text)))
            .collect()
    }

    fn pairs(items: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        items
            .iter()
            .map(|(n, s)| (n.to_string(), s.map(str::to_string)))
            .collect()
    }

    #[test]
    fn an_enumeration_takes_the_abbreviation_that_closes_it() {
        assert_eq!(
            read("gemäss Art. 29 Abs. 2 und 30 BV sowie Art. 95 lit. a BGG."),
            pairs(&[("29", Some("BV")), ("30", Some("BV")), ("95", Some("BGG"))])
        );
        assert_eq!(
            read("(art. 8 al. 1 let. b et 12bis CC; art. 90 ss LTF)"),
            pairs(&[
                ("8", Some("CC")),
                ("12bis", Some("CC")),
                ("90", Some("LTF"))
            ])
        );
        assert_eq!(
            read("ai sensi degli artt. 29 e 30 Cost."),
            pairs(&[("29", Some("Cost.")), ("30", Some("Cost."))])
        );
        assert_eq!(
            read("artikel 1675/7 Ger.W. en art. 6.1 EVRM"),
            pairs(&[("1675/7", Some("Ger.W.")), ("6.1", Some("EVRM"))])
        );
    }

    #[test]
    fn a_dutch_article_reads_its_book_and_its_ordinal_paragraphs() {
        assert_eq!(
            read("de artikelen 7, eerste lid, en 12w, eerste en vierde lid, van de IW"),
            pairs(&[("7", Some("IW")), ("12w", Some("IW"))])
        );
        assert_eq!(
            read("artikel 141 lid 1 en 2, aanhef en onder 2°, van het WvS"),
            pairs(&[("141", Some("WvS"))])
        );
        assert_eq!(
            read("artikel 8:29, derde lid, AWB"),
            pairs(&[("8:29", Some("AWB"))])
        );
    }

    #[test]
    fn a_dutch_abbreviation_is_one_the_place_carries_or_one_the_decision_defines() {
        assert_eq!(
            read("artikel 59, eerste lid, aanhef en onder a, van de Vw"),
            pairs(&[("59", Some("Vw"))])
        );
        assert_eq!(
            read("artikel 8:29 van de Algemene wet bestuursrecht (hierna: Awb)"),
            pairs(&[("8:29", Some("Awb"))])
        );
        assert_eq!(read("artikel 4 van de Overeenkomst"), pairs(&[("4", None)]));
        assert_eq!(read("artikel 81 RO"), pairs(&[("81", Some("RO"))]));
    }

    #[test]
    fn a_genitive_names_the_law_or_defines_its_abbreviation() {
        assert_eq!(
            read("Art. 396 Abs. 1 der Strafprozessordnung [StPO; SR 312.0]"),
            pairs(&[("396", Some("StPO"))])
        );
        assert_eq!(read("l'article 96 du COCC"), pairs(&[("96", Some("COCC"))]));
        assert_eq!(
            read("l'article 1382 du Code civil"),
            pairs(&[("1382", None)])
        );
    }

    #[test]
    fn a_paragraph_after_its_article_qualifies_it() {
        assert_eq!(
            read("des articles 84, § 1er et 127 du CWATUP"),
            pairs(&[("84", Some("CWATUP")), ("127", Some("CWATUP"))])
        );
        assert_eq!(
            read("l'article 6, § 1er, VI, alinéa 3, de la loi spéciale"),
            pairs(&[("6", None)])
        );
    }

    #[test]
    fn a_collection_number_closes_no_article() {
        assert_eq!(read("Art. 8 SR 101"), pairs(&[("8", None)]));
    }

    fn cases(text: &str, place: &str) -> Vec<(String, String)> {
        let chars: Vec<char> = text.chars().collect();
        foreign_cases(&chars, place)
            .into_iter()
            .map(|c| {
                (
                    chars[c.char_start..c.char_end].iter().collect(),
                    format!(
                        "{}|{}",
                        c.address.court.unwrap_or_default(),
                        c.address.docket.unwrap_or_default()
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn a_swiss_report_list_keeps_its_volume_across_pages() {
        assert_eq!(
            cases("(BGE 140 III 86 E. 2, 115 E. 2; 133 IV 286 E. 1.4)", "CH"),
            vec![
                ("140 III 86".into(), "ch_cour_supreme|140III86".into()),
                ("115".into(), "ch_cour_supreme|140III115".into()),
                ("133 IV 286".into(), "ch_cour_supreme|133IV286".into()),
            ]
        );
    }

    #[test]
    fn a_dutch_court_cites_by_ecli() {
        assert_eq!(
            cases(
                "HR 16 november 2004, ECLI:NL:HR:2004:AR3230; PHR, ECLI:NL:PHR:2026:729 en HvJ EU, ECLI:EU:C:2022:858.",
                "NL"
            ),
            vec![
                ("ECLI:NL:HR:2004:AR3230".into(), "nl_cour_supreme|ECLI:NL:HR:2004:AR3230".into()),
                ("ECLI:NL:PHR:2026:729".into(), "|ECLI:NL:PHR:2026:729".into()),
                ("ECLI:EU:C:2022:858".into(), "cjue|ECLI:EU:C:2022:858".into()),
            ]
        );
    }

    #[test]
    fn swiss_dockets_name_their_court() {
        assert_eq!(
            cases(
                "arrêt 6B_396/2008 du 25 août 2008 et arrêt C-1234/2020",
                "CH-VD"
            ),
            vec![
                ("6B_396/2008".into(), "ch_cour_supreme|6B_396/2008".into()),
                (
                    "C-1234/2020".into(),
                    "ch_tribunal_administratif_federal|C-1234/2020".into()
                ),
            ]
        );
        assert!(cases("arrêt 6B_396/2008", "BE").is_empty());
    }
}
