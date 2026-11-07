//! Usage names scholarly writing pairs with a reference given by number and day:
//! « Cass. com., 24 nov. 1998, n° 96-18.357, Chevassus-Marche », « l'arrêt SAT (Cass. …) ».

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use lj_core::case_names::{case_name_key, MAX_NAME_WORDS};
use lj_core::citation::name_words;
use lj_core::dates::parse_date_fr;
use lj_fold::fold;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::chrono::ChronoSnapshot;
use crate::compiled::{doc_extract, CompiledVocab};
use crate::eponyme::normalise_nom;
use crate::link::LinkSnapshot;

/// A name a deposit writes beside a reference it gives by number and day.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DoctrineCitation {
    /// `cc`, `ce` or `tc`, the court the case linker reads.
    pub court: String,
    /// ISO day the reference writes.
    pub date: String,
    /// Lowercase alphanumerics, the docket key a decision is found by.
    pub docket: String,
    pub name: String,
}

struct Linker {
    vocab: CompiledVocab,
    snap: LinkSnapshot,
    chrono: ChronoSnapshot,
}

static LINKER: LazyLock<Linker> = LazyLock::new(|| {
    let snap = LinkSnapshot::build(Vec::new(), Vec::new());
    Linker {
        vocab: CompiledVocab::build(&[], &snap),
        snap,
        chrono: ChronoSnapshot::new(Vec::new()),
    }
});

const NAME: &str = r"[A-ZÀ-ÖØ-Þ][\p{L}\p{N}'’&.\-]*(?:\s+(?:[A-ZÀ-ÖØ-Þ][\p{L}\p{N}'’&.\-]*|[dl]['’][A-ZÀ-ÖØ-Þ][\p{L}'’\-]*|c/|c\.|et|de|du|des|la|le|les|en|sur|au|aux|à|pour))*";

const COURT: &str = r"(?:Cass\.?|Cour\s+de\s+cassation|Civ\.?|Com\.|Soc\.|Crim\.|Req\.|Ass\.?\s?pl[ée]n\.?|Ch\.?\s?mixte|Ch\.?\s?r[ée]un\.?|\bCE\b|C\.\s?E\.|Cons\.\s?d['’]\s?[ÉE]tat|Conseil\s+d['’]\s?[ÉE]tat|\bTC\b|T\.\s?C\.|T\.\s?confl\.|Trib\.\s?confl\.|Tribunal\s+des\s+conflits|[Cc]hambre\s+(?:civile|commerciale|sociale|criminelle))";

static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(1er|[0-3]?\d)\s+(janv(?:ier)?|f[ée]v(?:r(?:ier)?)?|mars|avr(?:il)?|mai|juin|juil(?:l(?:et)?)?|ao[uû]t|sept(?:embre)?|oct(?:obre)?|nov(?:embre)?|d[ée]c(?:embre)?)\.?\s+(1[789]\d\d|20[0-3]\d)\b")
        .unwrap()
});

/// From the day to the number: nothing but the number's mark, or the name
/// between them as the Conseil d'État writes it (« 20 oct. 1989, Nicolo, n° »).
static GAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*,?\s*(?:(?P<name>[^,;()«»\[\]]{2,90}?)\s*,\s*)?\(?\s*(?:(?i:pourvois?|req\.?|requ[êe]tes?)\s*)?(?:(?i:n\s?[°ºo]s?|num[ée]ros?)\.?\s*)?$")
        .unwrap()
});

/// Between two numbers of one reference.
static ENUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|et|&)\s*(?:(?i:n\s?[°ºo]s?)\.?\s*)?$").unwrap());

/// What follows the reference names its commentator, never its parties.
static REFUSED_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*[,(]?\s*(?i:notes?|obs\.?|observations?|concl\.?|conclusions?|rapp\.?|rapports?|comm\.?|commentaires?|chron\.?|chroniques?|avis|cf\.?)(?:\W|$)")
        .unwrap()
});

/// After the number, a name runs to the punctuation that closes it, its lowercase words included.
static AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?P<open>,|\(|:)\s*(?:(?i:arr[êe]t|aff\.|affaire|dit|dite|jurisprudence)\s+)?(?P<name>[A-ZÀ-ÖØ-Þ][^,;:()«»\[\]]*)",
    )
    .unwrap()
});

/// « l'arrêt SAT (Cass. ass. plén., » up to the day: the name touches the reference.
static BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i:\b(?:arr[êe]ts?|jurisprudence|affaire|aff\.))\s+(?P<name>{NAME})\s*[,(:]?\s*{COURT}(?P<court>[^;()«»\[\]]{{0,60}}?)[,\s]\s*$"
    ))
    .unwrap()
});

/// « M. Clément-Fontaine, « L'exclusion des idées » » writes an author and the title of a work.
static QUOTED_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*,?\s*[«"“]"#).unwrap());

/// « obs. sous Cass. soc., 4 mars 2008, n° 06-45221, Clause pénale… »: lowercase words after it write a title.
static COMMENTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:obs|observations?|notes?|comm|commentaires?|concl|conclusions?|chron)\.?\s+(?:sous|ss\.?|sur)\s+(?:\S+\s+){0,4}\S*\s*$")
        .unwrap()
});

/// « 21 mars 2016, Société Fairvesta et autres. CE, n° 390023 »: the name is the previous reference's.
static COURT_ENDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\s{COURT}\s*$")).unwrap());

/// « n° 00-18.192, arrêt Ducasse » names the reference whose number it follows.
static AFTER_A_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d\s*,\s*(?:[dl]['’]\s*)?$").unwrap());

/// After a name, a volume a journal dates or numbers: « BJT avril 2021 », « RLDA 2010/50 ».
static JOURNAL_FOLLOWS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\s,]*(?:(?:1er\s+|[0-3]?\d\s+)?(?i:janv|f[ée]v|mars|avr|mai|juin|juil|ao[uû]t|sept|oct|nov|d[ée]c)\p{L}*\.?\s*(?:1[789]\d\d|20[0-3]\d)|(?:1[789]\d\d|20[0-3]\d)\b|\d{1,4}\s*/\s*\d|n\s?[°ºo]\.?\s*\d|pp?\.\s*\d)")
        .unwrap()
});

/// Words that open no name: journals, reporters, courts and chambers, the words
/// of prose and of a commentary. Folded, dot dropped.
const NOT_A_NAME: &[&str] = &[
    "act",
    "actu",
    "actualite",
    "aff",
    "affaire",
    "aj",
    "ajct",
    "ajda",
    "ajdi",
    "ajfp",
    "ainsi",
    "ann",
    "annales",
    "apres",
    "arr",
    "arret",
    "arrets",
    "art",
    "article",
    "articles",
    "ass",
    "assemblee",
    "aussi",
    "avis",
    "bic",
    "bicc",
    "bjce",
    "bje",
    "bjs",
    "brda",
    "bul",
    "bull",
    "bulletin",
    "c",
    "ca",
    "caa",
    "cah",
    "cahiers",
    "cass",
    "cassation",
    "cc",
    "cce",
    "ccc",
    "ce",
    "cedh",
    "cet",
    "cette",
    "cf",
    "ch",
    "chambre",
    "chron",
    "cit",
    "civ",
    "cjce",
    "cjeg",
    "cjue",
    "code",
    "com",
    "comm",
    "comme",
    "commentaire",
    "concl",
    "conclusions",
    "cons",
    "conseil",
    "const",
    "constr",
    "contrats",
    "cour",
    "crim",
    "d",
    "dalloz",
    "dans",
    "dc",
    "decision",
    "decret",
    "defr",
    "defrenois",
    "depuis",
    "des",
    "deux",
    "dh",
    "donc",
    "dp",
    "dr",
    "droit",
    "ecli",
    "economica",
    "edce",
    "elle",
    "en",
    "enfin",
    "env",
    "europe",
    "f",
    "fp",
    "fr",
    "frh",
    "fs",
    "gaja",
    "gajc",
    "gajf",
    "gaz",
    "gazette",
    "gdas",
    "grands",
    "i",
    "ibid",
    "ibidem",
    "id",
    "idem",
    "ii",
    "iii",
    "il",
    "ils",
    "inedit",
    "infra",
    "iv",
    "jcp",
    "jcpa",
    "jcpe",
    "jcpg",
    "jcpn",
    "jcps",
    "jdi",
    "jo",
    "jorf",
    "jsl",
    "jugement",
    "juridictions",
    "juris",
    "jurisclasseur",
    "jurisdata",
    "jurisp",
    "jurisprudence",
    "just",
    "l",
    "la",
    "lamy",
    "le",
    "lebon",
    "leb",
    "legifrance",
    "les",
    "lexbase",
    "lgdj",
    "liaisons",
    "litec",
    "loi",
    "lpa",
    "mais",
    "me",
    "melanges",
    "n",
    "no",
    "non",
    "note",
    "notes",
    "numero",
    "obs",
    "on",
    "op",
    "or",
    "ord",
    "ordonnance",
    "p",
    "pal",
    "par",
    "pbri",
    "plen",
    "pour",
    "pourvoi",
    "pourvois",
    "pr",
    "prec",
    "precite",
    "proc",
    "procedures",
    "prof",
    "publie",
    "puf",
    "qpc",
    "rapp",
    "rapport",
    "rca",
    "rdc",
    "rdi",
    "rdp",
    "rdss",
    "rdt",
    "rec",
    "recueil",
    "req",
    "requete",
    "resp",
    "rev",
    "revue",
    "rfd",
    "rfda",
    "rgda",
    "rjda",
    "rjf",
    "rjs",
    "rtd",
    "rtdciv",
    "rtdcom",
    "rep",
    "s",
    "sect",
    "section",
    "selon",
    "sem",
    "semaine",
    "si",
    "sirey",
    "soc",
    "ssl",
    "supra",
    "sur",
    "t",
    "ta",
    "tab",
    "tables",
    "tc",
    "tgi",
    "tj",
    "trib",
    "tribunal",
    "un",
    "une",
    "v",
    "voir",
];

/// Names that alone designate no party, a bare head or its abbreviation among them.
const GENERIC: &[&str] = &[
    "asso",
    "assistance",
    "assoc",
    "association",
    "banque",
    "caisse",
    "centre",
    "cie",
    "commune",
    "compagnie",
    "conf",
    "confederation",
    "consorts",
    "credit",
    "cte",
    "cts",
    "departement",
    "dpt",
    "epoux",
    "etat",
    "fed",
    "federation",
    "france",
    "garde",
    "ligue",
    "min",
    "ministere public",
    "ministre",
    "prefet",
    "region",
    "sa",
    "sarl",
    "sas",
    "sci",
    "scp",
    "sel",
    "snc",
    "societe",
    "ste",
    "synd",
    "syndicat",
    "union",
    "ville",
];

/// A word no party name holds.
const NEVER_IN_A_NAME: &[&str] = &["arret", "arrets"];

/// Abbreviations whose dot keeps the name going (« Sté. Bordas »).
const ABBREVIATIONS: &[&str] = &["cie", "ets", "m", "mm", "mme", "soc", "st", "ste", "sa"];

/// Words that end no name.
const GLUE: &[&str] = &[
    "a", "au", "aux", "c", "c/", "c.", "d", "de", "des", "du", "en", "et", "l", "la", "le", "les",
    "pour", "sur",
];

/// Text as a PDF reader lays it out: a soft hyphen dropped, a word cut at the
/// line end rejoined, every run of blanks one space.
pub fn plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut blank = false;
    while let Some(c) = chars.next() {
        if c == '\u{ad}' {
            continue;
        }
        if c == '-' && chars.peek() == Some(&'\n') {
            let mut ahead = chars.clone();
            ahead.next();
            while matches!(ahead.peek(), Some(' ' | '\t' | '\r')) {
                ahead.next();
            }
            if ahead.peek().is_some_and(|n| n.is_lowercase()) {
                chars = ahead;
                continue;
            }
        }
        if c.is_whitespace() {
            blank = true;
            continue;
        }
        if blank && !out.is_empty() {
            out.push(' ');
        }
        blank = false;
        out.push(c);
    }
    out
}

/// Every (reference, name) pair of a text, distinct and sorted.
pub fn read(text: &str) -> Vec<DoctrineCitation> {
    let t = plain_text(text);
    let linker = &*LINKER;
    let cases = doc_extract(&t, &linker.vocab, &linker.snap, &linker.chrono, None, None).cases;
    let bytes: Vec<usize> = t.char_indices().map(|(b, _)| b).chain([t.len()]).collect();
    let mut spans: Vec<(usize, usize, &'static str, String)> = cases
        .iter()
        .filter_map(|c| {
            let court = ["cc", "ce", "tc"]
                .into_iter()
                .find(|k| c.address.court.as_deref() == Some(*k))?;
            let docket: String = c
                .address
                .docket
                .as_deref()?
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase();
            Some((bytes[c.char_start], bytes[c.char_end], court, docket))
        })
        .collect();
    spans.sort();
    spans.dedup();

    let mut lower: HashMap<String, usize> = HashMap::new();
    for w in t.split(|c: char| !c.is_alphanumeric()) {
        if w.starts_with(char::is_lowercase) {
            *lower.entry(fold(w)).or_default() += 1;
        }
    }

    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < spans.len() {
        let mut j = i + 1;
        while j < spans.len()
            && spans[j - 1].1 <= spans[j].0
            && ENUM.is_match(&t[spans[j - 1].1..spans[j].0])
        {
            j += 1;
        }
        let run = &spans[i..j];
        i = j;
        let Some((date, names)) = reference(&t, run[0].0, run[run.len() - 1].1, &lower) else {
            continue;
        };
        for (_, _, court, docket) in run {
            for name in &names {
                out.insert(DoctrineCitation {
                    court: court.to_string(),
                    date: date.clone(),
                    docket: docket.clone(),
                    name: name.clone(),
                });
            }
        }
    }
    out.into_iter().collect()
}

/// The day a reference writes before its first number, and the names around it.
fn reference(
    t: &str,
    first: usize,
    last: usize,
    lower: &HashMap<String, usize>,
) -> Option<(String, Vec<String>)> {
    let before = &t[floor(t, first.saturating_sub(200))..first];
    let day = DATE.captures_iter(before).last()?;
    let whole = day.get(0)?;
    let gap = GAP.captures(&before[whole.end()..])?;
    let date = parse_date_fr(&format!("{} {} {}", &day[1], &day[2], &day[3]))?.to_string();

    let mut raw: Vec<(&str, Slot)> = Vec::new();
    if let Some(n) = gap.name("name") {
        if !REFUSED_TAIL.is_match(n.as_str()) {
            raw.push((n.as_str(), Slot::Between));
        }
    }
    let tail = &t[last..floor(t, (last + 200).min(t.len()))];
    let ahead = &before[..whole.start()];
    raw.extend(after_name(tail, ahead, lower).map(|n| (n, Slot::After)));
    raw.extend(before_name(ahead).map(|n| (n, Slot::Before)));
    let names: Vec<String> = raw
        .into_iter()
        .filter_map(|(n, slot)| usage_name(n, slot))
        .collect();
    (!names.is_empty()).then_some((date, names))
}

/// The name written after a reference, `ahead` being the text before its day and `lower`
/// the words the text writes in lowercase.
fn after_name<'a>(tail: &'a str, ahead: &str, lower: &HashMap<String, usize>) -> Option<&'a str> {
    if REFUSED_TAIL.is_match(tail) {
        return None;
    }
    let c = AFTER.captures(tail)?;
    let n = c.name("name")?;
    let name = walk(n.as_str(), Slot::After);
    let last_word = name.rsplit(' ').next().unwrap_or_default();
    let open = name.ends_with('.') || GLUE.contains(&fold(last_word).as_str());
    let follows = &tail[n.start() + name.len()..];
    // In « : M. Espagnon, comm. » the M. is the commentator's initial.
    let commentator = name.starts_with("M. ") && REFUSED_TAIL.is_match(follows);
    // « PETIT F., Rép. Sociétés », « M. Clément-Fontaine, « L'exclusion… » »: an author.
    let author = (INITIALLED.is_match(name) && QUOTED_TITLE.is_match(follows))
        || INITIAL_FOLLOWS.is_match(follows);
    // « Association du quartier « Les Hauts de Choiseul » »: the quote goes on with the name.
    let quoted = QUOTE_GOES_ON.is_match(follows);
    // « DMF 20°19 » is a volume; after « Séphora 668 » or « Molex. 263 » a footnote's number.
    let numbered = follows
        .trim_start()
        .starts_with(|c: char| c.is_ascii_digit());
    let volume = numbered && ACRONYM.is_match(name);
    let prose = name.split_whitespace().skip(1).any(|w| {
        let elided = w
            .get(2..)
            .is_some_and(|r| r.starts_with(char::is_uppercase));
        w.starts_with(char::is_lowercase)
            && !GLUE.contains(&fold(w).as_str())
            && !elided
            && !others(w)
    });
    let commented = prose && COMMENTED.is_match(ahead);
    // « n° 02-86182 (Blanchiment) », « : Résolution unilatérale »: a label in common words.
    let label = &c["open"] != "," && common_words(name, lower);
    let journal = !open && JOURNAL_FOLLOWS.is_match(follows);
    // « Mentionné dans les tables du Recueil », « L'Essentiel Droit de la famille »: a name
    // with lowercase words or an article runs to its punctuation, a journal word does not stop it.
    let closed = name.len() == n.as_str().trim_end().len() || name.ends_with('.') || numbered;
    let unclosed = !closed && (prose || ARTICLE.is_match(name));
    (!commentator && !author && !quoted && !commented && !label && !volume && !journal && !unclosed)
        .then_some(n.as_str())
}

/// Every word of a name past a head that names no one alone is one the text writes in
/// lowercase twice at least.
fn common_words(name: &str, lower: &HashMap<String, usize>) -> bool {
    let key = case_name_key(name);
    let head = key.split(' ').next().unwrap_or_default();
    if GENERIC.contains(&head) || ABBREVIATIONS.contains(&head) {
        return false;
    }
    let mut words = key
        .split(' ')
        .filter(|w| {
            w.chars().count() > 1 && !GLUE.contains(w) && !w.chars().all(|c| c.is_ascii_digit())
        })
        .peekable();
    words.peek().is_some() && words.all(|w| lower.get(w).is_some_and(|n| *n >= 2))
}

/// An initial right after a name: « PETIT F., », « Syndicat national C. Justice ».
static INITIAL_FOLLOWS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+\p{Lu}\.?(?:\s*,|\s*$|\s+\p{Lu})").unwrap());

/// « M. Morand », « E.A.Caprioli »: a name opening on an initial.
static INITIALLED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\p{Lu}\.(?:\s|\p{Lu}\.)").unwrap());

static QUOTE_GOES_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*[«"“]"#).unwrap());

static ACRONYM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Lu}{2,6}$").unwrap());

static ARTICLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:La|Le|Les)\s|^L['’]").unwrap());

/// A Cour de cassation publication code: « PBI », « PBR », « FS-P+B ».
static PUBLICATION_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:F[SPD]?[\s-]*)?(?:[PBRI]\s*\+?\s*){1,4}$").unwrap());

/// « l'arrêt SAT (Cass. ass. plén., » : the name touching the reference, `ahead` ending at its day.
fn before_name(ahead: &str) -> Option<&str> {
    let ahead = &ahead[floor(ahead, ahead.len().saturating_sub(250))..];
    let c = BEFORE.captures(ahead)?;
    let keyword = c.get(0).map_or(0, |m| m.start());
    let another_day = c.name("court").is_some_and(|s| DATE.is_match(s.as_str()));
    if another_day || AFTER_A_NUMBER.is_match(&ahead[..keyword]) {
        return None;
    }
    c.name("name").map(|n| n.as_str())
}

/// The side of « A c/ B » a pseudonym hides is dropped, and a name all pseudonym is none.
fn unpseudonymised(name: &str) -> Option<String> {
    // « Mme A. », « consorts X », but neither « et a. » nor an acronym « C.F.T.C. ».
    let hidden = |part: &str| {
        let key = case_name_key(part);
        let words: Vec<&str> = key.split(' ').rev().collect();
        let letters = words.iter().take_while(|w| w.chars().count() == 1).count();
        (1..=2).contains(&letters) && words.get(1) != Some(&"et")
    };
    let Some((start, end)) = versus(name) else {
        return (!hidden(name)).then(|| name.to_string());
    };
    let (left, right) = (&name[..start], &name[end..]);
    match (hidden(left), hidden(right)) {
        (false, false) => Some(name.to_string()),
        (false, true) => Some(left.to_string()),
        (true, false) => Some(right.to_string()),
        (true, true) => None,
    }
}

static VERSUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+(?:[cC]/|c\.|contre)\s+").unwrap());

/// Where « A c/ B » turns to its other party; « Collectif contre les nuisances » has none.
fn versus(name: &str) -> Option<(usize, usize)> {
    VERSUS
        .find_iter(name)
        .find(|m| {
            !m.as_str().trim().eq("contre") || name[m.end()..].starts_with(char::is_uppercase)
        })
        .map(|m| (m.start(), m.end()))
}

/// Where a name is written around its reference.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Between the day and the number, closed on both sides.
    Between,
    /// After the number, closed by the punctuation that follows it.
    After,
    /// Before the reference, after the word that points at it.
    Before,
}

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S+").unwrap());

/// The words that still name, up to a journal word, a number after the reference, a dotted word.
fn walk(raw: &str, slot: Slot) -> &str {
    let mut end = 0;
    for (k, m) in TOKEN.find_iter(raw).enumerate() {
        let w = m.as_str();
        let bare = fold(w.trim_end_matches('.'));
        let journal = w.starts_with(char::is_uppercase)
            && NOT_A_NAME.contains(&bare.as_str())
            && (bare.chars().count() == 1 || !GLUE.contains(&bare.as_str()));
        let after = slot == Slot::After && k > 0;
        let number = after
            && (w.starts_with(|c: char| c.is_ascii_digit())
                || JOURNAL_FOLLOWS.is_match(&raw[m.start()..]));
        if k > 0 && (number || (journal && slot != Slot::Between)) {
            break;
        }
        end = m.end();
        if dotted(w) {
            break;
        }
    }
    &raw[..end]
}

/// A word its own dot closes: neither an initial nor an abbreviation a name carries.
fn dotted(w: &str) -> bool {
    let bare = fold(w.trim_end_matches('.'));
    let letters = bare.chars().filter(|c| c.is_alphabetic()).count();
    w.ends_with('.')
        && letters > 2
        && !ABBREVIATIONS.contains(&bare.as_str())
        && !GENERIC.contains(&bare.as_str())
}

/// « et a. », « et al. », « et autres », « et N. »: the parties a name leaves unnamed.
fn others(w: &str) -> bool {
    let bare = fold(w.trim_end_matches('.'));
    matches!(bare.as_str(), "a" | "al" | "autre" | "autres")
        || (bare.chars().count() == 1 && w.starts_with(char::is_uppercase))
}

/// A written name cut where it stops naming, then held to the forms a party takes.
fn usage_name(raw: &str, slot: Slot) -> Option<String> {
    let raw = raw.trim_start_matches(|c: char| !c.is_alphanumeric());
    let head = fold(raw.split_whitespace().next().unwrap_or_default());
    if slot == Slot::Between
        && ((raw.starts_with(char::is_lowercase) && !GENERIC.contains(&head.trim_end_matches('.')))
            || COURT_ENDS.is_match(raw))
    {
        return None;
    }
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut words: Vec<&str> = walk(raw, slot).split_whitespace().collect();
    // A lone abbreviation its own dot cuts, « Caz. Pal. » or « Jurisp. soc. UIMM », is a journal's,
    // and so are abbreviations alone, « Conf. Nat. ».
    let journal_word = |w: &str| w.ends_with('.') || w.starts_with(char::is_lowercase);
    if (words.len() == 1
        && words[0].ends_with('.')
        && tokens.get(1).is_some_and(|w| journal_word(w)))
        || (words.len() > 1 && words.iter().all(|w| w.ends_with('.')))
    {
        return None;
    }
    loop {
        let n = words.len();
        // « Milhaud c/ Cons. nat. Ordre médecins »: the side its first dot cuts names nobody.
        let cut_side =
            n >= 3 && VERSUS.is_match(&format!(" {} ", words[n - 2])) && dotted(words[n - 1]);
        if cut_side || (n >= 3 && fold(words[n - 2]) == "et" && others(words[n - 1])) {
            words.truncate(n - 2);
        } else if n > 0
            && GLUE.contains(&fold(words[n - 1]).trim_end_matches(['\'', '’']))
            && !(words[n - 1].trim_end_matches('.').chars().count() == 1
                && words[n - 1].starts_with(char::is_uppercase))
        {
            words.pop();
        } else {
            break;
        }
    }
    // « RD rur. », « BJT déc. »: a name does not end on a lowercase abbreviation, a journal does.
    if words
        .last()
        .is_some_and(|w| w.starts_with(char::is_lowercase) && dotted(w))
    {
        return None;
    }
    let first = *words.first()?;
    if first.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let civility = ABBREVIATIONS.contains(&fold(first.trim_end_matches('.')).as_str());
    let initial = first.ends_with('.')
        && !civility
        && (first.trim_end_matches('.').chars().count() <= 2
            || first.split(['.', '-']).all(|p| p.chars().count() <= 1));
    let name = normalise_nom(words.join(" ").trim_end_matches(['.', ',']))?;
    let name = unpseudonymised(&name)?;
    // « Société des Monoprix c/ La société civile immobilière "Résidence Bonaparte" »: the first party.
    let name = match versus(&name) {
        Some((start, _)) if case_name_key(&name).split(' ').count() > MAX_NAME_WORDS => {
            let left = name[..start].trim_end_matches(['.', ',']);
            let mut words: Vec<&str> = left.split_whitespace().collect();
            if words.len() >= 3
                && fold(words[words.len() - 2]) == "et"
                && others(words[words.len() - 1])
            {
                words.truncate(words.len() - 2);
            }
            words.join(" ")
        }
        _ => name,
    };
    let key = case_name_key(&name);
    let mut heads = key.split(' ');
    let head = heads.next().unwrap_or_default();
    // « La Samaritaine », « Le Pen »: an article opens a name when a capital follows it.
    let article = matches!(head, "la" | "le" | "les" | "l")
        && name
            .split([' ', '\'', '’'])
            .nth(1)
            .is_some_and(|w| w.starts_with(char::is_uppercase))
        && heads.next().is_some_and(|w| !NOT_A_NAME.contains(&w));
    // « Louduna is », « pou r », « As sociation »: a word the PDF split leaves a stub.
    let stub = |w: &str| {
        let bare = fold(w);
        bare.chars().count() <= 2
            && bare.chars().all(char::is_alphabetic)
            && !GLUE.contains(&bare.as_str())
            && !ABBREVIATIONS.contains(&bare.as_str())
    };
    let parts: Vec<&str> = name.split_whitespace().collect();
    let split = parts.iter().enumerate().any(|(k, w)| {
        let next_lower = parts.get(k + 1).is_some_and(|n| {
            n.starts_with(char::is_lowercase) && !GLUE.contains(&fold(n).as_str())
        });
        stub(w)
            && (w.starts_with(char::is_lowercase)
                || (next_lower && w.chars().skip(1).all(char::is_lowercase)))
    });
    if initial
        || split
        || PUBLICATION_CODE.is_match(&name)
        || (NOT_A_NAME.contains(&head) && !article)
        || GENERIC.contains(&key.as_str())
        || key.split(' ').count() > MAX_NAME_WORDS
        || key.split(' ').any(|w| NEVER_IN_A_NAME.contains(&w))
        || key.split(' ').all(|w| w.chars().count() == 1 || w == "et")
        || !key.chars().any(char::is_alphabetic)
    {
        return None;
    }
    Some(name)
}

fn floor(t: &str, mut i: usize) -> usize {
    while !t.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// One deposit's name, on the decision its reference resolves to.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub deposit: String,
    pub name: String,
    pub decision: i64,
}

/// When a name is shown rather than only answered to.
#[derive(Debug, Clone, Copy)]
pub struct NameRule {
    /// Deposits that must write it.
    pub deposits: usize,
    /// One deposit is enough when the decision's own text writes the name.
    pub text_confirms: bool,
}

/// What the deposits give a decision: its keys, and the name it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Election {
    pub decision: i64,
    pub name: Option<String>,
    pub keys: Vec<String>,
    /// Deposits behind the shown name, or behind the keys when none is.
    pub deposits: Vec<String>,
}

/// The particles a surname opens on, which stay lowercase at the head of a name: « de Peretti ».
const PARTICLES: &[&str] = &[
    "de", "d", "du", "des", "van", "von", "der", "den", "di", "da", "del", "della",
];

/// A written name as shown: trailing marks cut unless they close what it opened or end an
/// abbreviation, a note call glued to its dot cut, a lowercase head capitalised unless a particle.
fn shown_form(written: &str) -> String {
    let mut name = written.trim_end();
    loop {
        let before = name.len();
        if let Some((head, call)) = name.rsplit_once('.') {
            if !call.is_empty()
                && call.chars().all(|c| c.is_ascii_digit())
                && head.ends_with(char::is_alphabetic)
            {
                name = head;
            }
        }
        if let Some(last) = name.chars().last() {
            if !last.is_alphanumeric() && !stays(name, last) {
                name = name[..name.len() - last.len_utf8()].trim_end();
            }
        }
        if name.len() == before {
            break;
        }
    }
    let head = name.split_whitespace().next().unwrap_or_default();
    let particle = PARTICLES.contains(&fold(head).split(['\'', '’']).next().unwrap_or_default());
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_lowercase() && !particle => c.to_uppercase().chain(chars).collect(),
        _ => name.to_string(),
    }
}

fn stays(name: &str, last: char) -> bool {
    let count = |c: char| name.matches(c).count();
    match last {
        '.' => name[..name.len() - 1].rsplit(' ').next().is_some_and(|w| {
            (1..=3).contains(&w.chars().count()) && w.chars().all(char::is_alphabetic)
        }),
        ')' => count('(') >= count(')'),
        ']' => count('[') >= count(']'),
        '»' => name.contains('«'),
        '”' => name.contains('“'),
        '’' => name.contains('‘'),
        '"' | '\'' => count(last) % 2 == 0,
        _ => false,
    }
}

/// Every key goes to its decision, and a name is shown when `rule` holds. A deposit writes a
/// name when one of its own holds every word of it; paired with several decisions, it writes none.
pub fn elect(
    evidence: &[Evidence],
    rule: NameRule,
    confirmed: &dyn Fn(i64, &str) -> bool,
) -> Vec<Election> {
    let mut targets: HashMap<(&str, String), BTreeSet<i64>> = HashMap::new();
    for e in evidence {
        targets
            .entry((e.deposit.as_str(), case_name_key(&e.name)))
            .or_default()
            .insert(e.decision);
    }
    struct Written<'a> {
        deposit: &'a str,
        name: &'a str,
        key: String,
        alone: bool,
    }
    let mut by_decision: BTreeMap<i64, Vec<Written>> = BTreeMap::new();
    for e in evidence {
        let key = case_name_key(&e.name);
        if key.is_empty() {
            continue;
        }
        let alone = targets[&(e.deposit.as_str(), key.clone())].len() == 1;
        by_decision.entry(e.decision).or_default().push(Written {
            deposit: &e.deposit,
            name: &e.name,
            key,
            alone,
        });
    }

    let words =
        |k: &str| -> HashSet<String> { name_words(k).into_iter().map(str::to_string).collect() };
    by_decision
        .into_iter()
        .map(|(decision, written)| {
            let keys: Vec<String> = written
                .iter()
                .map(|w| w.key.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut best: Option<((usize, usize), &str, BTreeSet<&str>)> = None;
            for key in &keys {
                let wanted = words(key);
                if wanted.is_empty() {
                    continue;
                }
                let behind: BTreeSet<&str> = written
                    .iter()
                    .filter(|w| w.alone && words(&w.key).is_superset(&wanted))
                    .map(|w| w.deposit)
                    .collect();
                let shown = behind.len() >= rule.deposits
                    || (rule.text_confirms && !behind.is_empty() && confirmed(decision, key));
                let rank = (behind.len(), wanted.len());
                if shown && best.as_ref().is_none_or(|b| rank > b.0) {
                    best = Some((rank, key.as_str(), behind));
                }
            }
            let (name, deposits) = match best {
                Some((_, key, behind)) => {
                    let mut forms: BTreeMap<String, usize> = BTreeMap::new();
                    for w in written.iter().filter(|w| w.key == key) {
                        *forms.entry(shown_form(w.name)).or_default() += 1;
                    }
                    let form = forms
                        .iter()
                        .max_by_key(|(f, n)| (**n, std::cmp::Reverse(*f)))
                        .map(|(f, _)| f.clone());
                    (form, behind.into_iter().map(str::to_string).collect())
                }
                None => (
                    None,
                    written
                        .iter()
                        .map(|w| w.deposit)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
            };
            Election {
                decision,
                name,
                keys,
                deposits,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(text: &str) -> Vec<(String, String, String, String)> {
        read(text)
            .into_iter()
            .map(|c| (c.court, c.date, c.docket, c.name))
            .collect()
    }

    fn one(court: &str, date: &str, docket: &str, name: &str) -> (String, String, String, String) {
        (court.into(), date.into(), docket.into(), name.into())
    }

    const CHEVASSUS: &str = "Chevassus-Marche";

    #[test]
    fn the_name_after_the_number() {
        assert_eq!(
            pairs("V. Cass. com., 24 nov. 1998, n° 96-18.357, Chevassus-Marche, Bull. civ. IV, n° 277."),
            vec![one("cc", "1998-11-24", "9618357", CHEVASSUS)]
        );
        assert_eq!(
            pairs("Cass. com., 24 nov. 1998, n° 96-18.357 (Chevassus-Marche), qui retient"),
            vec![one("cc", "1998-11-24", "9618357", CHEVASSUS)]
        );
    }

    #[test]
    fn the_name_between_the_day_and_the_number() {
        assert_eq!(
            pairs("V. CE, Ass., 20 octobre 1989, Nicolo, n° 108243, Rec. p. 190."),
            vec![one("ce", "1989-10-20", "108243", "Nicolo")]
        );
    }

    #[test]
    fn the_usage_name_touching_its_reference() {
        assert_eq!(
            pairs("En toile de fond, l'arrêt SAT (Cass. ass. plén., 8 déc. 2000, n° 97-44219, Bull. ass. plén., n° 11) et son"),
            vec![one("cc", "2000-12-08", "9744219", "SAT")]
        );
    }

    #[test]
    fn a_usage_name_away_from_its_reference_is_not_read() {
        assert!(pairs("l'arrêt SAT, dont on sait la portée, a été suivi. Cass. soc., 8 déc. 2000, n° 97-44.219, Bull.").is_empty());
    }

    #[test]
    fn a_commentator_or_a_journal_is_no_party() {
        for text in [
            "Cass. 1re civ., 3 mai 2000, n° 98-11.381, note J. Ghestin.",
            "Cass. com., 24 nov. 1998, n° 96-18.357, obs. Mestre.",
            "CE, 3 mars 2004, concl. Prada Bordenave, n° 241151.",
            "Cass. com., 24 nov. 1998, n° 96-18.357, JCP G 1999, II, 10210.",
            "Cass. com., 24 nov. 1998, n° 96-18.357, D. 1999, p. 58.",
            "Cass. com., 24 nov. 1998, n° 96-18.357, RTD civ. 1999, p. 124.",
        ] {
            assert!(pairs(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn a_journal_a_pseudonym_or_a_bare_head_is_no_name() {
        for text in [
            "Cass. soc. 28 juin 2018, n° 17-16.499, BJT septembre 2018, p. 43.",
            "Com. 26 Mai 2009, n° 08-13.839, RLDA, n° 2675, 2010, 45.",
            "Cass. civ. 2e, 30 janv. 2014, n° 12-24.145 : J.-M. Bruguière, comm. ss déc. préc.",
            "CE 17 février 2011, Raymond A., n°344445 ; la suite.",
            "CE, 26 mars 1990, Assoc. SOS-Défense, n° 39734 ; CE, 3 mai 1991, n° 1, Assoc. « X ».",
            "Cass. crim., 6 mai 2014, 2 arrêts, N° 13-81.406 et n° 12-88.354.",
        ] {
            assert!(
                pairs(text).iter().all(|p| ![
                    "BJT",
                    "RLDA",
                    "J.-M. Bruguière",
                    "Raymond A.",
                    "Assoc.",
                    "2 arrêts"
                ]
                .contains(&p.3.as_str())),
                "{text}: {:?}",
                pairs(text)
            );
        }
    }

    #[test]
    fn the_pseudonymised_side_drops_and_the_other_stays() {
        assert_eq!(
            pairs("Cass. com., 26 nov. 2003, Société Alain Manoukian c/ consorts X, n° 00-10.243 et n° 00-10.949, publié au Bulletin"),
            vec![
                one("cc", "2003-11-26", "0010243", "Société Alain Manoukian"),
                one("cc", "2003-11-26", "0010949", "Société Alain Manoukian"),
            ]
        );
    }

    #[test]
    fn a_name_after_another_reference_number_is_not_the_next_ones() {
        let got = pairs("Cass. com., 6 mai 2003, pourvoi n° 00-18.192, arrêt Ducasse Cass. com., 7 janv. 2004, pourvoi n° 02-11.014, Bull.");
        assert_eq!(got, vec![one("cc", "2003-05-06", "0018192", "Ducasse")]);
    }

    #[test]
    fn every_number_of_a_reference_takes_its_name() {
        assert_eq!(
            pairs("Cass. com., 26 nov. 2003, n° 00-10.243 et 00-10.949, Manoukian, Bull. civ. IV, n° 186."),
            vec![
                one("cc", "2003-11-26", "0010243", "Manoukian"),
                one("cc", "2003-11-26", "0010949", "Manoukian"),
            ]
        );
    }

    #[test]
    fn a_reference_without_a_number_gives_nothing() {
        assert!(
            pairs("Cass. ass. plén., 29 mars 1991, Blieck, Bull. ass. plén., n° 1.").is_empty()
        );
    }

    fn names(text: &str) -> Vec<String> {
        read(text).into_iter().map(|c| c.name).collect()
    }

    #[test]
    fn a_name_after_the_number_runs_to_its_punctuation() {
        assert_eq!(
            names("Cass. soc., 25 janv. 2006, n° 04-17.000, Société Bio-Rad laboratoires, Bull. civ. V, n° 26."),
            vec!["Société Bio-Rad laboratoires"]
        );
    }

    #[test]
    fn an_article_opens_a_name_a_capital_follows() {
        assert_eq!(
            names("concl. sous Cass. soc., 13 février 1997, La Samaritaine, n° 95-16648, Dr. soc. 1997, p. 249."),
            vec!["La Samaritaine"]
        );
    }

    #[test]
    fn a_label_in_the_words_of_the_text_is_no_name() {
        let text = "Le blanchiment suppose une infraction. Crim. 25 juin 2003, n° 02-86182 (Blanchiment). Crim. 6 mai 2009, n° 08-84107 (Prescription corruption). La prescription de la corruption court, comme celle du blanchiment, dès la prescription acquise.";
        assert!(names(text).is_empty(), "{:?}", names(text));
    }

    #[test]
    fn a_title_under_a_commentary_is_no_name() {
        assert!(names("obs. sous Cass. soc., 4 mars 2008, n° 06-45221, Clause pénale et clause de garantie d'emploi, JCP S 2008").is_empty());
    }

    #[test]
    fn a_split_word_initials_or_a_publication_code_is_no_name() {
        for text in [
            "CE, 26 juillet 2011, As sociation FNATH, n° 337065 Lebon ; AJDA 2010",
            "Cass. 1re civ., 3 mai 2000, n° 98-11.381, B et M, Bull.",
            "Cass. soc., 12 mars 2020, n° 18-12.345, FS-P+B, Bull.",
            "CE, 28 avr. 2006, n° 280197, BJCP, no 47, p. 268.",
        ] {
            assert!(names(text).is_empty(), "{text}: {:?}", names(text));
        }
    }

    #[test]
    fn a_chamber_is_no_name() {
        let text =
            "Arrêt n°466 du 5 juillet 2023 - Première chambre civile, Pourvoi n° 22-13.457, p. 25";
        assert!(!names(text).contains(&"Première chambre civile".to_string()));
    }

    #[test]
    fn a_side_its_dot_cuts_drops_and_an_overlong_pair_keeps_its_first_party() {
        assert_eq!(
            names("Cass. 1re civ., 3 mai 2000, n° 98-11.381, Milhaud c/ Cons. nat. Ordre médecins, Bull."),
            vec!["Milhaud"]
        );
        assert_eq!(
            names("Cass. 3e civ., 9 févr. 1972, n° 70-12.345, Société des Monoprix c/ La société civile immobilière Résidence Bonaparte et autres ; Bull."),
            vec!["Société des Monoprix"]
        );
    }

    #[test]
    fn a_word_cut_at_the_line_end_is_rejoined() {
        assert_eq!(
            plain_text("la juris-\nprudence\u{ad} Saint-\nÉtienne"),
            "la jurisprudence Saint- Étienne"
        );
    }

    fn ev(deposit: &str, name: &str, decision: i64) -> Evidence {
        Evidence {
            deposit: deposit.into(),
            name: name.into(),
            decision,
        }
    }

    const TWO: NameRule = NameRule {
        deposits: 2,
        text_confirms: false,
    };

    #[test]
    fn one_deposit_gives_keys_two_give_the_name() {
        let none = |_: i64, _: &str| false;
        let lone = elect(&[ev("a", "Manoukian", 1)], TWO, &none);
        assert_eq!(lone[0].name, None);
        assert_eq!(lone[0].keys, vec!["manoukian"]);
        let both = elect(
            &[
                ev("a", "Manoukian", 1),
                ev("b", "Sté Pommery c/ Manoukian", 1),
            ],
            TWO,
            &none,
        );
        assert_eq!(both[0].name.as_deref(), Some("Manoukian"));
        assert_eq!(both[0].keys, vec!["manoukian", "ste pommery c manoukian"]);
    }

    #[test]
    fn a_name_one_deposit_gives_several_decisions_is_a_key_alone() {
        let none = |_: i64, _: &str| false;
        let out = elect(
            &[
                ev("a", "Samaritaine", 1),
                ev("a", "Samaritaine", 2),
                ev("b", "Samaritaine", 1),
            ],
            TWO,
            &none,
        );
        assert!(out.iter().all(|e| e.name.is_none()));
        assert!(out.iter().all(|e| e.keys == vec!["samaritaine"]));
    }

    #[test]
    fn the_decision_text_confirms_a_lone_deposit() {
        let rule = NameRule {
            deposits: 2,
            text_confirms: true,
        };
        let writes = |d: i64, k: &str| d == 1 && k == "expovit";
        let out = elect(
            &[ev("a", "Expovit", 1), ev("a", "Lebrun", 2)],
            rule,
            &writes,
        );
        assert_eq!(out[0].name.as_deref(), Some("Expovit"));
        assert_eq!(out[1].name, None);
    }

    #[test]
    fn a_name_is_shown_without_its_debris_and_with_its_capital() {
        for (written, shown) in [
            (
                "Société Treuils et Grues Labor •",
                "Société Treuils et Grues Labor",
            ),
            ("Jammet.-", "Jammet"),
            ("SCI Cristal Parc\"", "SCI Cristal Parc"),
            ("Société Ocréal.81", "Société Ocréal"),
            ("commune d'Hyères", "Commune d'Hyères"),
            ("société France Loisirs", "Société France Loisirs"),
            ("préfet des Alpes-Maritimes", "Préfet des Alpes-Maritimes"),
            (
                "syndicat Sud transports urbains 31 c. Semvat",
                "Syndicat Sud transports urbains 31 c. Semvat",
            ),
            ("de Peretti", "de Peretti"),
            ("d'Artagnan", "d'Artagnan"),
            ("SA Méribel 92", "SA Méribel 92"),
            (
                "Association ‘‘Notre affaire à tous’’",
                "Association ‘‘Notre affaire à tous’’",
            ),
            (
                "Société \" Les Rapides de la Côte-d'Or \"",
                "Société \" Les Rapides de la Côte-d'Or \"",
            ),
            ("Société Bic (France)", "Société Bic (France)"),
            ("UFC Que Choisir ?", "UFC Que Choisir"),
            (
                "SA Comilog international et a.",
                "SA Comilog international et a.",
            ),
        ] {
            assert_eq!(shown_form(written), shown, "{written}");
        }
        let none = |_: i64, _: &str| false;
        let out = elect(
            &[
                ev("a", "Société Ocréal.81", 1),
                ev("b", "Société Ocréal 81", 1),
            ],
            TWO,
            &none,
        );
        assert_eq!(out[0].name.as_deref(), Some("Société Ocréal"));
        assert_eq!(out[0].keys, vec!["societe ocreal 81"]);
    }
}
