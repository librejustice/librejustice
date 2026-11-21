//! Construction du prompt de résumé + nettoyage de sortie.
//!
//! PUR : pas d'appel LLM ici (l'I/O réseau vit dans `lj-ingest`). On expose le
//! prompt versionné, le découpage tête/queue de l'input, et le nettoyage de la
//! sortie modèle.

use std::sync::LazyLock;

use regex::Regex;

mod context;
mod input;

use context::{NAME_MAX, PARTIES};

pub use context::{classification, Header, Provenance, CONTEXT_FIELDS, CONTEXT_SOURCES};
pub use input::{body_text_from_full_text, decision_title};

/// Prompt generation in `decisions.summary_prompt_version`. ORDERED, not a text
/// identifier: staleness is a `<`, so two live prompts of equivalent quality
/// share a number and are told apart by [`SummaryEngine`].
///
/// The collage shares the number with the rewrite. A summary is stale below
/// [`due_generation`] of its text's language, a collage never.
pub const SUMMARY_PROMPT_VERSION: i16 = 12;

/// Generation 12 changed only the language of a summary, which a French text's already had.
pub const FRENCH_SUMMARY_GENERATION: i16 = 11;

/// The generation below which the summary of a text in `lang` is stale.
pub fn due_generation(lang: &str) -> i16 {
    if lang == "fr" {
        FRENCH_SUMMARY_GENERATION
    } else {
        SUMMARY_PROMPT_VERSION
    }
}

/// Supreme and European courts, then appeal, then the authorities, then first
/// instance. A type left out comes last.
pub const JURISDICTION_TIERS: &[(&str, i32)] = &[
    ("CONSTIT", 0),
    ("TC", 0),
    ("CE", 0),
    ("CC", 0),
    ("CJUE", 0),
    ("CEDH", 0),
    ("CAA", 1),
    ("CA", 1),
    ("CNIL", 2),
    ("CNDA", 2),
    ("DDD", 2),
    ("ADLC", 2),
    ("AMF", 2),
    ("COMEU", 2),
    ("TA", 3),
    ("TJ", 3),
    ("TCOM", 3),
];

/// Prompt système de résumé (texte intégral, source de vérité).
pub const SUMMARY_PROMPT: &str = include_str!("../data/summary_prompt.txt");

/// Not interchangeable with [`SUMMARY_PROMPT`]: its per-sentence bands are held
/// to the character by Flash-Lite and read as indicative by mistral-small.
pub const SUMMARY_PROMPT_GEMINI: &str = include_str!("../data/summary_prompt_gemini.txt");

/// Names what the third sentence is for: what the rule holds over the dispute, never
/// the procedural disposition. Model-agnostic, mistral-small included.
pub const SUMMARY_PROMPT_SOLUTION: &str = include_str!("../data/summary_prompt_solution.txt");

/// One call where the others take three: the selection happens inside the
/// reasoning rather than in a separate sorting pass. Written for luna, where it outscores
/// the prompt it replaces; its budget of 235 characters per point of law holds luna on the
/// reference length, which the same text at 350 overshoots for no better substance.
pub const SUMMARY_PROMPT_MONO: &str = include_str!("../data/summary_prompt_mono.txt");

/// The same text at 350 characters per point of law, which is what the two
/// `flash-lite` need: given one instruction gemini writes shorter than luna, and this
/// budget brings it to the reference length. gpt-6-luna at `low` reads it too, where it
/// outscores the 235 one and still writes under the reference length.
pub const SUMMARY_PROMPT_MONO_WIDE: &str = include_str!("../data/summary_prompt_mono_wide.txt");

/// The court's own commentary carries the law, so the model writes only what it
/// lacks: an opening sentence and an outcome clause, one per line, the second
/// often empty. [`assemble_collage`] pastes the commentary between them.
///
/// Its angles are bracketed patterns and never sentences: a literal example gets
/// copied onto the wrong court, where a pattern is never pasted and leaves the
/// outcome clause written more often.
pub const SUMMARY_PROMPT_COLLAGE: &str = include_str!("../data/summary_prompt_collage.txt");

/// The collage text for gpt-6-luna, which given the one above runs its opening past
/// the 160 characters of the meta and drops the outcome clause more often.
pub const SUMMARY_PROMPT_COLLAGE_TIGHT: &str =
    include_str!("../data/summary_prompt_collage_tight.txt");

/// [`SUMMARY_PROMPT_MONO`] at 140 characters per point of law, for `claude-haiku-5-5`,
/// which writes a little longer than gpt-6 on the wide text and ties against it.
pub const SUMMARY_PROMPT_MONO_HAIKU: &str = include_str!("../data/summary_prompt_mono_haiku.txt");

/// [`SUMMARY_PROMPT_MONO_WIDE`] answering its opening on one line and its points of law
/// on the next, which [`assemble_rewrite`] stores apart, and reading the header
/// [`Header::rewrite_input`] writes. The outcome stays where the points put it: a line of
/// its own took a point's room.
pub const SUMMARY_PROMPT_PARTS: &str = include_str!("../data/summary_prompt_parts.txt");

/// [`SUMMARY_PROMPT_PARTS`] in one block, its opening the first sentence [`assemble_rewrite`]
/// cuts. On a line of its own the opening shortened gpt-6's and Gemini's bodies outside
/// cassation and the Conseil d'Etat; in one block gpt-6 scores there at least as well as
/// generation 10.
pub const SUMMARY_PROMPT_BLOCK: &str = include_str!("../data/summary_prompt_block.txt");

/// [`SUMMARY_PROMPT_BLOCK`] on the budget of [`SUMMARY_PROMPT_MONO_HAIKU`]: in two lines Haiku
/// reasons longer and a summary costs more than generation 10's, in one block less, both
/// held to the same form.
pub const SUMMARY_PROMPT_BLOCK_HAIKU: &str = include_str!("../data/summary_prompt_block_haiku.txt");

/// [`SUMMARY_PROMPT_BLOCK`] in English, writing in the language of the text it reads.
pub const SUMMARY_PROMPT_BLOCK_EN: &str = include_str!("../data/summary_prompt_block_en.txt");

/// [`SUMMARY_PROMPT_BLOCK_HAIKU`] in English, its count held to the points a court indexes.
/// At `high`, the only level at which Haiku reasons under English instructions.
pub const SUMMARY_PROMPT_BLOCK_HAIKU_EN: &str =
    include_str!("../data/summary_prompt_block_haiku_en.txt");

/// The languages a summary is written in, by the ISO 639-1 code of the text it reads,
/// and the English name a prompt's `{target_language}` takes.
pub const SUMMARY_LANGUAGES: [(&str, &str); 25] = [
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("en", "English"),
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("el", "Greek"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("ga", "Irish"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mt", "Maltese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sv", "Swedish"),
];

/// [`SUMMARY_PROMPT_COLLAGE_TIGHT`] reading the header [`Header::collage_input`] writes.
pub const SUMMARY_PROMPT_COLLAGE_HEADED: &str =
    include_str!("../data/summary_prompt_collage_headed.txt");

/// [`SUMMARY_PROMPT_COLLAGE_HEADED`] in English, writing in the language of the text it reads.
pub const SUMMARY_PROMPT_COLLAGE_HEADED_EN: &str =
    include_str!("../data/summary_prompt_collage_headed_en.txt");

/// Reasoning level asked of the model, frozen by the engine like the prompt.
///
/// `AV-M2` on gemini-3.5-flash-lite at `high` outscores the shipped prompt; nothing
/// says the same text holds at the door's default, so the level travels with the pair
/// or the bench describes another system than the one that writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    /// The door's own default: no field is sent.
    Default,
    /// Refused by Gemini with a 400, which only knows `minimal` and above.
    None,
    Low,
    Medium,
    High,
}

impl Thinking {
    /// Wire value of the level, `None` when the field is left out.
    pub fn wire(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::None => Some("none"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }
}

/// The (prompt, model, reasoning) triple behind a summary, persisted in
/// `decisions.summary_engine`. `prompt_version` derives from it, so the two
/// columns cannot drift apart.
///
/// It also says what the summary was written from: a prompt shown the court's
/// own commentary carries a code of its own and never takes a decision that
/// carries none. One falling back to the plain text where the commentary is
/// missing would stop saying which summaries came from the court's words.
///
/// Codes 1, 4, 5, 14 and 23 were Mistral's: no stored row carries them, and they
/// are not reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum SummaryEngine {
    Gemini31FlashLiteV9 = 2,
    Gemini35FlashLiteV9 = 3,
    LunaMonoV9 = 6,
    LunaCollageV10 = 7,
    Gemini31MonoV10 = 8,
    Gemini35MonoV10 = 9,
    LunaMonoV10 = 10,
    LunaCollageV11 = 11,
    Luna6MonoV10 = 12,
    Luna6CollageV10 = 13,
    Haiku55MonoBatchV10 = 15,
    Haiku55MonoCliV10 = 16,
    Gemini31PartsV11 = 17,
    Gemini35PartsV11 = 18,
    Luna6PartsV11 = 19,
    Luna6CollageHeadedV11 = 20,
    Haiku55BlockBatchV11 = 21,
    Haiku55BlockCliV11 = 22,
    Luna6BlockV11 = 24,
    Gemini31BlockV11 = 25,
    Gemini35BlockV11 = 26,
    Luna6BlockV12 = 28,
    Haiku55BlockBatchV12 = 29,
    Haiku55BlockCliV12 = 30,
    Gemini31BlockV12 = 31,
    Gemini35BlockV12 = 32,
    Luna6CollageHeadedV12 = 33,
}

impl SummaryEngine {
    /// Every engine that pastes the court's commentary, whatever its generation.
    pub const COLLAGES: [Self; 5] = [
        Self::LunaCollageV10,
        Self::LunaCollageV11,
        Self::Luna6CollageV10,
        Self::Luna6CollageHeadedV11,
        Self::Luna6CollageHeadedV12,
    ];

    pub fn code(self) -> i16 {
        self as i16
    }

    pub fn prompt_version(self) -> i16 {
        match self {
            Self::Gemini31FlashLiteV9 | Self::Gemini35FlashLiteV9 | Self::LunaMonoV9 => 9,
            Self::LunaCollageV10
            | Self::Gemini31MonoV10
            | Self::Gemini35MonoV10
            | Self::LunaMonoV10
            | Self::LunaCollageV11
            | Self::Luna6MonoV10
            | Self::Luna6CollageV10
            | Self::Haiku55MonoBatchV10
            | Self::Haiku55MonoCliV10 => 10,
            Self::Gemini31PartsV11
            | Self::Gemini35PartsV11
            | Self::Luna6PartsV11
            | Self::Luna6CollageHeadedV11
            | Self::Haiku55BlockBatchV11
            | Self::Haiku55BlockCliV11
            | Self::Luna6BlockV11
            | Self::Gemini31BlockV11
            | Self::Gemini35BlockV11 => 11,
            Self::Luna6BlockV12
            | Self::Haiku55BlockBatchV12
            | Self::Haiku55BlockCliV12
            | Self::Gemini31BlockV12
            | Self::Gemini35BlockV12
            | Self::Luna6CollageHeadedV12 => 12,
        }
    }

    pub fn model(self) -> &'static str {
        match self {
            Self::Gemini31FlashLiteV9
            | Self::Gemini31MonoV10
            | Self::Gemini31PartsV11
            | Self::Gemini31BlockV11
            | Self::Gemini31BlockV12 => "gemini-3.1-flash-lite",
            Self::Gemini35FlashLiteV9
            | Self::Gemini35MonoV10
            | Self::Gemini35PartsV11
            | Self::Gemini35BlockV11
            | Self::Gemini35BlockV12 => "gemini-3.5-flash-lite",
            Self::LunaMonoV9 | Self::LunaCollageV10 | Self::LunaMonoV10 | Self::LunaCollageV11 => {
                "gpt-5.6-luna"
            }
            Self::Luna6MonoV10
            | Self::Luna6CollageV10
            | Self::Luna6PartsV11
            | Self::Luna6CollageHeadedV11
            | Self::Luna6BlockV11
            | Self::Luna6BlockV12
            | Self::Luna6CollageHeadedV12 => "gpt-6-luna",
            Self::Haiku55MonoBatchV10
            | Self::Haiku55MonoCliV10
            | Self::Haiku55BlockBatchV11
            | Self::Haiku55BlockCliV11
            | Self::Haiku55BlockBatchV12
            | Self::Haiku55BlockCliV12 => "claude-haiku-5-5",
        }
    }

    /// Reasoning level the bench measured this pair at.
    pub fn thinking(self) -> Thinking {
        match self {
            Self::Gemini31MonoV10
            | Self::Gemini35MonoV10
            | Self::Gemini31PartsV11
            | Self::Gemini35PartsV11
            | Self::Gemini31BlockV11
            | Self::Gemini35BlockV11
            | Self::Gemini31BlockV12
            | Self::Gemini35BlockV12
            | Self::Haiku55BlockBatchV12
            | Self::Haiku55BlockCliV12 => Thinking::High,
            Self::Luna6MonoV10
            | Self::Luna6CollageV10
            | Self::Haiku55MonoBatchV10
            | Self::Haiku55MonoCliV10
            | Self::Luna6PartsV11
            | Self::Luna6CollageHeadedV11
            | Self::Luna6BlockV11
            | Self::Haiku55BlockBatchV11
            | Self::Haiku55BlockCliV11
            | Self::Luna6BlockV12
            | Self::Luna6CollageHeadedV12 => Thinking::Low,
            Self::LunaMonoV9 | Self::LunaCollageV10 | Self::LunaMonoV10 | Self::LunaCollageV11 => {
                Thinking::None
            }
            Self::Gemini31FlashLiteV9 | Self::Gemini35FlashLiteV9 => Thinking::Default,
        }
    }

    /// The prompt is a property of the engine, never a global: a model given the
    /// other model's prompt writes something no bench ever measured. A retired
    /// code returns the current text of its slot and never generates again.
    pub fn prompt(self) -> &'static str {
        match self {
            Self::Gemini31FlashLiteV9 => SUMMARY_PROMPT_GEMINI,
            Self::Gemini35FlashLiteV9 => SUMMARY_PROMPT_SOLUTION,
            Self::LunaMonoV9 | Self::LunaMonoV10 => SUMMARY_PROMPT_MONO,
            Self::Gemini31MonoV10 | Self::Gemini35MonoV10 | Self::Luna6MonoV10 => {
                SUMMARY_PROMPT_MONO_WIDE
            }
            Self::LunaCollageV10 | Self::LunaCollageV11 => SUMMARY_PROMPT_COLLAGE,
            Self::Luna6CollageV10 => SUMMARY_PROMPT_COLLAGE_TIGHT,
            Self::Haiku55MonoBatchV10 | Self::Haiku55MonoCliV10 => SUMMARY_PROMPT_MONO_HAIKU,
            Self::Gemini31PartsV11 | Self::Gemini35PartsV11 | Self::Luna6PartsV11 => {
                SUMMARY_PROMPT_PARTS
            }
            Self::Luna6CollageHeadedV11 => SUMMARY_PROMPT_COLLAGE_HEADED,
            Self::Haiku55BlockBatchV11 | Self::Haiku55BlockCliV11 => SUMMARY_PROMPT_BLOCK_HAIKU,
            Self::Luna6BlockV11 | Self::Gemini31BlockV11 | Self::Gemini35BlockV11 => {
                SUMMARY_PROMPT_BLOCK
            }
            Self::Luna6BlockV12 | Self::Gemini31BlockV12 | Self::Gemini35BlockV12 => {
                SUMMARY_PROMPT_BLOCK_EN
            }
            Self::Haiku55BlockBatchV12 | Self::Haiku55BlockCliV12 => SUMMARY_PROMPT_BLOCK_HAIKU_EN,
            Self::Luna6CollageHeadedV12 => SUMMARY_PROMPT_COLLAGE_HEADED_EN,
        }
    }

    /// Whether this engine writes the summary of a text in `lang`: a prompt that names
    /// no `{target_language}` writes French.
    pub fn writes(self, lang: &str) -> bool {
        if self.prompt().contains("{target_language}") {
            SUMMARY_LANGUAGES.iter().any(|(code, _)| *code == lang)
        } else {
            lang == "fr"
        }
    }

    /// The prompt sent for a text in `lang`, `None` for a language this engine does not write.
    pub fn system_prompt(self, lang: &str) -> Option<String> {
        let (_, name) = SUMMARY_LANGUAGES
            .iter()
            .find(|(code, _)| *code == lang)
            .filter(|_| self.writes(lang))?;
        Some(self.prompt().replace("{target_language}", name))
    }

    /// Level of the one call more an answer gets when [`wants_second_pass`] asks for it.
    pub fn second_pass(self) -> Option<Thinking> {
        match self {
            Self::Haiku55MonoBatchV10
            | Self::Haiku55MonoCliV10
            | Self::Haiku55BlockBatchV11
            | Self::Haiku55BlockCliV11 => Some(Thinking::Medium),
            Self::Haiku55BlockBatchV12 | Self::Haiku55BlockCliV12 => Some(Thinking::High),
            _ => None,
        }
    }

    pub fn from_code(code: i16) -> Option<Self> {
        Some(match code {
            2 => Self::Gemini31FlashLiteV9,
            3 => Self::Gemini35FlashLiteV9,
            6 => Self::LunaMonoV9,
            7 => Self::LunaCollageV10,
            8 => Self::Gemini31MonoV10,
            9 => Self::Gemini35MonoV10,
            10 => Self::LunaMonoV10,
            11 => Self::LunaCollageV11,
            12 => Self::Luna6MonoV10,
            13 => Self::Luna6CollageV10,
            15 => Self::Haiku55MonoBatchV10,
            16 => Self::Haiku55MonoCliV10,
            17 => Self::Gemini31PartsV11,
            18 => Self::Gemini35PartsV11,
            19 => Self::Luna6PartsV11,
            20 => Self::Luna6CollageHeadedV11,
            21 => Self::Haiku55BlockBatchV11,
            22 => Self::Haiku55BlockCliV11,
            24 => Self::Luna6BlockV11,
            25 => Self::Gemini31BlockV11,
            26 => Self::Gemini35BlockV11,
            28 => Self::Luna6BlockV12,
            29 => Self::Haiku55BlockBatchV12,
            30 => Self::Haiku55BlockCliV12,
            31 => Self::Gemini31BlockV12,
            32 => Self::Gemini35BlockV12,
            33 => Self::Luna6CollageHeadedV12,
            _ => return None,
        })
    }
}

// --- Input -------------------------------------------------------------------
//
// The full text up to the cap. Past it, the head (parties, grounds, reasons) and
// the tail (operative part), never the head alone, which inverts the outcome.

const SUMMARY_INPUT_CAP: usize = 80_000;
const SUMMARY_INPUT_HEAD: usize = 56_000;
const SUMMARY_INPUT_TAIL: usize = 24_000;
const ELISION: &str = "\n[...]\n";

/// Borne l'input modèle : intégral si `≤ cap`, sinon tête + queue avec élision.
///
/// Découpage sur frontières de caractères (code points), pas d'octets.
pub fn build_summary_input(body_text: &str) -> String {
    let char_len = body_text.chars().count();
    if char_len <= SUMMARY_INPUT_CAP {
        return body_text.to_string();
    }
    let head: String = body_text.chars().take(SUMMARY_INPUT_HEAD).collect();
    let tail: String = body_text
        .chars()
        .skip(char_len - SUMMARY_INPUT_TAIL)
        .collect();
    format!("{head}{ELISION}{tail}")
}

// --- Second pass ------------------------------------------------------------

/// What the meta description serves of the first sentence.
const FIRST_SENTENCE_MAX: usize = 160;

static CODE_BRACKET_OR_ELLIPSIS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]*\]|\b[A-Z]\.\.\.").unwrap());
static CODE_AFTER_CIVILITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:M\.|Mme|Mlle)\s+[A-Z]").unwrap());

/// On Haiku at `low`, the call that skipped its reasoning is the
/// one that names a party by its anonymisation code or opens past the meta description.
pub fn wants_second_pass(answer: &str, thinking_tokens: u64) -> bool {
    thinking_tokens == 0
        || names_by_code(answer)
        || first_sentence_chars(answer) > FIRST_SENTENCE_MAX
}

pub(crate) fn names_by_code(text: &str) -> bool {
    CODE_BRACKET_OR_ELLIPSIS.is_match(text)
        || CODE_AFTER_CIVILITY.find_iter(text).any(|m| {
            !text[m.end()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
}

fn first_sentence_chars(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if !matches!(c, '.' | '?' | '!') {
            continue;
        }
        let rest = &chars[i + 1..];
        let blank = rest.iter().take_while(|c| c.is_whitespace()).count();
        match rest.get(blank) {
            None => return i + 1,
            Some(next) if blank > 0 && matches!(next, 'A'..='Z' | 'À'..='Ö' | 'Ø'..='Þ') => {
                return i + 1
            }
            _ => {}
        }
    }
    chars.len()
}

// --- Collage of the court's own commentary ---------------------------------

/// Written body required after the nomenclature chain. Below it the chain is all
/// the commentary carries and there is nothing to paste.
pub const COLLAGE_PROSE_MIN: usize = 120;

/// Separators from which a head reads as a nomenclature chain and not a sentence.
const CHAIN_SEGMENTS: usize = 2;

/// Recueil footnote mark, always against the period: `... disposition (1).`
static FOOTNOTE_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d\)(\s*\.)").unwrap());
/// Standalone nomenclature enumerator, never `(CGI)` nor `C.G.I.)`.
static ENUMERATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)[0-9a-z]\)\s").unwrap());
static TYPEWRITER_QUOTES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\"([^\"]{2,}?)\"").unwrap());
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// The commentary stripped of its recueil apparatus, ready to paste verbatim.
///
/// The apparatus is a Conseil d'Etat trait: a cassation summary needs nothing, while an
/// analysis may open on a nomenclature chain and carry enumerators, footnote marks and
/// typewriter quotes.
pub fn commentary_prose(commentary: &str) -> String {
    let trimmed = commentary.trim();
    let body = match trimmed.split_once("\n\n") {
        Some((head, rest))
            if head.matches(" - ").count() >= CHAIN_SEGMENTS
                && rest.trim().chars().count() > COLLAGE_PROSE_MIN =>
        {
            rest
        }
        _ => trimmed,
    };
    let body = WHITESPACE_RUN.replace_all(body, " ");
    let body = FOOTNOTE_MARK.replace_all(&body, "$1");
    let mut body = body.into_owned();
    while ENUMERATOR.is_match(&body) {
        body = ENUMERATOR.replace_all(&body, "$1").into_owned();
    }
    let body = TYPEWRITER_QUOTES.replace_all(&body, "« $1 »");
    let body = SPACE_BEFORE_PUNCT.replace_all(&body, "$1");
    body.trim().to_string()
}

/// Recueil reference to a judgment cited: `[ RJ1 ]`, `(RJ2)`, `[RJ1-2]`, `,RJ1,RJ2`, `RJ2`.
static RJ_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:\s*,\s*RJ\s*\d+)+|\s*[\[(]?\s*\bRJ\s*\d+(?:\s*[,-]\s*(?:RJ\s*)?\d+|\s+RJ\s*\d+)*\s*[\])]?",
    )
    .unwrap()
});
/// Points of a résumé, `1) a) `, which the rubric repeating one point drops.
static LEADING_ENUMERATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\s*(?:\d+|[a-z])\)\s*)+").unwrap());
static TITRAGE_FOOTNOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\d+\)\.?$").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());

/// Under it a block of an analysis reads as a titrage.
const TITRAGE_MAX: usize = 160;
const SHINGLE: usize = 4;
/// Share of the shorter line's word 4-grams the longer one carries, past which the
/// two state one point. The reworded repeats sit above it.
const REPEAT: f64 = 0.8;

/// A titrage of the old recueil opens on a `:` or `- :` line.
fn opens_old_titrage(block: &str) -> bool {
    matches!(block.lines().next().map(str::trim), Some(":" | "- :"))
}

fn reads_as_titrage(block: &str) -> bool {
    block.contains(" - ")
        || block.contains(" \u{2013} ")
        || TITRAGE_FOOTNOTE.is_match(block)
        || block.chars().count() < TITRAGE_MAX
}

fn shingles(line: &str) -> std::collections::HashSet<String> {
    let lower = line.to_lowercase();
    let unnumbered = LEADING_ENUMERATOR.replace(&lower, "");
    let words: Vec<&str> = WORD.find_iter(&unnumbered).map(|w| w.as_str()).collect();
    if words.len() < SHINGLE {
        return std::iter::once(words.join(" ")).collect();
    }
    words.windows(SHINGLE).map(|w| w.join(" ")).collect()
}

fn repeats(
    line: &std::collections::HashSet<String>,
    kept: &std::collections::HashSet<String>,
) -> bool {
    let shared = line.intersection(kept).count() as f64;
    shared / line.len().min(kept.len()) as f64 > REPEAT
}

/// The blocks of a Conseil d'Etat analysis, each told résumé (`true`) or titrage. The
/// body alternates titrage and résumé, one pair per rubric, blank lines apart.
fn analysis_blocks(body: &str) -> Vec<(&str, bool)> {
    let blocks: Vec<&str> = body
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .collect();
    if blocks.iter().any(|block| opens_old_titrage(block)) {
        return blocks
            .into_iter()
            .map(|block| (block, !opens_old_titrage(block)))
            .collect();
    }
    if blocks.len().is_multiple_of(2) {
        return blocks
            .into_iter()
            .enumerate()
            .map(|(i, block)| (block, i % 2 == 1))
            .collect();
    }
    let last = blocks.len() - 1;
    blocks
        .into_iter()
        .enumerate()
        .map(|(i, block)| (block, i == last || !reads_as_titrage(block)))
        .collect()
}

/// The titrages of a Conseil d'Etat analysis, one per rubric, its lines joined.
pub fn analysis_titrages(body: &str) -> Vec<String> {
    analysis_blocks(body)
        .into_iter()
        .filter(|&(_, resume)| !resume)
        .map(|(block, _)| {
            block
                .lines()
                .map(|line| RJ_MARK.replace_all(line, ""))
                .map(|line| line.trim().trim_matches([':', '-', ' ']).to_string())
                .filter(|line| WORD.is_match(line))
                .collect::<Vec<_>>()
                .join(" - ")
        })
        .filter(|titrage| !titrage.is_empty())
        .collect()
}

/// The résumés of a Conseil d'Etat analysis, each point once, one per line. A résumé
/// recurs, at times reworded, under every rubric it answers.
pub fn analysis_resumes(body: &str) -> String {
    let resumes: Vec<&str> = analysis_blocks(body)
        .into_iter()
        .filter(|&(_, resume)| resume)
        .map(|(block, _)| block)
        .collect();
    let mut kept: Vec<(String, std::collections::HashSet<String>)> = Vec::new();
    for line in resumes.iter().flat_map(|resume| resume.lines()) {
        let line = RJ_MARK.replace_all(line, "");
        let line = line.trim().trim_start_matches([':', '-', ' ']);
        if !WORD.is_match(line) {
            continue;
        }
        let words = shingles(line);
        match kept.iter_mut().find(|(_, seen)| repeats(&words, seen)) {
            Some(slot) if line.len() > slot.0.len() => *slot = (line.to_string(), words),
            Some(_) => {}
            None => kept.push((line.to_string(), words)),
        }
    }
    kept.into_iter()
        .map(|(line, _)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The model writes the curly apostrophe, the courts the straight one. Two
/// registers inside one paragraph is what the collage makes visible and the rewrite
/// never does, the court's prose being the mass of the text.
fn straight_apostrophes(text: &str) -> String {
    text.replace('\u{2019}', "'")
}

/// A court's sommaire sometimes stops on its last word, which glued it to the
/// outcome clause.
const STOPS: [char; 4] = ['.', '!', '?', '\u{2026}'];
const DANGLING: [char; 4] = [',', ';', ':', ' '];

fn closed(prose: &str) -> String {
    let body = prose.trim_end_matches(DANGLING);
    if body.ends_with(STOPS) {
        body.to_string()
    } else {
        format!("{body}.")
    }
}

/// An outcome clause holds more than one word: a single one is noise, never a clause.
fn is_outcome(clause: &str) -> bool {
    clause.trim().contains(char::is_whitespace)
}

/// Byte offsets, in a stored collage, of the commentary's first word and of the
/// end of its last one, before its closing stop.
fn locate_commentary(summary: &str, commentary: &str) -> Option<(usize, usize)> {
    let prose = clean_summary(&commentary_prose(commentary));
    let tail = prose.trim_end_matches(DANGLING);
    if tail.is_empty() {
        return None;
    }
    // The last words recur, in the commentary itself as in the outcome clause, so
    // they are looked for where the commentary ends, give or take what the cleaning
    // took out of it.
    let head = &tail[..tail.char_indices().nth(40).map_or(tail.len(), |(i, _)| i)];
    let anchor = &tail[tail.char_indices().rev().nth(39).map_or(0, |(i, _)| i)..];
    let mut from = summary.find(head)? + (tail.len() - anchor.len()).saturating_sub(40);
    while !summary.is_char_boundary(from) {
        from -= 1;
    }
    let end = from + summary.get(from..)?.find(anchor)? + anchor.len();
    if summary[end..].starts_with(char::is_alphanumeric) {
        return None;
    }
    // The opening often starts on the commentary's own first words.
    let expected = end.saturating_sub(tail.len());
    let start = summary
        .match_indices(head)
        .map(|(at, _)| at)
        .filter(|&at| at < end)
        .min_by_key(|&at| at.abs_diff(expected))?;
    Some((start, end))
}

/// Where the court's commentary sits in a stored collage: codepoint offsets, half
/// open, its closing stop included. `None` when its first or last words are not
/// found.
pub fn collage_quote(summary: &str, commentary: &str) -> Option<std::ops::Range<usize>> {
    let (start, end) = locate_commentary(summary, commentary)?;
    let stops = summary[end..].len() - summary[end..].trim_start_matches(STOPS).len();
    Some(summary[..start].chars().count()..summary[..end + stops].chars().count())
}

/// Repairs, in a collage already stored, what [`assemble_collage`] now does on
/// writing: the commentary closed by a stop, an outcome of one word dropped.
/// `None` when nothing changes, or when the commentary's last words are not found.
pub fn repair_collage(summary: &str, commentary: &str) -> Option<String> {
    let (_, end) = locate_commentary(summary, commentary)?;
    let rest = summary[end..].trim_start_matches(|c| DANGLING.contains(&c) || STOPS.contains(&c));
    let mut out = summary[..end].to_string();
    if !out.ends_with(STOPS) {
        out.push('.');
    }
    let outcome = rest.trim();
    if is_outcome(outcome) {
        out.push(' ');
        out.push_str(outcome);
    }
    (out != summary).then_some(out)
}

/// A stored collage with `commentary` pasted in place of `pasted`, `None` when it is not found.
pub fn repaste_collage(summary: &str, pasted: &str, commentary: &str) -> Option<String> {
    repaste_quote(summary, collage_quote(summary, pasted)?, commentary)
}

/// A stored collage with `commentary` pasted over `quote`, its codepoint span.
pub fn repaste_quote(
    summary: &str,
    quote: std::ops::Range<usize>,
    commentary: &str,
) -> Option<String> {
    let chars: Vec<char> = summary.chars().collect();
    let opening: String = chars[..quote.start].iter().collect();
    let outcome: String = chars[quote.end..].iter().collect();
    // An old paste could open on the recueil's `-: -.` markers, left before the quote.
    let opening = opening.trim_end_matches(|c: char| c.is_whitespace() || ".:;,-".contains(c));
    assemble_collage(
        &format!("{}\n{}", opening.trim(), outcome.trim()),
        commentary,
    )
    .map(|written| written.summary)
}

/// A summary as stored: its text, and where its body sits between the opening and the
/// outcome clause, in codepoints, its closing stop included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub summary: String,
    pub body: std::ops::Range<usize>,
}

/// The lines of an answer, without the ones a model writes for an empty clause and the
/// header lines it echoes.
fn said(model_output: &str) -> Vec<&str> {
    cut_model_tail(model_output)
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !matches!(
                    line.to_lowercase().as_str(),
                    "-" | "aucune" | "vide" | "néant"
                )
                && !context::LABELS.iter().any(|label| line.starts_with(label))
        })
        .collect()
}

/// Each part cleaned on its own, so the bounds hold on the text stored.
fn assemble(opening: &str, body: &str, outcome: Option<&str>) -> Option<Written> {
    let opening = clean_summary(opening);
    let body = clean_summary(body);
    if opening.is_empty() || body.is_empty() {
        return None;
    }
    let mut summary = closed(opening.trim_end_matches('.'));
    summary.push(' ');
    let start = summary.chars().count();
    summary.push_str(&closed(&body));
    let end = summary.chars().count();
    if let Some(outcome) = outcome.map(clean_summary).filter(|o| is_outcome(o)) {
        summary.push(' ');
        summary.push_str(&closed(&capitalize_first(outcome.trim_end_matches('.'))));
    }
    Some(Written {
        summary,
        body: start..end,
    })
}

/// An opening the meta description would cut, or that names a party by its anonymisation
/// code, is a failed answer, to be written again.
fn opening_holds(written: Written) -> Option<Written> {
    let opening: String = written.summary.chars().take(written.body.start).collect();
    (written.body.start <= FIRST_SENTENCE_MAX + 1 && !names_by_code(&opening)).then_some(written)
}

/// Opening, points of law, outcome clause: the three lines of a rewrite, the third left
/// empty when the points already say what the decision does. An opening line that runs
/// on into the points is cut after its first sentence. `None` past three lines.
pub fn assemble_rewrite(model_output: &str) -> Option<Written> {
    let said = said(model_output);
    let (line, body, outcome) = match said.as_slice() {
        [line] => (*line, "", None),
        [line, body] => (*line, *body, None),
        [line, body, outcome] => (*line, *body, Some(*outcome)),
        _ => return None,
    };
    let (opening, body) = match run_on_opening(line) {
        Some((opening, run_on)) => (opening, format!("{run_on} {body}")),
        None => (line, body.to_owned()),
    };
    assemble_holding(opening, &body, outcome)
}

/// The opening as written, else with its name cut to its first party, else with the party
/// named by its code at its head named by its role.
fn assemble_holding(opening: &str, body: &str, outcome: Option<&str>) -> Option<Written> {
    [
        Some(opening.to_owned()),
        first_party(opening),
        by_role(opening),
    ]
    .into_iter()
    .flatten()
    .find_map(|opening| assemble(&opening, body, outcome).and_then(opening_holds))
}

/// « Dans l'affaire Hepple, … », the name cut to its first party as the Luxembourg and
/// Strasbourg courts cite a case.
fn first_party(opening: &str) -> Option<String> {
    let end = Some(name_end(opening)).filter(|&end| end > 0)?;
    let start = opening.find("affaire ")? + "affaire ".len();
    let cut = PARTIES
        .iter()
        .filter_map(|party| opening[start..end].find(party))
        .min()?;
    Some(format!("{}{}", &opening[..start + cut], &opening[end..]))
}

static CODED_SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
    let party = r"(?:M\.|Mme|Mlle|MM\.|Mmes) (?:et (?:Mme|M\.) )?[A-Z](?:\.\.\.|\.)?(?: [A-Z](?:\.\.\.|\.)?)?";
    Regex::new(&format!(r"^{party}(?:, {party})*(?: et {party})? ")).unwrap()
});

/// « M. A conteste » is the party that seized the court: « Le requérant conteste ».
fn by_role(opening: &str) -> Option<String> {
    let subject = CODED_SUBJECT.find(opening)?;
    let rest = &opening[subject.end()..];
    let verb = rest.split(' ').next()?;
    if verb == "et" || !verb.chars().next()?.is_lowercase() {
        return None;
    }
    let civilities: Vec<&str> = subject
        .as_str()
        .split(' ')
        .filter(|word| matches!(*word, "M." | "Mme" | "Mlle" | "MM." | "Mmes"))
        .collect();
    let several = civilities.len() > 1 || civilities.iter().any(|c| matches!(*c, "MM." | "Mmes"));
    let women = civilities
        .iter()
        .all(|c| matches!(*c, "Mme" | "Mlle" | "Mmes"));
    let role = match (several, women) {
        (false, false) => "Le requérant",
        (false, true) => "La requérante",
        (true, false) => "Les requérants",
        (true, true) => "Les requérantes",
    };
    Some(format!("{role} {rest}"))
}

/// Below this, a stop closes an abbreviation (« S.A. »), not an opening: no opening
/// written on a line of its own is that short.
const OPENING_MIN: usize = 25;

const CIVILITIES: [&str; 6] = ["M", "MM", "Mme", "Mlle", "Me", "Dr"];

fn run_on_opening(line: &str) -> Option<(&str, &str)> {
    let name_end = name_end(line);
    line.match_indices(". ").find_map(|(at, _)| {
        let head = &line[..=at];
        let rest = line[at + 2..].trim_start();
        let sentence = rest.chars().next().is_some_and(char::is_uppercase);
        (at > name_end && !initial(&line[..at]) && sentence && head.chars().count() >= OPENING_MIN)
            .then_some((head, rest))
    })
}

/// Where « Dans l'affaire <name>, » closes the name, whose stops end no sentence. A comma
/// further than a name runs is in the points, after an opening that names nobody.
fn name_end(line: &str) -> usize {
    let Some(named) = ["Dans l'affaire ", "Dans l\u{2019}affaire "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
    else {
        return 0;
    };
    named
        .find(", ")
        .filter(|&end| named[..end].chars().count() <= NAME_MAX)
        .map_or(0, |end| line.len() - named.len() + end)
}

/// « c. » and « v. » between parties, « M. », a capital after a civility or another
/// initial; « catégorie A. » ends a sentence.
fn initial(before_stop: &str) -> bool {
    let mut words = before_stop.rsplit(' ');
    let mut letters = words.next().unwrap_or("").chars();
    let (Some(letter), None) = (letters.next(), letters.next()) else {
        return false;
    };
    letter.is_lowercase()
        || letter == 'M'
        || words.next().is_some_and(|previous| {
            let previous = previous.trim_end_matches('.');
            CIVILITIES.contains(&previous) || previous.chars().count() == 1
        })
}

/// Opening, the court's commentary, outcome clause.
///
/// `None` when the model wrote no opening: a commentary alone is not a summary,
/// it never says who contests what.
pub fn assemble_collage(model_output: &str, commentary: &str) -> Option<Written> {
    let said = said(model_output);
    let opening = straight_apostrophes(said.first()?);
    let outcome = said.get(1).map(|outcome| straight_apostrophes(outcome));
    assemble_holding(&opening, &commentary_prose(commentary), outcome.as_deref())
}

/// A stored summary cleaned again part by part, so that its body bounds still hold.
pub fn reclean_parts(summary: &str, body: std::ops::Range<usize>) -> Option<Written> {
    let chars: Vec<char> = summary.chars().collect();
    let part = |span: std::ops::Range<usize>| chars.get(span).map(String::from_iter);
    let outcome = part(body.end..chars.len())?;
    assemble(
        &part(0..body.start)?,
        &part(body)?,
        Some(outcome.as_str()).filter(|outcome| !outcome.trim().is_empty()),
    )
}

/// Where a stored summary's body sits, in codepoints: its stored bounds, else the court's
/// words of a collage, else what follows the opening of a rewrite stored without bounds,
/// its outcome included. `None` for an opening alone.
pub fn summary_body(
    summary: &str,
    stored: Option<std::ops::Range<usize>>,
    quote: Option<std::ops::Range<usize>>,
) -> Option<std::ops::Range<usize>> {
    stored.or(quote).or_else(|| {
        let (_, rest) = run_on_opening(summary)?;
        let len = summary.chars().count();
        Some(len - rest.chars().count()..len)
    })
}

/// `summary_engine` of a court's own summary pasted whole, with no model and no prompt.
pub const COURT_SUMMARY_ENGINE: i16 = 27;

/// `summary_prompt_version` of a court's summary, above every generation: no stale
/// query takes it.
pub const COURT_SUMMARY_GENERATION: i16 = i16::MAX;

/// Above a model summary's length, below a court's summary pasted whole.
const SUMMARY_HEAD_MAX: usize = 2_000;

/// What the embedder and the reranker read of a summary: its first [`SUMMARY_HEAD_MAX`]
/// characters, ending on the last line or full stop they hold.
pub fn summary_head(summary: &str) -> &str {
    let Some((cut, _)) = summary.char_indices().nth(SUMMARY_HEAD_MAX) else {
        return summary;
    };
    let head = &summary[..cut];
    let end = head.rfind(['\n', '.']).map_or(cut, |i| i + 1);
    head[..end].trim_end()
}

/// What a list of decisions carries of a summary: its [`summary_head`], marked when cut.
pub fn summary_teaser(summary: &str) -> String {
    match summary_head(summary) {
        head if head.len() == summary.len() => head.to_string(),
        head => format!("{head} […]"),
    }
}

// --- Nettoyage déterministe des codes d'anonymisation ----------------------
//
// Le LLM recopie parfois la pseudonymisation de la source. Deux familles :
// - placeholders sans valeur de nom — lieu (« [Localité 6] ») ou code numérique
//   (« la société [7] », « [22] ») : on supprime le code ET sa préposition
//   introductrice éventuelle, sinon « le comité économique du [7] » laisse « le
//   comité économique du, » orphelin.
// - initiales d'une personne : « M. [W] [Y] » — c'est le nom anonymisé, on garde
//   le contenu et on retire juste les crochets → « M. W Y ».
//
// Le LLM recopie aussi parfois l'en-tête « [Décision] <jur>, <date>, <num> »
// (interdit par le prompt) en première ligne : on le retire.

/// En-tête recopié en tête de résumé : ligne « [Décision]/Décision … <date ISO>
/// … » suivie d'un saut de ligne. La date ISO (signature de la citation injectée
/// en en-tête) évite de confondre avec une vraie phrase commençant par « Décision ».
static HEADER_ECHO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[?\s*Décision\b[^\n]*?\d{4}-\d{2}-\d{2}[^\n]*\n+").unwrap());
/// Placeholder pseudonymisé sans valeur de nom — lieu ou code numérique — retiré
/// avec sa préposition introductrice éventuelle.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[ \t]*(?:\b(?:de la|de l'|du|des|de|à la|aux|au|à|en|dans|sur|vers|chez)\s*)?\[(?:(?:Localité|Adresse|Commune|Ville)[^\]]*|\d[^\]]*)\]",
    )
    .unwrap()
});
/// Initiales de personne « [W] » → contenu gardé, crochets retirés.
static INITIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\d][^\]]{0,29})\]").unwrap());
static SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([,.;:!?])").unwrap());
static MULTISPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
/// Ponctuation/espaces orphelins en tête (placeholder retiré en début de phrase).
static LEADING_JUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,;:.–—-]+").unwrap());
/// Outside the repertoire French legal prose uses: ideographs, private use area,
/// fullwidth forms, a repeated female sign. Greek stays, the
/// CJUE quotes Greek trademarks; so do the comparison operators, a summary writes
/// `effectif salarié ≤ 5`.
/// Chat-template and code tokens. What follows one is no summary, and the sentence
/// it broke is cut with it.
static MODEL_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\s*(?:#\+#|\bto=\w|assistant\.?final|<\|\w+\|>|```).*").unwrap()
});

/// Code tokens glued to a word.
static GLUED_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(?:Winvalid\b|-?offsetof\b|%timeout\b)\??").unwrap());

fn cut_model_tail(text: &str) -> &str {
    let Some(found) = MODEL_TAIL.find(text) else {
        return text;
    };
    let kept = &text[..found.start()];
    let Some(stop) = kept.rfind(STOPS) else {
        return kept;
    };
    let kept = &kept[..stop + kept[stop..].chars().next().map_or(0, char::len_utf8)];
    match kept.strip_suffix("..") {
        Some(doubled) if !doubled.ends_with('.') => &kept[..kept.len() - 1],
        _ => kept,
    }
}

static FOREIGN_GLYPH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "[^\t\n\r\x20-\x7e\u{a0}-\u{24f}\u{300}-\u{52f}\u{600}-\u{6ff}\u{1e00}-\u{1eff}\
         \u{2000}-\u{209f}\u{20a0}-\u{20bf}\u{2100}-\u{218f}\u{2200}-\u{22ff}]",
    )
    .unwrap()
});

/// Nettoie la sortie modèle (echo d'en-tête, placeholders, initiales, ponctuation).
///
/// Trois choses : (1) un en-tête « [Décision] … » recopié en tête ; (2) les
/// placeholders (numéros, lieux) avec leur préposition introductrice ; en gardant
/// (3) les noms (initiales) — crochets retirés. Préserve les sauts de ligne
/// (séparateur de phrases) et est idempotent / no-op sur un résumé déjà propre.
pub fn clean_summary(text: &str) -> String {
    let text = GLUED_TOKEN.replace_all(cut_model_tail(text), "");
    let text = FOREIGN_GLYPH.replace_all(&text, "");
    let text = HEADER_ECHO.replace(&text, "");
    let text = PLACEHOLDER.replace_all(&text, "");
    let text = INITIAL.replace_all(&text, "$1");
    let text = SPACE_BEFORE_PUNCT.replace_all(&text, "$1");
    let text = MULTISPACE.replace_all(&text, " ");
    let leading_removed = LEADING_JUNK.is_match(&text);
    let text = LEADING_JUNK.replace(&text, "");
    let text = text.trim();

    // Un placeholder retiré en tête de phrase laisse espace/ponctuation orphelins
    // (`leading_removed`), puis une minuscule en première position : un résumé ouvre
    // toujours une phrase, on recapitalise.
    if leading_removed && first_char_is_lower(text) {
        capitalize_first(text)
    } else {
        text.to_string()
    }
}

/// Vrai ssi le premier caractère est une minuscule (au sens Unicode). Faux sur
/// chaîne vide ou premier char non-cased.
fn first_char_is_lower(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_lowercase())
}

/// Majuscule du premier caractère.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_pass_redoes_what_did_not_reason_named_a_code_or_opens_too_long() {
        let short = "La cour rejette le pourvoi d'un salarié licencié. Elle juge que la faute grave prive de préavis.";
        assert!(!wants_second_pass(short, 900));
        assert!(wants_second_pass(short, 0));
        assert!(wants_second_pass(
            "Le requérant [X] conteste un refus de titre.",
            900
        ));
        assert!(wants_second_pass("M. X conteste un refus de titre.", 900));
        assert!(wants_second_pass(
            "La société B... conteste une pénalité.",
            900
        ));
        assert!(!wants_second_pass(
            "M. Dupont conteste un refus de titre.",
            900
        ));
        let long_first = format!("Un salarié conteste {}. Le juge rejette.", "x".repeat(150));
        assert!(wants_second_pass(&long_first, 900));
        let long_second = format!("Un salarié conteste. Le juge {}.", "x".repeat(200));
        assert!(!wants_second_pass(&long_second, 900));
        assert_eq!(first_sentence_chars("Article 1. de la loi. Fin."), 21);
    }

    const ENGINES: [SummaryEngine; 27] = [
        SummaryEngine::Gemini31FlashLiteV9,
        SummaryEngine::Gemini35FlashLiteV9,
        SummaryEngine::LunaMonoV9,
        SummaryEngine::LunaCollageV10,
        SummaryEngine::Gemini31MonoV10,
        SummaryEngine::Gemini35MonoV10,
        SummaryEngine::LunaMonoV10,
        SummaryEngine::LunaCollageV11,
        SummaryEngine::Luna6MonoV10,
        SummaryEngine::Luna6CollageV10,
        SummaryEngine::Haiku55MonoBatchV10,
        SummaryEngine::Haiku55MonoCliV10,
        SummaryEngine::Gemini31PartsV11,
        SummaryEngine::Gemini35PartsV11,
        SummaryEngine::Luna6PartsV11,
        SummaryEngine::Luna6CollageHeadedV11,
        SummaryEngine::Haiku55BlockBatchV11,
        SummaryEngine::Haiku55BlockCliV11,
        SummaryEngine::Luna6BlockV11,
        SummaryEngine::Gemini31BlockV11,
        SummaryEngine::Gemini35BlockV11,
        SummaryEngine::Luna6BlockV12,
        SummaryEngine::Haiku55BlockBatchV12,
        SummaryEngine::Haiku55BlockCliV12,
        SummaryEngine::Gemini31BlockV12,
        SummaryEngine::Gemini35BlockV12,
        SummaryEngine::Luna6CollageHeadedV12,
    ];

    #[test]
    fn prompt_version_is_the_newest_generation() {
        let newest = ENGINES.map(SummaryEngine::prompt_version).into_iter().max();
        assert_eq!(newest, Some(SUMMARY_PROMPT_VERSION));
    }

    /// Two engines may share a prompt, a text measured on both models being one text.
    /// What may never happen is a generation reading another one's: the number would
    /// then stop saying what was sent.
    #[test]
    fn every_engine_round_trips_and_carries_its_generation_prompt() {
        for engine in ENGINES {
            assert_eq!(SummaryEngine::from_code(engine.code()), Some(engine));
            assert_eq!(
                engine.prompt() == SUMMARY_PROMPT,
                engine.prompt_version() == 8
            );
        }
        assert_eq!(SummaryEngine::from_code(0), None);
    }

    #[test]
    fn the_newest_generation_writes_in_the_language_of_its_text() {
        let newest = ENGINES
            .into_iter()
            .filter(|engine| engine.prompt_version() == SUMMARY_PROMPT_VERSION);
        for engine in newest {
            for (code, name) in SUMMARY_LANGUAGES {
                let prompt = engine.system_prompt(code).unwrap();
                assert!(prompt.contains(name) && !prompt.contains("{target_language}"));
            }
            assert_eq!(engine.system_prompt("ja"), None);
        }
        assert_eq!(
            SummaryEngine::Luna6CollageHeadedV11.system_prompt("de"),
            None
        );
    }

    #[test]
    fn clean_strips_the_spam_token_and_keeps_the_summary_scripts_and_operators() {
        assert_eq!(
            clean_summary("La requête est rejetée.娱乐网址"),
            "La requête est rejetée."
        );
        assert_eq!(
            clean_summary("Le pourvoi est rejeté. 天天中彩票官方 La cassation est acquise."),
            "Le pourvoi est rejeté. La cassation est acquise."
        );
        assert_eq!(
            clean_summary("Le code de justice administrative s'applique.♀♀♀"),
            "Le code de justice administrative s'applique."
        );
        assert_eq!(
            clean_summary("La marque υγεία est enregistrée pour un effectif salarié ≤ 5."),
            "La marque υγεία est enregistrée pour un effectif salarié ≤ 5."
        );
        for summary in ["Жалбата се отхвърля.", "يرفض الطلب."] {
            assert_eq!(clean_summary(summary), summary);
        }
    }

    #[test]
    fn build_input_passthrough_under_cap() {
        let body = "petite décision";
        assert_eq!(build_summary_input(body), body);
    }

    #[test]
    fn build_input_head_tail_over_cap() {
        let body: String = "x".repeat(SUMMARY_INPUT_CAP + 1);
        let out = build_summary_input(&body);
        assert_eq!(
            out.chars().count(),
            SUMMARY_INPUT_HEAD + ELISION.chars().count() + SUMMARY_INPUT_TAIL
        );
        assert!(out.contains("\n[...]\n"));
    }

    #[test]
    fn build_input_at_cap_is_passthrough() {
        let body: String = "y".repeat(SUMMARY_INPUT_CAP);
        assert_eq!(build_summary_input(&body), body);
    }

    #[test]
    fn clean_keeps_names_strips_brackets() {
        assert_eq!(
            clean_summary("M. [W] [Y] conteste le refus."),
            "M. W Y conteste le refus."
        );
    }

    #[test]
    fn clean_strips_placeholder_with_preposition() {
        assert_eq!(clean_summary("le comité du [7] décide"), "le comité décide");
        assert_eq!(
            clean_summary("la cour de [Localité 5] statue"),
            "la cour statue"
        );
    }

    #[test]
    fn clean_strips_locality_placeholder() {
        assert_eq!(
            clean_summary("La société [Localité 6] agit."),
            "La société agit."
        );
    }

    #[test]
    fn clean_strips_header_echo() {
        let input = "[Décision] Cour de cassation, 2024-03-12, 22-10.123\nUn salarié conteste son licenciement.";
        assert_eq!(
            clean_summary(input),
            "Un salarié conteste son licenciement."
        );
    }

    #[test]
    fn clean_recapitalizes_after_leading_junk() {
        assert_eq!(
            clean_summary("[7] saisit le tribunal."),
            "Saisit le tribunal."
        );
    }

    #[test]
    fn clean_is_noop_on_clean_summary() {
        let clean = "Un salarié licencié conteste la rupture de son contrat.";
        assert_eq!(clean_summary(clean), clean);
    }

    #[test]
    fn clean_cuts_what_follows_a_chat_template_token() {
        assert_eq!(
            clean_summary(
                "Le montant du cautionnement initialement fixé.. #+#+#+#+assistantfinal."
            ),
            "Le montant du cautionnement initialement fixé."
        );
        assert_eq!(
            clean_summary(
                "La valeur du service a pu être appréciée. Grâce to=final code (0 chars) Ndzi."
            ),
            "La valeur du service a pu être appréciée."
        );
        assert_eq!(
            clean_summary("Article R. 222-1 du code de justice administrative.Winvalid to=assistant (commentary) code"),
            "Article R. 222-1 du code de justice administrative."
        );
        assert_eq!(
            clean_summary("La cassation s'étend à toutes les dispositions de l'arrêtWinvalid."),
            "La cassation s'étend à toutes les dispositions de l'arrêt."
        );
        assert_eq!(
            clean_summary("Rectification du calcul des intérêts dus-offsetof."),
            "Rectification du calcul des intérêts dus."
        );
        assert_eq!(
            clean_summary("Le délai court depuis 5 ans. Winvalid La preuve incombe au conjoint."),
            "Le délai court depuis 5 ans. La preuve incombe au conjoint."
        );
        let clean = "Une assistante finale de la chaîne conteste son licenciement.";
        assert_eq!(clean_summary(clean), clean);
    }

    #[test]
    fn clean_removes_space_before_punct() {
        assert_eq!(clean_summary("le comité [7] , statue"), "le comité, statue");
    }

    #[test]
    fn collage_pastes_the_commentary_between_what_the_model_wrote() {
        let commentary = "Champ d'application - Inclusion - Conséquence\n\n1) Il résulte de \
            l'article L. 136-3 du CSS que les revenus tirés d'une activité non salariée sont \
            assujettis dès lors qu'elle est exercée à titre professionnel (1). 2) Indemnité pour \
            \"gestion d'affaire\" regardée comme un revenu professionnel.";
        let assembled = assemble_collage(
            "Des contribuables contestent l'assujettissement d'une indemnité\n\nDécharge des \
             cotisations supplémentaires",
            commentary,
        )
        .unwrap()
        .summary;
        assert!(assembled.starts_with(
            "Des contribuables contestent l'assujettissement d'une indemnité. Il résulte"
        ));
        assert!(assembled.ends_with("Décharge des cotisations supplémentaires."));
        assert!(assembled.contains("« gestion d'affaire »"));
        for apparatus in ["Champ d'application", "1)", "2)", "(1)", "\n"] {
            assert!(!assembled.contains(apparatus), "reste {apparatus:?}");
        }
    }

    #[test]
    fn collage_quote_spans_the_commentary_and_its_stop() {
        let commentary = "Le contrat d'hospitalisation met à la charge de l'établissement \
            une obligation de sécurité de résultat dont il ne se libère que par la cause \
            étrangère [Localité 3]";
        let court = "Le contrat d'hospitalisation met à la charge de l'établissement une \
            obligation de sécurité de résultat dont il ne se libère que par la cause étrangère.";
        let quoted = |summary: &str| {
            let quote = collage_quote(summary, commentary).unwrap();
            summary
                .chars()
                .skip(quote.start)
                .take(quote.end - quote.start)
                .collect::<String>()
        };
        for model in [
            "Une patiente infectée recherche la responsabilité de la clinique",
            "Le contrat d'hospitalisation met à la charge de la clinique une obligation",
            "Une patiente infectée recherche la responsabilité de la clinique\nD'où la \
             cassation de l'arrêt qui retient l'aléa thérapeutique",
        ] {
            let written = assemble_collage(model, commentary).unwrap();
            assert_eq!(quoted(&written.summary), court);
            assert_eq!(
                collage_quote(&written.summary, commentary),
                Some(written.body)
            );
        }
        assert_eq!(
            collage_quote("Une patiente conteste. Autre chose.", commentary),
            None
        );
    }

    #[test]
    fn repaste_swaps_the_commentary_and_keeps_the_model_words() {
        let shouted =
            "LE JUGE DU BORNAGE APPRECIE SOUVERAINEMENT L'UTILITE D'UNE MESURE D'INSTRUCTION";
        let recased =
            "Le juge du bornage apprécie souverainement l'utilité d'une mesure d'instruction";
        let stored = assemble_collage("Un voisin conteste le bornage\nd'où le rejet", shouted)
            .unwrap()
            .summary;
        let repasted = repaste_collage(&stored, shouted, recased).unwrap();
        assert_eq!(
            repasted,
            "Un voisin conteste le bornage. Le juge du bornage apprécie souverainement \
             l'utilité d'une mesure d'instruction. D'où le rejet."
        );
        assert!(collage_quote(&repasted, recased).is_some());
        assert_eq!(repaste_collage(&repasted, shouted, recased), None);
    }

    #[test]
    fn collage_keeps_a_commentary_that_is_prose_from_its_first_word() {
        let commentary = "L'infra petita ne se soulève pas d'office.";
        let assembled = assemble_collage("Un requérant conteste un jugement", commentary)
            .unwrap()
            .summary;
        assert_eq!(
            assembled,
            "Un requérant conteste un jugement. L'infra petita ne se soulève pas d'office."
        );
    }

    #[test]
    fn a_body_is_read_from_its_bounds_else_after_the_opening() {
        let named = "Dans l'affaire Commission c. France, la Commission conteste un manquement. La Cour peut cumuler une astreinte et une somme forfaitaire.";
        let body = summary_body(named, None, None).unwrap();
        assert!(named
            .chars()
            .skip(body.start)
            .collect::<String>()
            .starts_with("La Cour peut"));
        assert_eq!(body.end, named.chars().count());
        assert_eq!(summary_body(named, Some(3..9), Some(1..2)), Some(3..9));
        assert_eq!(summary_body(named, None, Some(1..2)), Some(1..2));
        assert_eq!(
            summary_body(
                "Une entreprise conteste le traitement fiscal de sa réserve.",
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn a_long_summary_is_read_up_to_its_last_whole_line_or_sentence() {
        let short = "La Cour rejette la requête.";
        assert_eq!(summary_head(short), short);
        let line = format!("{}.\n", "a".repeat(599));
        let long = format!("{}{}", line.repeat(3), "b".repeat(500));
        assert_eq!(summary_head(&long), line.repeat(3).trim_end());
        let unbroken = "c".repeat(2_500);
        assert_eq!(summary_head(&unbroken).chars().count(), 2_000);
        assert_eq!(summary_teaser(short), short);
        assert_eq!(
            summary_teaser(&long),
            format!("{} […]", line.repeat(3).trim_end())
        );
    }

    #[test]
    fn collage_without_an_opening_writes_nothing() {
        assert_eq!(assemble_collage("", "Un sommaire officiel."), None);
        assert_eq!(assemble_collage("\n  \n", "Un sommaire officiel."), None);
    }

    #[test]
    fn collage_aligns_the_model_apostrophe_on_the_court_one() {
        let assembled =
            assemble_collage("Une soci\u{e9}t\u{e9} conteste l\u{2019}assiette d\u{2019}une taxe\nd\u{2019}o\u{f9} la cassation", "Le sommaire.")
                .unwrap()
                .summary;
        assert_eq!(
            assembled,
            "Une société conteste l'assiette d'une taxe. Le sommaire. D'où la cassation."
        );
    }

    #[test]
    fn collage_closes_a_commentary_that_stops_on_its_last_word() {
        let assembled = assemble_collage(
            "Une société conteste sa condamnation\nd'où la cassation",
            "Encourt la cassation la décision qui ne caractérise pas la fraude retenue",
        )
        .unwrap()
        .summary;
        assert_eq!(
            assembled,
            "Une société conteste sa condamnation. Encourt la cassation la décision qui ne caractérise pas la fraude retenue. D'où la cassation."
        );
    }

    #[test]
    fn a_stored_collage_gets_the_stop_its_commentary_lacked() {
        let commentary =
            "Encourt la cassation la décision qui ne caractérise pas la fraude retenue ;";
        let stored = "Une société conteste sa condamnation. Encourt la cassation la décision qui ne caractérise pas la fraude retenue; D'où la cassation.";
        let fixed = "Une société conteste sa condamnation. Encourt la cassation la décision qui ne caractérise pas la fraude retenue. D'où la cassation.";
        assert_eq!(repair_collage(stored, commentary).as_deref(), Some(fixed));
        assert_eq!(repair_collage(fixed, commentary), None);
        let at_the_end = "Une société conteste sa condamnation. Encourt la cassation la décision qui ne caractérise pas la fraude retenue";
        assert_eq!(repair_collage(at_the_end, commentary).as_deref(), Some("Une société conteste sa condamnation. Encourt la cassation la décision qui ne caractérise pas la fraude retenue."));
        assert_eq!(
            repair_collage(stored, "Un autre sommaire sans rapport"),
            None
        );
        let inner = "Un juge doit appliquer la responsabilité du fait des produits défectueux. Une cour d'appel doit examiner la responsabilité du fait des produits défectueux";
        assert_eq!(
            repair_collage(
                &format!("Une société conteste. {inner} La cour d'appel devait le faire."),
                inner
            )
            .as_deref(),
            Some(
                format!("Une société conteste. {inner}. La cour d'appel devait le faire.").as_str()
            )
        );
        let repeated = "Un homme conteste sa détention. Elle n'intervient que par l'effet d'un ordre ou d'une condition Par l'effet d'un ordre ou d'une condition. Le pourvoi est rejeté.";
        assert_eq!(
            repair_collage(repeated, "Elle n'intervient que par l'effet d'un ordre ou d'une condition").as_deref(),
            Some("Un homme conteste sa détention. Elle n'intervient que par l'effet d'un ordre ou d'une condition. Par l'effet d'un ordre ou d'une condition. Le pourvoi est rejeté.")
        );
    }

    #[test]
    fn collage_drops_an_outcome_of_one_word() {
        let assembled = assemble_collage("Une ouverture\nCuntegn", "Le sommaire.")
            .unwrap()
            .summary;
        assert_eq!(assembled, "Une ouverture. Le sommaire.");
        let commentary =
            "Encourt la cassation la décision qui ne caractérise pas la fraude retenue";
        assert_eq!(
            repair_collage(
                &format!("Une ouverture. {commentary}. Cuntegn."),
                commentary
            )
            .as_deref(),
            Some(format!("Une ouverture. {commentary}.").as_str())
        );
        assert_eq!(
            repair_collage(
                &format!("Une ouverture. {commentary}. D'où la cassation."),
                commentary
            ),
            None
        );
    }

    #[test]
    fn collage_drops_the_word_a_model_writes_for_an_empty_clause() {
        let assembled = assemble_collage("Une ouverture\naucune", "Le sommaire.")
            .unwrap()
            .summary;
        assert_eq!(assembled, "Une ouverture. Le sommaire.");
    }

    #[test]
    fn a_rewrite_stores_its_three_lines_and_where_the_points_of_law_sit() {
        let body = |written: &Written| -> String {
            written
                .summary
                .chars()
                .skip(written.body.start)
                .take(written.body.len())
                .collect()
        };
        let written = assemble_rewrite(
            "Un salarié conteste son licenciement pour faute grave.\n\
             La faute grave rend impossible le maintien du salarié dans l'entreprise. \
             Elle prive de préavis.\n\
             d'où le rejet de la demande d'indemnité de préavis",
        )
        .unwrap();
        assert_eq!(
            written.summary,
            "Un salarié conteste son licenciement pour faute grave. La faute grave rend \
             impossible le maintien du salarié dans l'entreprise. Elle prive de préavis. \
             D'où le rejet de la demande d'indemnité de préavis."
        );
        assert_eq!(
            body(&written),
            "La faute grave rend impossible le maintien du salarié dans l'entreprise. \
             Elle prive de préavis."
        );
        let carried = assemble_rewrite(
            "[Décision] Cour de cassation, 3 mai 2024\n\
             Le préfet [Localité 2] conteste une annulation\n\
             Le préfet ne pouvait légalement refuser le titre\n\n",
        )
        .unwrap();
        assert_eq!(
            carried.summary,
            "Le préfet conteste une annulation. Le préfet ne pouvait légalement refuser le titre."
        );
        assert_eq!(
            body(&carried),
            "Le préfet ne pouvait légalement refuser le titre."
        );
        assert_eq!(
            reclean_parts(&written.summary, written.body.clone()),
            Some(written.clone())
        );
        assert_eq!(assemble_rewrite("Un seul bloc. Sans lignes."), None);
        let run_on = assemble_rewrite(
            "Un salarié conteste son licenciement pour faute grave. La faute grave prive de préavis.\n\
             D'où le rejet.",
        )
        .unwrap();
        assert_eq!(
            body(&run_on),
            "La faute grave prive de préavis. D'où le rejet."
        );
        assert_eq!(
            assemble_rewrite(
                "Dans l'affaire Commission c. Italie, la Commission reproche un manquement."
            ),
            None
        );
        let category = assemble_rewrite(
            "Un conducteur conteste le refus d'échanger son permis de catégorie A. \
             L'échange suppose un accord de réciprocité.",
        )
        .unwrap();
        assert_eq!(
            body(&category),
            "L'échange suppose un accord de réciprocité."
        );
        let initials = assemble_rewrite(
            "Dans l'affaire Banque Générale contre Jean A. Dupont, la banque conteste un arrêt. \
             La caution s'éteint avec la dette.",
        )
        .unwrap();
        assert_eq!(body(&initials), "La caution s'éteint avec la dette.");
        let unnamed = assemble_rewrite(
            "Dans l'affaire la venderesse conteste l'appel incident des acquéreurs. \
             L'article 910-4 sanctionne les prétentions nouvelles présentées après les premières \
             conclusions, sauf les moyens nouveaux.",
        )
        .unwrap();
        assert!(body(&unnamed).starts_with("L'article 910-4"));
        let long_name = assemble_rewrite(
            "Dans l'affaire Gesamthochschule Essen contre Hauptzollamt Düsseldorf, une université \
             conteste le refus d'une franchise douanière sur un matériau destiné à la recherche \
             scientifique. La notion d'instrument scientifique s'apprécie objectivement.",
        )
        .unwrap();
        assert!(long_name
            .summary
            .starts_with("Dans l'affaire Gesamthochschule Essen, une université conteste"));
        for (coded, role) in [
            (
                "M. A... conteste un refus de séjour.",
                "Le requérant conteste",
            ),
            (
                "Mme B conteste un refus de séjour.",
                "La requérante conteste",
            ),
            (
                "M. et Mme E contestent un refus de séjour.",
                "Les requérants contestent",
            ),
            (
                "Mme C et Mme D contestent un refus de séjour.",
                "Les requérantes contestent",
            ),
        ] {
            let written = assemble_rewrite(&format!("{coded}\nUne règle.")).unwrap();
            assert!(written.summary.starts_with(role), "{coded}");
        }
        assert_eq!(
            assemble_rewrite("Le préfet conteste l'annulation du refus opposé à M. A.\nUne règle."),
            None
        );
        let short = assemble_rewrite(
            "Une épouse demande le divorce. Le juge français est compétent pour statuer.",
        )
        .unwrap();
        assert_eq!(body(&short), "Le juge français est compétent pour statuer.");
        let long = format!(
            "Dans l'affaire {}, une société conteste\nUne règle.",
            "X ".repeat(70)
        );
        assert_eq!(assemble_rewrite(&long), None);
        assert!(assemble_collage("M. H A conteste un arrêt", "Le sommaire.")
            .unwrap()
            .summary
            .starts_with("Le requérant conteste un arrêt."));
        assert_eq!(
            assemble_collage(
                "Le préfet conteste un arrêt rendu pour M. H A",
                "Le sommaire."
            ),
            None
        );
        assert_eq!(assemble_rewrite("Une.\nDeux.\nTrois.\nQuatre."), None);
    }
}
