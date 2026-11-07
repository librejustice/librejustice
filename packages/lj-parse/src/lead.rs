//! Sections of the text a Judilibre decision carries before its dispositif when
//! the source zoned none of it, found by the markers of the court's drafting.

use std::sync::LazyLock;

use lj_core::decision::DecisionSection;
use regex::Regex;

use crate::{char_len, char_slice, orphan_motivations_bound, SECTION_NO_OFFSET, ZONE_KIND_MAP};

/// Below this, the text before the dispositif holds no more than a heading.
const LEAD_MIN_CHARS: usize = 200;

/// A paragraph this short can be a heading.
const HEADING_MAX_CHARS: usize = 80;

/// A paragraph this long, free of the header's words, is the narrative.
const NARRATIVE_MIN_CHARS: usize = 200;

/// How much of a paragraph the markers read.
const PREFIX_CHARS: usize = 300;

static CC_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:mais|et)\s+)?sur\s+(?:le|les|la|l')\s*(?:\S+\s+){0,3}?(?:moyens?|branches?)\b",
    )
    .unwrap()
});

static CC_BODY_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:mais|et)\s+)?sur\s+(?:le|les|la|l')\s*(?:\S+\s+){0,3}?(?:moyens?|branches?|recevabilite|fin de non)\b|^attendu|^vu\s+(?:l'article|les articles|le principe|la regle|l'arret)|^mais attendu|^donne acte",
    )
    .unwrap()
});

static CC_EXPOSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^attendu,?\s+(?:selon|qu'il resulte|qu'il ressort)\b").unwrap());

static CC_GRIEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:fai(?:t|s|sons|tes|nt)\s+grief|reproch\w*\s+(?:a|au|aux)\s+(?:l'arret|le jugement|la decision|l'ordonnance|la cour))",
    )
    .unwrap()
});

/// The branches of a moyen and the reasons it quotes.
static CC_BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\d+\s*°?\s*/?\s*-?\s*(?:alors\s+)?(?:que|qu')|alors\s+(?:\S+\s+){0,3}?(?:que|qu')|aux\s+motifs\s+(?:\S+\s+){0,2}?(?:que|qu')|en\s+ce\s+que)",
    )
    .unwrap()
});

static CC_MOTIV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:attendu|mais\s+attendu|et\s+attendu|d'ou\s+il\s+suit|vu\s|qu'en\s+statuant|qu'ainsi|que\s+le\s+moyen)")
        .unwrap()
});

/// The headings of the structured drafting, since 2019.
static CC_STRUCT: LazyLock<[(Regex, &str); 3]> = LazyLock::new(|| {
    [
        (Regex::new(r"^(?:\d+\s*[.-]\s*)?faits et procedure\b").unwrap(), "expose"),
        (
            Regex::new(r"^(?:\d+\s*[.-]\s*)?(?:examen d[ue]s? moyens?|enonce d[ue]s? moyens?|sur (?:le|les) moyens? relev)")
                .unwrap(),
            "moyens",
        ),
        (Regex::new(r"^(?:\d+\s*[.-]\s*)?reponse de la cour\b").unwrap(), "motivations"),
    ]
});

const NUMBERING: &str = r"^(?:[ivx]+\s*[-.)]\s*|[a-z0-9]\s*[-.)]\s*)?";

static CA_EXPOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"{NUMBERING}(?:expose\b|faits\b|rappel des faits|procedure\b|les faits|pretentions|exposition)"
    ))
    .unwrap()
});

static CA_MOTIV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"{NUMBERING}(?:motifs?\b|motivation|sur ce\b|sur quoi\b|discussion\b|sur le fond\b|la cour,?\s*$)"
    ))
    .unwrap()
});

static CA_HEADER_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"appelant|intime|avocat|avoue|represente|demeurant|domicili|siege|greffier|president|conseiller|composition|debats|prononce|mise a disposition|arret contradictoire|arret par defaut|repute contradictoire|aide juridictionnelle|ne le |nee le |\br\.?\s?g\b|minute|copie|expedition|delivree|au nom du peuple",
    )
    .unwrap()
});

static CA_BODY_MOTIV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:attendu|considerant|sur ce\b|sur quoi\b|motifs?\b|la cour,?\s)").unwrap()
});

static CA_EXPOSE_OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:par\s+(?:jugement|acte|ordonnance|declaration|requete|arret|exploit|lettre|courrier|decision|assignation)|suivant\s+(?:acte|jugement|exploit)|vu\s+(?:le|l')\s*(?:jugement|ordonnance|assignation|appel))",
    )
    .unwrap()
});

/// A non-empty line: its first character's codepoint offset, its length in
/// codepoints, and the fold of its opening.
struct Paragraph {
    start: usize,
    len: usize,
    folded: String,
}

fn paragraphs(text: &str) -> Vec<Paragraph> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let indent = line.len() - line.trim_start().len();
            let prefix: String = trimmed.chars().take(PREFIX_CHARS).collect();
            out.push(Paragraph {
                start: at + line[..indent].chars().count(),
                len: trimmed.chars().count(),
                folded: lj_fold::fold(&prefix),
            });
        }
        at += line.chars().count() + 1;
    }
    out
}

/// Fills the text before a dispositif standing first, which is all a decision
/// gets when its source zoned nothing and the dispositif came from its markers,
/// or the whole text when not even a dispositif was found.
pub(crate) fn augment_lead(
    sections: Vec<DecisionSection>,
    text: &str,
    cassation: bool,
) -> Vec<DecisionSection> {
    let (end, dispositif) = match sections
        .iter()
        .filter(|s| s.start_char != SECTION_NO_OFFSET)
        .min_by_key(|s| s.start_char)
    {
        Some(first) if first.kind == "dispositif" => (first.start_char, true),
        Some(_) => return sections,
        None => (char_len(text), false),
    };
    if end < LEAD_MIN_CHARS {
        return sections;
    }
    let paragraphs = paragraphs(&char_slice(text, 0, end));
    let mut starts = if cassation {
        segment_cassation(&paragraphs)
    } else {
        segment_headed(&paragraphs)
    };
    if let Some(anchor) = orphan_motivations_bound(starts.iter().copied()) {
        for (start, kind) in starts.iter_mut() {
            if *kind == "motivations" && *start < anchor {
                *kind = "preamble";
            }
        }
    }
    // Ordered as `build_sections` rebuilds them from storage, the synthetic visa
    // after the introduction.
    let mut out: Vec<DecisionSection> = starts
        .iter()
        .enumerate()
        .filter_map(|(i, &(start, kind))| {
            let stop = starts.get(i + 1).map_or(end, |&(next, _)| next);
            let body = char_slice(text, start, stop).trim().to_string();
            let label = ZONE_KIND_MAP.iter().find(|(_, k, _)| *k == kind)?.2;
            (!body.is_empty()).then(|| DecisionSection {
                label: label.to_string(),
                kind: kind.to_string(),
                start_char: start,
                end_char: start + char_len(&body),
                text: body,
            })
        })
        .collect();
    if !dispositif && out.iter().all(|s| s.kind == out[0].kind) {
        return sections;
    }
    let visa_at = out
        .iter()
        .position(|s| s.kind == "preamble")
        .map_or(0, |i| i + 1);
    for section in sections {
        if section.start_char == SECTION_NO_OFFSET {
            out.insert(visa_at, section);
        } else {
            out.push(section);
        }
    }
    out
}

/// The Cour de cassation's drafting: the header up to the first moyen or
/// attendu, then each paragraph by its opening, else in the section it continues.
fn segment_cassation(paragraphs: &[Paragraph]) -> Vec<(usize, &'static str)> {
    let mut out: Vec<(usize, &'static str)> = Vec::new();
    let mut state: Option<&'static str> = None;
    let mut started = false;
    for p in paragraphs {
        let f = p.folded.as_str();
        let heading = (p.len <= HEADING_MAX_CHARS)
            .then(|| {
                CC_STRUCT
                    .iter()
                    .find(|(re, _)| re.is_match(f))
                    .map(|(_, k)| *k)
            })
            .flatten();
        if !started {
            if heading.is_some() || CC_BODY_START.is_match(f) {
                started = true;
            } else {
                if state != Some("preamble") {
                    out.push((p.start, "preamble"));
                    state = Some("preamble");
                }
                continue;
            }
        }
        let kind = if let Some(kind) = heading {
            kind
        } else if CC_HEAD.is_match(f) {
            "moyens"
        } else if CC_EXPOSE.is_match(f) {
            "expose"
        } else if (f.starts_with("attendu") && CC_GRIEF.is_match(f)) || CC_BRANCH.is_match(f) {
            "moyens"
        } else if CC_MOTIV.is_match(f) {
            "motivations"
        } else {
            state.unwrap_or("motivations")
        };
        if state != Some(kind) {
            out.push((p.start, kind));
            state = Some(kind);
        }
    }
    out
}

/// A drafting that heads its parts, as the cours d'appel do. Without a heading,
/// the exposé opens on the narrative and the motifs on the court's attendu.
fn segment_headed(paragraphs: &[Paragraph]) -> Vec<(usize, &'static str)> {
    let Some(first) = paragraphs.first() else {
        return Vec::new();
    };
    let mut out = vec![(first.start, "preamble")];
    for p in paragraphs.iter().filter(|p| p.len <= HEADING_MAX_CHARS) {
        let state = out.last().expect("starts with the preamble").1;
        if state == "preamble" && CA_EXPOSE.is_match(&p.folded) {
            out.push((p.start, "expose"));
        } else if state != "motivations" && CA_MOTIV.is_match(&p.folded) {
            out.push((p.start, "motivations"));
        }
    }
    match out.as_slice() {
        [_] => segment_unheaded(paragraphs),
        &[_, (motifs, "motivations")] => {
            if let Some(p) = paragraphs
                .iter()
                .skip(1)
                .take_while(|p| p.start < motifs)
                .find(|p| CA_EXPOSE_OPEN.is_match(&p.folded) || is_narrative(p))
            {
                out.insert(1, (p.start, "expose"));
            }
            out
        }
        _ => out,
    }
}

fn segment_unheaded(paragraphs: &[Paragraph]) -> Vec<(usize, &'static str)> {
    let mut out = vec![(paragraphs[0].start, "preamble")];
    for p in paragraphs {
        let state = out.last().expect("starts with the preamble").1;
        if state == "preamble" && is_narrative(p) {
            let kind = if CA_BODY_MOTIV.is_match(&p.folded) {
                "motivations"
            } else {
                "expose"
            };
            out.push((p.start, kind));
        } else if state == "expose" && CA_BODY_MOTIV.is_match(&p.folded) {
            out.push((p.start, "motivations"));
        }
    }
    out
}

fn is_narrative(p: &Paragraph) -> bool {
    p.len >= NARRATIVE_MIN_CHARS && !CA_HEADER_WORDS.is_match(head(&p.folded, 200))
}

fn head(s: &str, chars: usize) -> &str {
    s.char_indices().nth(chars).map_or(s, |(i, _)| &s[..i])
}
