//! Djibouti's electronic Journal officiel through its WordPress API: each text's nature, reference
//! and issue, its numbered articles, and the act its title says it amends or repeals.

use std::cmp::{Ordering, Reverse};
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::ConsolidatedArticle;

/// A text as the API lists it, its content present when it was asked for.
#[derive(Debug, Clone)]
pub struct EjoText {
    pub id: u64,
    /// The text's own day, `YYYY-MM-DD`.
    pub day: Option<String>,
    /// The day the site last changed it, `YYYY-MM-DD`.
    pub modified: Option<String>,
    pub title: String,
    pub reference: Option<String>,
    pub natures: Vec<u64>,
    pub institutions: Vec<u64>,
    pub issue: Option<u64>,
    pub link: Option<String>,
    pub content: Option<String>,
    /// The visas and the signatures the site files apart from the content, as text.
    pub visas: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EjoIssue {
    pub id: u64,
    pub number: Option<String>,
    pub day: Option<String>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EjoNature {
    Loi,
    LoiOrganique,
    Ordonnance,
    Decret,
    Arrete,
    Decision,
    Deliberation,
    Circulaire,
    Accord,
    Avis,
    Instruction,
    Resolution,
    Rectificatif,
    /// A rapport, a proclamation, a communiqué: no numbering of its own.
    Other,
}

/// What a title says its text does to another act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EjoEffect {
    pub verb: &'static str,
    pub nature: EjoNature,
    pub reference: String,
    /// The act as the title names it, `Décret n° 2017-44/PR/MFF`.
    pub label: String,
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</h\d>|</li>|</tr>|</table>").unwrap());
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]>").unwrap());
/// `Article 1er :`, `ARTICLE 2 :`, `Article 1-1-1 :`, `Article 448.1`, `Article 467- 2` or with any dash, `Art. 18.`, `Art, 7.` and `Article unique.` before a dash,
/// and as a scan misreads them, `ARTlCLE2 :`, `Article 1 0 :`, `Arti c le 4 :`, `. Arlic le 19 :`,
/// `Article 1O :`; and with a footnote call glued to a suffix, `Article 162 bis77`.
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[.·•]\s*){0,3}(?:A ?r ?t(?: ?i ?c ?l ?e)?|A ?R ?T ?[Il1] ?C ?L ?E|A[ .]?(?:[^\s\d:.][ .]?){1,3}i ?c ?l ?e)\s*[.,]?\s*(1\s?er|[Pp]remier|PREMIER|[Uu]nique|UNIQUE|[IVX]{1,4}|\d[\dO]*(?:\s\d)?(?:[\-\x{2010}-\x{2015}]\s?\d+|\.\d+)*(?:\s?-?\s?(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|decies))?)(?:\d{1,3})?\b[\s.,:;\-_\x{2010}-\x{2015}]*(.*)$").unwrap()
});
/// What follows a number in running text rather than in a heading, `Article 5 du décret …`, and
/// after the head of an article an amendment quotes, `Article 14 nouveau :`.
static RUNNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:du|de|des|d'|d\x{2019}|et|à|au|aux|alinéa|al\.|ainsi|précité|susvisé|ci-dessus|ci-après|est|sont|nouveau|nouveaux|nouvelle|ancien)\b").unwrap()
});
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(partie|livre|titre|chapitre|section|sous-section)\s+(?:[IVXLC]+|\d+|premier|première|unique|préliminaire|seconde?|\p{L}+i[eèé]me)\b").unwrap()
});
/// The place and day a text is signed or promulgated, `Fait à Djibouti`, `Promulguée à Antananarivo`.
static SIGNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Fait|FAIT|Promulgu\w*|PROMULGU\w*)\s+(?:à|À|A)\s+\p{Lu}").unwrap()
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bn\s*[°º]\s*").unwrap());
static SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s?-?\s?(?:bis|ter|quater|quinquies|sexies|septies|octies|nonies|decies)$")
        .unwrap()
});
static EFFECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(modifi|compl[eé]t|abrog|rempla[cç])").unwrap());
static TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(loi\s+organique|loi|ordonnance|d[ée]cret|arr[êe]t[ée]|d[ée]cision|d[ée]lib[ée]ration)\s+n\s*[°º]\s*(\d)").unwrap()
});
/// The legislature after a law's number, `9ème L`, however it is spaced.
static LEGISLATURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\s*(?:EME|E)?\s*L\b").unwrap());
static INDIVIDUAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b(?:nomination|nommant|nomm[ée]|remaniement|composition du gouvernement|naturalisation|nationalit[ée]|r[ée]int[ée]gration|int[ée]gration|titularisation|promotion|avancement|reclassement|d[ée]tachement|disponibilit[ée]|retraite|r[ée]vocation|radiation|licenciement|d[ée]mission|mutation|affectation|int[ée]rim|fin de fonctions|cessation de fonctions|d[ée]l[ée]gation de signature|d[ée]coration|m[ée]daille|ordre national|distinction|concession|agr[ée]ment|accordant|octroi|bourse|stage|concours|examen|recrutement|candidats?|admission|admis|liste d.aptitude|gr[âa]ce|remise de peine|commutation|expulsion|[ée]l[èe]ves|jury|honorariat|cong[ée]|Mme|Mlle|Monsieur|Madame|Sieur|Dame)\b|\bM\.\s)").unwrap()
});
static GENERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:fixant|r[ée]glementant|r[ée]glementation|portant organisation|organisant|r[ée]organis|portant cr[ée]ation|instituant|institution|portant statut|statut particulier|portant r[ée]glement|r[èe]glement int[ée]rieur|portant code|portant application|d.application|portant modification|modifiant|compl[ée]tant|abrogeant|relati(?:f|ve|fs|ves)|d[ée]finissant|d[ée]terminant|pr[ée]cisant|promulguant|promulgation|rendant (?:applicable|ex[ée]cutoire)|portant r[ée]gime|portant attributions|portant fixation|portant mise en (?:place|[œo]uvre)|portant approbation du (?:r[èe]glement|cahier))").unwrap()
});

fn day(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .and_then(|s| s.get(..10))
        .filter(|d| d.as_bytes().get(4) == Some(&b'-'))
        .map(str::to_string)
}

fn terms(item: &Value, taxonomy: &str) -> Vec<u64> {
    item.get(taxonomy)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn plain(html: &str) -> String {
    let text = TAG.replace_all(html, "");
    html_escape::decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A page of the texts the API answers.
pub fn ejo_texts(json: &str) -> Result<Vec<EjoText>, String> {
    let items: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    items
        .into_iter()
        .map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_u64)
                .ok_or("a text without an id")?;
            let acf = item.get("acf");
            Ok(EjoText {
                id,
                day: day(item.get("date")),
                modified: day(item.get("modified")),
                title: item
                    .pointer("/title/rendered")
                    .and_then(Value::as_str)
                    .map(plain)
                    .unwrap_or_default(),
                reference: acf
                    .and_then(|a| a.get("reference"))
                    .and_then(Value::as_str)
                    .map(|r| r.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|r| !r.is_empty()),
                natures: terms(&item, "nature-dun-texte"),
                institutions: terms(&item, "institution"),
                issue: acf
                    .and_then(|a| a.get("journal_officiel"))
                    .and_then(|j| j.as_u64().or_else(|| j.as_array()?.first()?.as_u64())),
                link: item.get("link").and_then(Value::as_str).map(str::to_string),
                content: item
                    .pointer("/content/rendered")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                visas: acf
                    .and_then(|a| a.get("visas"))
                    .and_then(Value::as_str)
                    .map(journal_body)
                    .filter(|v| !v.is_empty()),
                signature: acf
                    .and_then(|a| a.get("signature"))
                    .and_then(Value::as_str)
                    .map(journal_body)
                    .filter(|v| !v.is_empty()),
            })
        })
        .collect()
}

/// A page of the issues the API answers; an issue's day is written `19830228`.
pub fn ejo_issues(json: &str) -> Result<Vec<EjoIssue>, String> {
    let items: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    items
        .into_iter()
        .map(|item| {
            let id = match item.get("id") {
                Some(Value::Number(n)) => n.as_u64(),
                Some(Value::String(s)) => s.parse().ok(),
                _ => None,
            }
            .ok_or("an issue without an id")?;
            let acf = item.get("acf");
            let text = |k: &str| {
                acf.and_then(|a| a.get(k))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            };
            Ok(EjoIssue {
                id,
                number: text("numero").map(str::to_string),
                day: text("date")
                    .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
                    .map(|d| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..])),
                link: item.get("link").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

/// A page of taxonomy terms: each one's id and name.
pub fn ejo_terms(json: &str) -> Result<Vec<(u64, String)>, String> {
    let items: Vec<Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(items
        .iter()
        .filter_map(|t| Some((t.get("id")?.as_u64()?, plain(t.get("name")?.as_str()?))))
        .collect())
}

impl EjoNature {
    /// The nature the site files the text under, else the one its title opens with.
    pub fn of(natures: &[u64], title: &str) -> Self {
        let filed = natures.iter().find_map(|n| match n {
            247 | 260 => Some(Self::Loi),
            259 => Some(Self::LoiOrganique),
            263 => Some(Self::Ordonnance),
            248 | 264 => Some(Self::Decret),
            256 | 268 | 269 | 1310 => Some(Self::Arrete),
            254 | 1320 => Some(Self::Decision),
            1318 => Some(Self::Deliberation),
            257 | 265 => Some(Self::Circulaire),
            249 | 1316 | 1324 => Some(Self::Accord),
            1308 | 1312 => Some(Self::Avis),
            1322 => Some(Self::Instruction),
            255 => Some(Self::Resolution),
            253 | 261 => Some(Self::Rectificatif),
            _ => None,
        });
        match filed {
            Some(nature) => nature,
            None if natures.is_empty() => Self::named(title),
            None => Self::Other,
        }
    }

    /// A law, an ordinance, a decree or a deliberation: what the switch to normative texts keeps
    /// whatever its title, an arrêté or a décision being kept by its title.
    pub fn normative(self) -> bool {
        matches!(
            self,
            Self::Loi | Self::LoiOrganique | Self::Ordonnance | Self::Decret | Self::Deliberation
        )
    }

    fn named(words: &str) -> Self {
        let words = lj_fold::fold(words);
        match words.split_whitespace().next().unwrap_or_default() {
            "loi" if words.starts_with("loi organique") => Self::LoiOrganique,
            "loi" => Self::Loi,
            "ordonnance" => Self::Ordonnance,
            "decret" => Self::Decret,
            "arrete" => Self::Arrete,
            "decision" => Self::Decision,
            "deliberation" => Self::Deliberation,
            "circulaire" => Self::Circulaire,
            "instruction" => Self::Instruction,
            "rectificatif" | "erratum" => Self::Rectificatif,
            _ => Self::Other,
        }
    }

    /// The head of a text's uid: a numbering, which an organic law shares with the other laws.
    pub fn code(self) -> &'static str {
        match self {
            Self::Loi | Self::LoiOrganique => "LOI",
            Self::Ordonnance => "ORDONNANCE",
            Self::Decret => "DECRET",
            Self::Arrete => "ARRETE",
            Self::Decision => "DECISION",
            Self::Deliberation => "DELIBERATION",
            Self::Circulaire => "CIRCULAIRE",
            Self::Accord => "ACCORD",
            Self::Avis => "AVIS",
            Self::Instruction => "INSTRUCTION",
            Self::Resolution => "RESOLUTION",
            Self::Rectificatif => "RECTIFICATIF",
            Self::Other => "TEXTE",
        }
    }

    pub fn nature(self) -> &'static str {
        match self {
            Self::LoiOrganique => "LOI_ORGANIQUE",
            other => other.code(),
        }
    }
}

/// The institution the eJO files the Conseil constitutionnel's texts under.
const COUNCIL: u64 = 880;
/// The Constitution's promulgation, which created the Council.
const COUNCIL_SINCE: &str = "1992-09-15";

/// A ruling of the Conseil constitutionnel, never its rules of procedure nor an Assembly
/// resolution the site files under it.
pub fn ejo_council_ruling(t: &EjoText) -> bool {
    let reference = t
        .reference
        .clone()
        .or_else(|| ejo_title_reference(&t.title))
        .unwrap_or_default();
    t.institutions.contains(&COUNCIL)
        && t.day.as_deref().is_some_and(|d| d >= COUNCIL_SINCE)
        && matches!(
            EjoNature::of(&t.natures, &t.title),
            EjoNature::Decision | EjoNature::Resolution | EjoNature::Other
        )
        && !lj_fold::fold(&t.title).contains("reglement interieur")
        && !reference.to_ascii_uppercase().contains("/AN/")
}

/// A title that names a person or decides one case: a nomination, a concession, a concours.
pub fn ejo_individual(title: &str) -> bool {
    INDIVIDUAL.is_match(title)
}

/// A title of an arrêté or a décision that lays down general rules.
pub fn ejo_general(title: &str) -> bool {
    GENERAL.is_match(title)
}

/// A reference as a key: folded, the legislature after a law's number written `9L`, the leading
/// zeros of a number dropped. `106/AN/24/9ème L` and `106/AN/24/9èmeL` are `106-AN-24-9L`.
pub fn ejo_reference_key(reference: &str) -> String {
    let upper = lj_fold::fold(&reference.replace(['°', 'º'], "e")).to_uppercase();
    let upper = LEGISLATURE.replace_all(&upper, "${1}L");
    upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| match t.trim_start_matches('0') {
            "" => "0",
            bare if t.bytes().all(|b| b.is_ascii_digit()) => bare,
            _ => t,
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// A national number as a key: folded, its separators written as dashes, its zeros kept, since two
/// Malagasy series of 1959 differ by them alone. `609/2002` is `609-2002`, `91-010 bis` is `91-010-BIS`.
pub fn journal_number_key(number: &str) -> String {
    lj_fold::fold(number)
        .to_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The reference a title gives after its `n°`.
pub fn ejo_title_reference(title: &str) -> Option<String> {
    reference_at(&title[NUMBER.find(title)?.end()..])
}

/// The reference that opens `rest`: a number cut after a dash or a slash goes on past the space,
/// `2002- 0136/PR/MEF`, and a numbering by legislature keeps its `L` apart, `106/AN/24/9ème L`.
fn reference_at(rest: &str) -> Option<String> {
    let mut token = String::new();
    let mut at = 0;
    loop {
        let start = at + rest[at..].len() - rest[at..].trim_start().len();
        let word: String = rest[start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ';')
            .collect();
        if word.is_empty() {
            break;
        }
        token.push_str(&word);
        at = start + word.len();
        if !token.ends_with(['-', '/']) {
            break;
        }
    }
    let token = token.trim_end_matches(['.', ')']);
    if !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let legislature = token.contains('/')
        && rest[at..]
            .trim_start()
            .strip_prefix('L')
            .is_some_and(|after| {
                !after.starts_with(|c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}')
            });
    Some(if legislature {
        format!("{token} L")
    } else {
        token.to_string()
    })
}

/// The act a title says its text modifies, completes, replaces or repeals: the first one it names
/// after the verb.
pub fn ejo_effects(title: &str) -> Vec<EjoEffect> {
    let Some(verb) = EFFECT.captures(title) else {
        return Vec::new();
    };
    let at = verb.get(0).expect("a match").end();
    let Some(target) = TARGET.captures(&title[at..]) else {
        return Vec::new();
    };
    let start = at + target.get(2).expect("a reference").start();
    let Some(reference) = reference_at(&title[start..]) else {
        return Vec::new();
    };
    let verb = match lj_fold::fold(&verb[1]).as_str() {
        "abrog" => "abroge",
        v if v.starts_with("rempla") => "remplace",
        _ => "modifie",
    };
    vec![EjoEffect {
        verb,
        nature: EjoNature::named(&target[1]),
        label: format!("{} n° {reference}", target[1].trim()),
        reference,
    }]
}

/// The lines of a text's content, its tags gone and its spaces collapsed, up to its signatures.
fn lines(html: &str) -> Vec<String> {
    let text = BREAK.replace_all(html, "\n");
    let text = CELL.replace_all(&text, " ");
    let text = TAG.replace_all(&text, "");
    text_lines(&html_escape::decode_html_entities(&text))
}

fn text_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .take_while(|l| !SIGNED.is_match(l))
        .collect()
}

/// A journal text's HTML content as lines, up to its signatures.
pub fn journal_body(html: &str) -> String {
    lines(html).join("\n")
}

/// A journal text read from a PDF as lines, up to its signatures.
pub fn journal_text_body(text: &str) -> String {
    text_lines(text).join("\n")
}

/// The rank of a number in its text's sequence: `1er`, `premier`, `unique` and `I` are 1, `12bis` 12.
fn rank(num: &str) -> Option<u32> {
    let lower = num.to_lowercase();
    if lower.starts_with("premier") || lower.starts_with("unique") {
        return Some(1);
    }
    if !lower.is_empty() && lower.chars().all(|c| matches!(c, 'i' | 'v' | 'x')) {
        let value = |c| match c {
            'i' => 1i32,
            'v' => 5,
            _ => 10,
        };
        let digits: Vec<i32> = lower.chars().map(value).collect();
        let total: i32 = digits
            .iter()
            .enumerate()
            .map(|(i, &d)| {
                if digits.get(i + 1).is_some_and(|&n| n > d) {
                    -d
                } else {
                    d
                }
            })
            .sum();
        return u32::try_from(total).ok();
    }
    lower
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// The numbered articles of a journal text's HTML content.
pub fn journal_articles(html: &str) -> Vec<ConsolidatedArticle> {
    articles(lines(html))
}

/// The numbered articles of a journal text read from a PDF.
pub fn journal_text_articles(text: &str) -> Vec<ConsolidatedArticle> {
    articles(text_lines(text))
}

/// What a break in a text's numbering costs at most, in heads: one per number it skips.
const GAP: i64 = 3;

/// A line that may open an article.
pub(crate) struct Head {
    line: usize,
    num: String,
    rank: u32,
    rest: String,
    /// The number read without digits a scan glued to it, a footnote call or a stray figure.
    trimmed: bool,
}

impl Head {
    /// The head of `num` on line `line`, `None` for a number with no rank; an `alternate`
    /// reading of a line costs one head to the sequence that keeps it.
    pub(crate) fn new(line: usize, num: String, rest: String, alternate: bool) -> Option<Self> {
        Some(Self {
            line,
            rank: rank(&num)?,
            num,
            rest,
            trimmed: alternate,
        })
    }
}

fn digits(num: &str) -> bool {
    num.bytes().all(|b| b.is_ascii_digit())
}

fn heads(lines: &[String]) -> Vec<Head> {
    let mut heads = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let Some(c) = HEAD.captures(text) else {
            continue;
        };
        let rest = c.get(2).map_or("", |m| m.as_str());
        if RUNNING.is_match(rest) {
            continue;
        }
        let number = c.get(1).expect("a head's number");
        let (figures, suffix) = match SUFFIX.find(number.as_str()) {
            Some(m) => (&number.as_str()[..m.start()], Some(m.as_str())),
            None => (number.as_str(), None),
        };
        let figures = figures
            .split_whitespace()
            .collect::<String>()
            .replace('O', "0")
            .replace(
                |c: char| c == '.' || ('\u{2010}'..='\u{2015}').contains(&c),
                "-",
            );
        let num = match suffix {
            Some(suffix) => format!("{figures} {}", suffix.trim_start_matches([' ', '-'])),
            None => figures,
        };
        let mut push = |num: &str, rest: &str, trimmed: bool| {
            if let Some(rank) = rank(num) {
                heads.push(Head {
                    line,
                    num: num.to_string(),
                    rank,
                    rest: rest.to_string(),
                    trimmed,
                });
            }
        };
        push(&num, rest, false);
        if let Some((parent, last)) = num
            .rsplit_once('-')
            .filter(|(_, l)| l.len() > 1 && digits(l))
        {
            if dashed(parent).is_some() {
                (1..last.len()).for_each(|k| push(&format!("{parent}-{}", &last[..k]), rest, true));
            }
        }
        if num.len() < 2 || !digits(&num) {
            continue;
        }
        match number.as_str().split_once(' ') {
            Some((first, _)) => push(
                first,
                text[number.start() + first.len()..].trim_start(),
                true,
            ),
            None => (1..num.len()).for_each(|k| push(&num[..k], rest, true)),
        }
    }
    heads
}

/// What it costs to read `b` as the article after `a`, `None` when it cannot follow it.
fn step(a: &Head, b: &Head) -> Option<i64> {
    if b.line <= a.line {
        return None;
    }
    match b.rank.cmp(&a.rank) {
        Ordering::Less => None,
        Ordering::Equal => match (dashed(&a.num), dashed(&b.num)) {
            (Some(x), Some(y)) => below(&x, &y),
            _ => (b.num != a.num && !digits(&b.num)).then_some(0),
        },
        Ordering::Greater if b.num.contains('-') => {
            Some(dashed(&b.num).map_or(0, |y| below(&y[..1], &y).unwrap_or(0)))
        }
        Ordering::Greater if b.rank == a.rank + 1 => Some(0),
        Ordering::Greater if a.num.contains('-') => Some(GAP),
        Ordering::Greater => Some(i64::from(b.rank - a.rank - 1).min(GAP)),
    }
}

/// The parts of a dashed number, `[218, 4, 1]` for `218-4-1`; `None` with a suffix.
fn dashed(num: &str) -> Option<Vec<u32>> {
    num.split('-').map(|p| p.parse().ok()).collect()
}

/// What it costs to read `b` after `a` within one article's dashed numbering, `None` when `b`
/// goes back: nothing while it skips at most [`GAP`] parts, from where `a` stands or from 1 on a
/// level `a` has not opened, else [`GAP`], the jump of a footnote call glued to the last part,
/// 44-1 and its call 15 read `44-115`.
fn below(a: &[u32], b: &[u32]) -> Option<i64> {
    let common = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if common == b.len() {
        return None;
    }
    let mut cost = 0;
    for (i, &part) in b.iter().enumerate().skip(common) {
        let from = match a.get(i) {
            Some(&p) if i == common => p + 1,
            _ => 1,
        };
        if part < from {
            return None;
        }
        cost += i64::from(part - from);
    }
    Some(if cost > GAP { GAP } else { 0 })
}

/// The heads that open the text's articles: the run in sequence that keeps the most of them,
/// each break in its numbering costing the numbers it skips, up to [`GAP`], and a dashed
/// numbering left for a plain one costing [`GAP`].
fn sequence(heads: &[Head]) -> Vec<usize> {
    let mut best = vec![0i64; heads.len()];
    let mut next = vec![None; heads.len()];
    for i in (0..heads.len()).rev() {
        let mut kept: (i64, Option<(usize, i64)>) = (0, None);
        for j in i + 1..heads.len() {
            let Some(cost) = step(&heads[i], &heads[j]) else {
                continue;
            };
            let score = best[j] - cost;
            let cheaper = kept.1.is_some_and(|(_, c)| score == kept.0 && cost < c);
            if score > kept.0 || cheaper {
                kept = (score, Some((j, cost)));
            }
        }
        best[i] = 1 + kept.0 - i64::from(heads[i].trimmed);
        next[i] = kept.1.map(|(j, _)| j);
    }
    let entry = |h: &Head| match h.rank == 1 || h.num.contains('-') {
        true => 0,
        false => GAP,
    };
    let Some(mut at) = (0..heads.len()).max_by_key(|&i| (best[i] - entry(&heads[i]), Reverse(i)))
    else {
        return Vec::new();
    };
    let mut run = vec![at];
    while let Some(j) = next[at] {
        run.push(j);
        at = j;
    }
    run
}

/// The numbered articles of a text's lines, each heading opening a level of its path. A number
/// out of its text's sequence is the text of the article before it: an article it quotes or
/// amends, or a head a scan misread.
fn articles(lines: Vec<String>) -> Vec<ConsolidatedArticle> {
    let heads = heads(&lines);
    articles_at(&lines, &heads, heading_level)
}

/// A French heading's level: partie, livre, titre, chapitre, section, then the rest.
fn heading_level(line: &str) -> Option<usize> {
    if line.len() >= 200 || !line.starts_with(char::is_uppercase) || line.ends_with(['.', ';', ','])
    {
        return None;
    }
    let c = HEADING.captures(line)?;
    Some(match c[1].to_lowercase().as_str() {
        "partie" => 0,
        "livre" => 1,
        "titre" => 2,
        "chapitre" => 3,
        "section" => 4,
        _ => 5,
    })
}

/// The articles `heads` open in `lines`, a line `heading` gives a level opening that level of
/// their path.
pub(crate) fn articles_at(
    lines: &[String],
    heads: &[Head],
    heading: impl Fn(&str) -> Option<usize>,
) -> Vec<ConsolidatedArticle> {
    let mut opening: HashMap<usize, &Head> = sequence(heads)
        .into_iter()
        .map(|i| (heads[i].line, &heads[i]))
        .collect();
    let mut articles: Vec<ConsolidatedArticle> = Vec::new();
    let mut path: Vec<Option<String>> = Vec::new();
    let mut open = false;
    for (i, line) in lines.iter().enumerate() {
        if let Some(head) = opening.remove(&i) {
            let p: Vec<&str> = path.iter().flatten().map(String::as_str).collect();
            articles.push(ConsolidatedArticle {
                num: head.num.clone(),
                title_path: (!p.is_empty()).then(|| p.join(" > ")),
                texte: head.rest.clone(),
                nota: None,
            });
            open = true;
            continue;
        }
        if let Some(level) = heading(line) {
            path.resize(level, None);
            path.push(Some(line.clone()));
            open = false;
            continue;
        }
        if open {
            let texte = &mut articles.last_mut().expect("an open article").texte;
            if !texte.is_empty() {
                texte.push('\n');
            }
            texte.push_str(line);
        }
    }
    articles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_council_s_rulings_are_its_decisions_proclamations_and_resolutions_since_1992() {
        let post = |title: &str, day: &str, natures: Vec<u64>, institutions: Vec<u64>| EjoText {
            id: 1,
            day: Some(day.to_string()),
            modified: None,
            title: title.to_string(),
            reference: None,
            natures,
            institutions,
            issue: None,
            link: None,
            content: None,
            visas: None,
            signature: None,
        };
        let cases = [
            ("Décision n° 01/2013", "2013-02-04", vec![254], true),
            ("Décision Proclamation n° 3/2018/CC du 08 mars 2018 portant proclamation de résultats", "2018-03-08", vec![], true),
            ("Proclamation n° des résultats des élections législatives du 22 février 2013.", "2013-03-12", vec![], true),
            ("Résolution n° 01/2002 du Conseil Constitutionnel relative sur la conformité à la Constitution de la Loi Organique modificative n° 11/AN/2002/4ème L", "2002-07-18", vec![255], true),
            ("Décision n° 04/CC/2023 modifiant la décision n° 01/CC/2014 rendant exécutoire le règlement intérieur du Conseil Constitutionnel.", "2023-04-24", vec![254], false),
            ("Résolution n° 08/AN/2012 portant sur la mise en place d'une commission Ad'Hoc", "2012-03-21", vec![255], false),
            ("Décision n° 72-438/SG/AE complétant la liste des indices de prix", "1972-03-22", vec![254], false),
            ("Loi n° 92/AN/05/5ème L Relative aux Statuts de la Banque Centrale de Djibouti.", "2005-01-16", vec![247], false),
        ];
        for (title, day, natures, ruling) in cases {
            assert_eq!(
                ejo_council_ruling(&post(title, day, natures, vec![COUNCIL])),
                ruling,
                "{title}"
            );
        }
        assert!(!ejo_council_ruling(&post(
            "Décision n° 01/2013",
            "2013-02-04",
            vec![254],
            vec![]
        )));
    }

    #[test]
    fn a_text_reads_its_articles_in_sequence_and_stops_at_its_signatures() {
        let html = "<p>Vu la Constitution ;</p><p>CHAPITRE I : Dispositions générales</p>\
            <p>Article 1er : L’Article 5 du décret n°2017-44/PR/MFF est modifié comme suit :<br />\
            Article 5 : A Djibouti ville, le dépôt se fait au secrétariat.</p>\
            <p>Art. 2. \u{2014} Le reste est inchangé.</p><p>Article 3 du présent décret est abrogé.</p>\
            <p>ARTICLE 3 : Le présent Décret sera publié.</p><p>Fait à Djibouti, le 19 janvier 2022</p>\
            <p>ISMAIL OMAR GUELLEH</p>";
        let read: Vec<(String, Option<String>, String)> = journal_articles(html)
            .into_iter()
            .map(|a| (a.num, a.title_path, a.texte))
            .collect();
        let path = Some("CHAPITRE I : Dispositions générales".to_string());
        assert_eq!(
            read,
            [
                (
                    "1er".to_string(),
                    path.clone(),
                    "L’Article 5 du décret n°2017-44/PR/MFF est modifié comme suit :\n\
                     Article 5 : A Djibouti ville, le dépôt se fait au secrétariat."
                        .to_string()
                ),
                (
                    "2".to_string(),
                    path.clone(),
                    "Le reste est inchangé.\nArticle 3 du présent décret est abrogé.".to_string()
                ),
                (
                    "3".to_string(),
                    path,
                    "Le présent Décret sera publié.".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_misread_or_quoted_head_loses_none_of_the_articles_after_it() {
        let text = "ARTICLE PREMIER : Objet.\nARTlCLE2 : Champ.\nArticle 312\nLe mineur peut accepter.\n\
            Article 4 : Définitions.\nArticle 14 nouveau : Texte cité.\nArti c le 6 : Agrément.\n\
            Arlic le 74 : Recours.\nA rti c le 8 : Délai.\n. Adic le 9 : Sanctions.\n\
            Article 1 0 : Exclusivité.\nArticle 11 : Publication.\nFait à Porto-Novo, le 2 janvier 2020";
        let read: Vec<(String, String)> = journal_text_articles(text)
            .into_iter()
            .map(|a| (a.num, a.texte))
            .collect();
        let expected = [
            ("PREMIER", "Objet."),
            ("2", "Champ."),
            ("3", "Le mineur peut accepter."),
            ("4", "Définitions.\nArticle 14 nouveau : Texte cité."),
            ("6", "Agrément."),
            ("7", "Recours."),
            ("8", "Délai."),
            ("9", "Sanctions."),
            ("10", "Exclusivité."),
            ("11", "Publication."),
        ]
        .map(|(n, t)| (n.to_string(), t.to_string()));
        assert_eq!(read, expected);
        let nums: Vec<String> =
            journal_text_articles("Article 9 : Neuf.\nArticle 1O : Dix.\n· · Article 11 : Onze.")
                .into_iter()
                .map(|a| a.num)
                .collect();
        assert_eq!(nums, ["9", "10", "11"]);
    }

    #[test]
    fn a_heading_numbered_in_words_opens_its_level_and_running_text_none() {
        let text =
            "LIVRE DEUXIÉME : DES CONTRATS\nTITRE PREMIER : DE LA VENTE\nArticle 478 : La vente.\n\
            le livre IV de la loi n° 70-03\nArticle 479 : Le prix.";
        let paths: Vec<Option<String>> = journal_text_articles(text)
            .into_iter()
            .map(|a| a.title_path)
            .collect();
        let path = Some("LIVRE DEUXIÉME : DES CONTRATS > TITRE PREMIER : DE LA VENTE".to_string());
        assert_eq!(paths, [path.clone(), path]);
    }

    #[test]
    fn a_number_keeps_its_suffix_and_a_code_its_dashes() {
        let html = "<p>Article 1-1-1 : Objet de la Loi.</p><p>Article 1-1-2 : Terminologie</p>\
            <p>Article 4 : Quatre.</p><p>Art. 4 bis. \u{2014} Inséré.</p><p>ARTICLE V : Cinq.</p>";
        let nums: Vec<String> = journal_articles(html).into_iter().map(|a| a.num).collect();
        assert_eq!(nums, ["1-1-1", "1-1-2"]);
        let html =
            "<p>Article 4 : Quatre.</p><p>Art. 4 bis. \u{2014} Inséré.</p><p>ARTICLE V : Cinq.</p>";
        let nums: Vec<String> = journal_articles(html).into_iter().map(|a| a.num).collect();
        assert_eq!(nums, ["4", "4 bis", "V"]);
    }

    #[test]
    fn a_glued_footnote_call_and_any_dash_leave_the_number() {
        let text = "Article 1\nUn.\nArticle 2\nDeux.\nArticle 2\u{2013}169\nTrois.\nArticle 2- 2\nQuatre.\n\
            Article 2.3\nCinq.\nArticle 3\nSix.\nArticle 3 bis77\nSept.\nArticle 4\nHuit.";
        let nums: Vec<String> = journal_articles(text).into_iter().map(|a| a.num).collect();
        assert_eq!(nums, ["1", "2", "2-1", "2-2", "2-3", "3", "3 bis", "4"]);
    }

    #[test]
    fn a_reference_keys_one_way_however_it_is_spaced() {
        assert_eq!(ejo_reference_key("106/AN/24/9ème L"), "106-AN-24-9L");
        assert_eq!(ejo_reference_key("106/AN/24/9èmeL"), "106-AN-24-9L");
        assert_eq!(ejo_reference_key("003/AN/18/8ème L"), "3-AN-18-8L");
        assert_eq!(ejo_reference_key("2000-0028/PR"), "2000-28-PR");
        assert_eq!(ejo_reference_key("516/6eL"), "516-6L");
        assert_eq!(ejo_reference_key("452/6° L"), "452-6L");
        assert_eq!(ejo_reference_key("401/6 L"), "401-6L");
        assert_eq!(
            ejo_title_reference("modifiant le décret n° 2002- 0136/PR/MEF portant").as_deref(),
            Some("2002-0136/PR/MEF")
        );
        assert_eq!(
            ejo_title_reference("la délibération n° 452/6° L du 13 janvier").as_deref(),
            Some("452/6° L")
        );
        assert_eq!(
            ejo_title_reference("Décret n° 2010-0083/PRE L'article").as_deref(),
            Some("2010-0083/PRE")
        );
        assert_eq!(
            ejo_title_reference("Loi n° 193/AN/25/9ème L portant approbation").as_deref(),
            Some("193/AN/25/9ème L")
        );
        assert_eq!(
            ejo_title_reference("Décret n° 2022-013/PR/MI fixant").as_deref(),
            Some("2022-013/PR/MI")
        );
    }

    #[test]
    fn a_title_names_the_act_it_amends_or_repeals() {
        let effects = ejo_effects("Loi n° 178/AN/25/9ème L portant modification de la Loi n° 106/AN/24/9ème L relative à la Lutte contre le Blanchiment");
        assert_eq!(
            effects,
            [EjoEffect {
                verb: "modifie",
                nature: EjoNature::Loi,
                reference: "106/AN/24/9ème L".to_string(),
                label: "Loi n° 106/AN/24/9ème L".to_string(),
            }]
        );
        let effects = ejo_effects("Décret n° 2021-315/PR/MFF portant modification des dispositions du Décret n° 2017-44/PR/MFF portant modification du Décret n° 2000-0028/PR");
        assert_eq!(effects[0].reference, "2017-44/PR/MFF");
        assert_eq!(
            ejo_effects("Arrêté n° 2021-015/PR/MB abrogeant l’arrêté n° 2018-198/PR/MB.")[0].verb,
            "abroge"
        );
        assert!(ejo_effects(
            "Loi n° 168/AN/25/9ème L portant modification partielle de l’article 113 du Code de Travail."
        )
        .is_empty());
    }

    #[test]
    fn a_text_names_its_issue_as_a_number_or_a_list() {
        let texts = ejo_texts(
            r#"[{"id":1,"title":{"rendered":"Arrêté n° 1"},"acf":{"journal_officiel":105763}},
                {"id":2,"title":{"rendered":"Arrêté n° 2"},"acf":{"journal_officiel":[105764]}},
                {"id":3,"title":{"rendered":"Arrêté n° 3"},"acf":{"journal_officiel":false}}]"#,
        )
        .unwrap();
        let issues: Vec<Option<u64>> = texts.iter().map(|t| t.issue).collect();
        assert_eq!(issues, [Some(105763), Some(105764), None]);
    }

    #[test]
    fn a_nature_comes_from_the_site_else_from_the_title() {
        assert_eq!(EjoNature::of(&[256], "Décret n° 1"), EjoNature::Arrete);
        assert_eq!(
            EjoNature::of(&[], "Loi n° 18/AN/82 sur le budget"),
            EjoNature::Loi
        );
        assert_eq!(
            EjoNature::of(&[], "Loi organique n° 1"),
            EjoNature::LoiOrganique
        );
        assert_eq!(
            EjoNature::of(&[258], "Décision Proclamation"),
            EjoNature::Other
        );
        assert!(ejo_individual(
            "Décret n° 2022-009/PR/MTFPS portant nomination."
        ));
        assert!(ejo_individual(
            "DELIBERATION n° 514/6e L accordant des parcelles de terrains en concession provisoire"
        ));
        assert!(!ejo_individual(
            "Décret n° 2022-013/PR/MI fixant la répartition des Conseillers Régionaux"
        ));
        assert!(ejo_general(
            "Arrêté n° 2021-015/PR/MB modifiant et complétant l’arrêté n° 2018-198/PR/MB"
        ));
        assert!(!ejo_general(
            "Arrêté n° 2021-014/PR/ML portant approbation du budget prévisionnel 2021"
        ));
    }

    #[test]
    fn a_national_number_keys_one_way_however_it_is_written() {
        assert_eq!(journal_number_key("609/2002"), "609-2002");
        assert_eq!(journal_number_key("2019-006'"), "2019-006");
        assert_eq!(journal_number_key("59-019"), "59-019");
        assert_eq!(journal_number_key("91-010 bis"), "91-010-BIS");
        assert_eq!(
            journal_number_key("1355-Mininter/SG/DPROD/SOE/"),
            "1355-MININTER-SG-DPROD-SOE"
        );
    }
}
