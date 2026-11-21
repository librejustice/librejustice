//! Adala's Arabic files: the act a file holds, its articles, the day of its consolidation.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::ejo::{articles_at, Head};
use crate::{AdalaIdentity, ConsolidatedArticle};

const N: &str = r"(\d+(?:\s?[./\-]\s?\d+)*)";

static DECISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:قرار|مقرر)\s+(?:ال)?(?:محكمة|مجلس|غرفة)").unwrap());
static CONSTITUTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*دستور\s+المملكة\s+المغربية\s+(?:لسنة\s+)?(\d{4})").unwrap());
/// The law a dahir promulgates, `بتنفيذ القانون رقم 65.99`, `بتنفيذ قانون المالية رقم 50.25`.
static PROMULGATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"ب?ت?نفي[ذد]\s+(?:نص\s+)?(?:ال)?قانون\s+(?:(?:ال)?(تنظيمي)\s+)?(?:\S+\s+){{0,3}}?رقم\s+{N}"
    ))
    .unwrap()
});
/// An act a line opens on, with its number.
static NUMBERED: LazyLock<[(Regex, &'static str, &'static str); 6]> = LazyLock::new(|| {
    let act = |words: &str| Regex::new(&format!(r"^\s*{words}\s+رقم\s+{N}")).unwrap();
    [
        (act(r"(?:ال)?قانون\s+(?:ال)?تنظيمي"), "LOI", "LOI_ORGANIQUE"),
        (act(r"(?:ال)?قانون"), "LOI", "LOI"),
        (
            act(r"ظهير(?:\s+شريف)?(?:\s+بمثابة\s+قانون)?"),
            "DAHIR",
            "DAHIR",
        ),
        (
            act(r"مرسوم\s+(?:بقانون|بمثابة\s+قانون)"),
            "DECRET-LOI",
            "DECRET_LOI",
        ),
        (act(r"مرسوم(?:\s+ملكي)?"), "DECRET", "DECRET"),
        (
            Regex::new(&format!(
                r"^\s*قرار\s+(?:مشترك\s+)?(ل[^\n]{{0,240}}?)\s+رقم\s+{N}"
            ))
            .unwrap(),
            "ARRETE",
            "ARRETE",
        ),
    ]
});
/// Words that make an arrêté's `رقم` another act's number.
static CITED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:قانون|ظهير|مرسوم|المادة|الفصل)").unwrap());
/// A decree's or an arrêté's name, whose first lines may name the law it applies.
static ACT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:مرسوم|قرار)").unwrap());
static OBLIGATIONS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*ظهير\s+(?:شريف\s+)?بمثابة\s+قانون\s+الالتزامات\s+والعقود").unwrap()
});
static MARITIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*ظهير\s+(?:شريف\s+)?بمثابة\s+(?:مدونة|قانون)\s+التجارة\s+البحرية").unwrap()
});
/// Where a consolidated text starts listing the acts that amended it.
static AMENDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"كما\s+(?:تم|وقع)\s+(?:تعديل|تغيير|تتميم)").unwrap());
static CONSOLIDATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"صيغة\s+محينة\s+(?:بتاريخ|في|إلى\s+غاية)\s+(فاتح|\d{1,2})\s+(\p{L}+)\s+(\d{4})")
        .unwrap()
});
static DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*(فاتح|\d{1,2})\s+(\p{L}+)\s+(\d{4})\s*\)").unwrap());
static PROMULGATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[يت]نف[ذد]\s+و[يت]نشر|(?:يصادق|يوافق)\s+على\s+(?:ال)?نص").unwrap()
});
static SIGNED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*و?حرر\s+ب").unwrap());
static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\s*\d{1,4}\s*-?$").unwrap());
static FOOTNOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}\s*-\s*(?:ال)?جريدة\s+(?:ال)?رسمية").unwrap());
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(المادة|الفصل)\s+(?:(\d+(?:\s?-\s?\d+)*)|(ال\p{L}+(?:\s+(?:عشرة|عشر|وال\p{L}+))?))(?:\s+(مكرر[ةا]?)(?:\s+(مرتين|ثلاث\s+مرات))?)?\s*(?:[.:\-\x{2013}\x{2014}]\s*)*(.*)$").unwrap()
});
/// What follows an article's number in running text rather than in its heading.
static RUNNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:من|في|أعلاه|اعلاه|أدناه|ادناه|بعده|بعدها|المشار|السالف|المذكور|المذكورة|و|أو|إلى|الى|على|المتعلق|المتعلقة)(?:\s|$)").unwrap()
});
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:ال)?(جزء|كتاب|قسم|باب|فصل|فرع)\s+(?:ال\p{L}+|\d+|تمهيدي)").unwrap()
});
/// The acts that made the codes Adala's French files key by their title.
const CODES: [(&str, &str); 10] = [
    ("70-03", "CODE-FAMILLE"),
    ("65-99", "CODE-TRAVAIL"),
    ("1-59-413", "CODE-PENAL"),
    ("15-95", "CODE-COMMERCE"),
    ("39-08", "CODE-DROITS-REELS"),
    ("15-97", "CODE-RECOUVREMENT-CREANCES-PUBLIQUES"),
    ("17-04", "CODE-MEDICAMENT-PHARMACIE"),
    ("1-58-250", "CODE-NATIONALITE-MAROCAINE"),
    ("1-74-447", "CODE-PROCEDURE-CIVILE"),
    ("22-01", "CODE-PROCEDURE-PENALE"),
];

/// An Arabic text as the parsers read it: NFKC, the tatweel and the direction marks gone,
/// Persian forms and Eastern digits as Arabic and Western ones.
pub fn adala_arabic_text(raw: &str) -> String {
    raw.nfkc()
        .filter_map(|c| match c {
            '\u{0640}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            '\u{06CC}' => Some('\u{064A}'),
            '\u{06A9}' => Some('\u{0643}'),
            '\u{0660}'..='\u{0669}' => char::from_digit(c as u32 - 0x0660, 10),
            '\u{06F0}'..='\u{06F9}' => char::from_digit(c as u32 - 0x06F0, 10),
            c => Some(c),
        })
        .collect()
}

fn coded(code: &str, nature: &'static str, number: &str) -> AdalaIdentity {
    let number = crate::journal_number_key(number);
    match CODES.iter().find(|(n, _)| *n == number) {
        Some((_, key)) if matches!(nature, "LOI" | "DAHIR") => AdalaIdentity {
            code: key.to_string(),
            nature: "CODE",
            number: None,
        },
        _ => AdalaIdentity {
            code: code.to_string(),
            nature,
            number: Some(number),
        },
    }
}

fn promulgated(s: &str) -> Option<AdalaIdentity> {
    let c = PROMULGATED.captures(s)?;
    let nature = match c.get(1) {
        Some(_) => "LOI_ORGANIQUE",
        None => "LOI",
    };
    Some(coded("LOI", nature, &c[2]))
}

fn numbered(line: &str, laws: bool) -> Option<AdalaIdentity> {
    NUMBERED.iter().find_map(|(rx, code, nature)| {
        let c = rx.captures(line)?;
        if *code == "ARRETE" && CITED.is_match(&c[1]) {
            return None;
        }
        if !laws && matches!(*code, "LOI" | "DAHIR") {
            return None;
        }
        Some(coded(code, nature, &c[c.len() - 1]))
    })
}

/// The act an Arabic file holds, read as a French file's is, its name's or its path's act
/// first: a code by the act that made it, a constitution by its year, two old dahirs by their
/// names. A court's decision is none.
pub fn adala_arabic_identity(name: &str, path: &str, text: &str) -> Option<AdalaIdentity> {
    let name = adala_arabic_text(name);
    let stem = adala_arabic_text(path.rsplit('/').next().unwrap_or(path));
    if DECISION.is_match(&name) {
        return None;
    }
    for s in [&name, &stem] {
        if let Some(identity) = promulgated(s).or_else(|| numbered(s, true)) {
            return Some(identity);
        }
    }
    if let Some(c) = CONSTITUTION.captures(&name) {
        return Some(AdalaIdentity {
            code: format!("CONSTITUTION-{}", &c[1]),
            nature: "CONSTITUTION",
            number: None,
        });
    }
    let titled = |code: &str| {
        Some(AdalaIdentity {
            code: code.to_string(),
            nature: "CODE",
            number: None,
        })
    };
    if OBLIGATIONS.is_match(&name) {
        return titled("CODE-OBLIGATIONS-CONTRATS");
    }
    if MARITIME.is_match(&name) {
        return titled("CODE-COMMERCE-MARITIME");
    }
    let head: String = text.chars().take(1500).collect();
    let head = AMENDED
        .find(&head)
        .map_or(head.as_str(), |m| &head[..m.start()]);
    let lines: Vec<&str> = head
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(8)
        .collect();
    if ACT.is_match(&name) {
        return lines.iter().find_map(|l| numbered(l, false));
    }
    promulgated(head).or_else(|| lines.iter().find_map(|l| numbered(l, true)))
}

fn month(name: &str) -> Option<u32> {
    let folded: String = name
        .chars()
        .map(|c| match c {
            'أ' | 'إ' | 'آ' => 'ا',
            c => c,
        })
        .collect();
    Some(match folded.as_str() {
        "يناير" => 1,
        "فبراير" => 2,
        "مارس" => 3,
        "ابريل" => 4,
        "ماي" | "مايو" => 5,
        "يونيو" | "يونيه" => 6,
        "يوليو" | "يوليوز" => 7,
        "غشت" | "اغسطس" => 8,
        "شتنبر" | "سبتمبر" => 9,
        "اكتوبر" => 10,
        "نونبر" | "نوفمبر" => 11,
        "دجنبر" | "ديسمبر" => 12,
        _ => return None,
    })
}

fn date(c: &regex::Captures) -> Option<String> {
    let day: u32 = match &c[1] {
        "فاتح" => 1,
        d => d.parse().ok()?,
    };
    let iso = format!("{}-{:02}-{day:02}", &c[3], month(&c[2])?);
    lj_core::dates::parse_date_fr(&iso).map(|d| d.to_string())
}

/// The day of the consolidation a text's first page names, `صيغة محينة بتاريخ 16 يوليو 2026`.
pub fn adala_arabic_consolidated(text: &str) -> Option<String> {
    let head: String = text.chars().take(1500).collect();
    date(&CONSOLIDATED.captures(&head)?)
}

/// The first Gregorian day `s` gives after a Hegirian one, `(11 سبتمبر 2003)`.
pub fn adala_arabic_day(s: &str) -> Option<String> {
    DAY.captures_iter(s).find_map(|c| date(&c))
}

/// Where the act a file prints after the dahir promulgating or approving it begins, and the
/// line the dahir is signed on, `وحرر بتطوان في 14 من رجب 1424 (11 سبتمبر 2003).`.
pub fn adala_arabic_promulgation(text: &str) -> Option<(usize, &str)> {
    let mut promulgates = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        at += line.len();
        promulgates |= PROMULGATES.is_match(line);
        if SIGNED.is_match(line) {
            return promulgates.then_some((at, line.trim()));
        }
    }
    None
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty() && !PAGE.is_match(l) && !FOOTNOTE.is_match(l))
        .take_while(|l| !SIGNED.is_match(l))
        .collect()
}

/// An Arabic text's body as lines, up to its signature.
pub fn adala_arabic_body(text: &str) -> String {
    lines(text).join("\n")
}

/// An ordinal in words, `الأولى` 1, `الحادية عشرة` 11, `الثالثة والعشرون` 23.
fn ordinal(words: &str) -> Option<u32> {
    let fold = |w: &str| -> String {
        w.trim_start_matches('و')
            .trim_start_matches("ال")
            .chars()
            .map(|c| match c {
                'أ' | 'إ' | 'آ' => 'ا',
                'ى' => 'ي',
                'ة' => 'ه',
                c => c,
            })
            .collect()
    };
    let unit = |w: &str| -> Option<u32> {
        Some(match w {
            "اول" | "اولي" | "حادي" | "حاديه" => 1,
            "ثاني" | "ثانيه" => 2,
            "ثالث" | "ثالثه" => 3,
            "رابع" | "رابعه" => 4,
            "خامس" | "خامسه" => 5,
            "سادس" | "سادسه" => 6,
            "سابع" | "سابعه" => 7,
            "ثامن" | "ثامنه" => 8,
            "تاسع" | "تاسعه" => 9,
            "عاشر" | "عاشره" => 10,
            _ => return None,
        })
    };
    let ten = |w: &str| -> Option<u32> {
        Some(match w {
            "عشرون" => 20,
            "ثلاثون" => 30,
            "اربعون" => 40,
            "خمسون" => 50,
            "ستون" => 60,
            "سبعون" => 70,
            "ثمانون" => 80,
            "تسعون" => 90,
            _ => return None,
        })
    };
    let words: Vec<String> = words.split_whitespace().map(fold).collect();
    match words.as_slice() {
        [w] if w.starts_with("حادي") => None,
        [w] => unit(w).or_else(|| ten(w)),
        [u, t] if t == "عشر" || t == "عشره" => unit(u).filter(|u| *u < 10).map(|u| 10 + u),
        [u, t] => Some(ten(t)? + unit(u).filter(|u| *u < 10)?),
        _ => None,
    }
}

/// The heads of a text's articles: `المادة` with a figure or an ordinal, or `الفصل` in a text
/// no `المادة` with a figure opens. Word lays some dashed numbers out reversed in an Arabic
/// line, so a dashed number is read both ways.
fn heads(lines: &[String]) -> Vec<Head> {
    let word = match lines.iter().any(|l| {
        HEAD.captures(l).is_some_and(|c| {
            &c[1] == "المادة"
                && (c.get(2).is_some() || c.get(3).is_some_and(|w| ordinal(w.as_str()).is_some()))
        })
    }) {
        true => "المادة",
        false => "الفصل",
    };
    let mut heads = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(c) = HEAD.captures(line) else {
            continue;
        };
        let rest = c.get(6).map_or("", |m| m.as_str());
        if &c[1] != word || RUNNING.is_match(rest) {
            continue;
        }
        let suffix = match (c.get(4), c.get(5).map(|m| m.as_str())) {
            (None, _) => "",
            (Some(_), None) => " bis",
            (Some(_), Some("مرتين")) => " ter",
            (Some(_), Some(_)) => " quater",
        };
        let numbers: Vec<String> = match (c.get(2), c.get(3)) {
            (Some(figures), _) => {
                let parts: Vec<&str> = figures.as_str().split('-').map(str::trim).collect();
                let mut numbers = vec![parts.join("-")];
                if parts.len() > 1 {
                    numbers.push(parts.iter().rev().copied().collect::<Vec<_>>().join("-"));
                }
                numbers
            }
            (None, Some(words)) => match words.as_str() {
                "الفريدة" | "الوحيدة" => vec!["unique".to_string()],
                w => match ordinal(w) {
                    Some(n) => vec![n.to_string()],
                    None => continue,
                },
            },
            (None, None) => continue,
        };
        for (k, number) in numbers.into_iter().enumerate() {
            heads.extend(Head::new(
                i,
                format!("{number}{suffix}"),
                rest.to_string(),
                k > 0,
            ));
        }
    }
    heads
}

fn heading(line: &str) -> Option<usize> {
    if line.chars().count() >= 200 || line.ends_with(['.', '،', '؛']) {
        return None;
    }
    Some(match &HEADING.captures(line)?[1] {
        "جزء" => 0,
        "كتاب" => 1,
        "قسم" => 2,
        "باب" => 3,
        "فصل" => 4,
        _ => 5,
    })
}

/// An Arabic text's numbered articles, each heading (`الكتاب`, `القسم`, `الباب`, `الفصل`,
/// `الفرع`) opening a level of their path.
pub fn adala_arabic_articles(text: &str) -> Vec<ConsolidatedArticle> {
    let lines = lines(text);
    let heads = heads(&lines);
    articles_at(&lines, &heads, heading)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str, path: &str, text: &str) -> Option<(String, &'static str, Option<String>)> {
        adala_arabic_identity(name, path, text).map(|i| (i.code, i.nature, i.number))
    }

    #[test]
    fn an_arabic_file_is_keyed_on_its_act_a_code_on_the_act_that_made_it() {
        assert_eq!(
            id(
                "القانون المتعلق بمدونة الشغل",
                "uploads/2026/09/18/ظهير شريف رقم 1.03.194 بتنفيذ القانون رقم 65.99 المتعلق بمدونة الشغل( تحديث 1)-1789723578772.pdf",
                ""
            ),
            Some(("CODE-TRAVAIL".into(), "CODE", None))
        );
        assert_eq!(
            id(
                "مرسوم بتحديد إجراءات التصريح",
                "uploads/x/مـرسـوم رقـم 2.18.02 بتحديد إجراءات-1790350619720.pdf",
                ""
            ),
            Some(("DECRET".into(), "DECRET", Some("2-18-02".into())))
        );
        assert_eq!(
            id(
                "القانون المتعلق بالمجلس",
                "uploads/x/y.pdf",
                "ظهير شريف رقم 1.14.09 صادر في 4 جمادى الأولى 1435 (6 مارس 2014) بتنفيذ القانون التنظيمي رقم 065.13"
            ),
            Some(("LOI".into(), "LOI_ORGANIQUE", Some("065-13".into())))
        );
        assert_eq!(
            id(
                "قرار لوزير الاقتصاد والمالية بالمصادقة على منشور",
                "uploads/x/قرار لوزير الاقتصاد والمالية رقم 366.19 بالمصادقة-1762264551126.pdf",
                ""
            ),
            Some(("ARRETE".into(), "ARRETE", Some("366-19".into())))
        );
        assert_eq!(
            id(
                "قرار لوزير العدل تحدد بموجبه المميزات",
                "uploads/x/y.pdf",
                "قرار لوزير العدل بتطبيق القانون رقم 28.07\nالنص"
            ),
            None
        );
        assert_eq!(
            id(
                "مرسوم بتطبيق القانون المتعلق بمجلس المنافسة",
                "uploads/x/y.pdf",
                "القانون رقم 104.12\nمرسوم رقم 2.14.652 صادر في 8 صفر 1436 (فاتح ديسمبر 2014)"
            ),
            Some(("DECRET".into(), "DECRET", Some("2-14-652".into())))
        );
        assert_eq!(
            id("ظهير بمثابة قانون الالتزامات والعقود", "uploads/x/y.pdf", ""),
            Some(("CODE-OBLIGATIONS-CONTRATS".into(), "CODE", None))
        );
        assert_eq!(
            id(
                "دستور المملكة المغربية لسنة 2011",
                "uploads/x/الدستور.pdf",
                ""
            ),
            Some(("CONSTITUTION-2011".into(), "CONSTITUTION", None))
        );
        assert_eq!(
            id("قرار المحكمة الدستورية رقم 256.25", "uploads/x/y.pdf", ""),
            None
        );
        let amended = "مجموعة القانون الجنائي\nصيغة محينة بتاريخ 22 غشت 2024\n\
            ظهير شريف رقم 1.59.413 صادر في 28 جمادى الثانية 1382\n(26 نونبر 1962) بالمصادقة على مجموعة القانون الجنائي\n\
            كما تم تعديله بـ:\n1- القانون رقم 43.22 المتعلق بالعقوبات البديلة";
        assert_eq!(
            id("مجموعة القانون الجنائي", "uploads/x/y.pdf", amended),
            Some(("CODE-PENAL".into(), "CODE", None))
        );
        assert_eq!(
            adala_arabic_consolidated(amended).as_deref(),
            Some("2024-08-22")
        );
        assert_eq!(adala_arabic_day(amended).as_deref(), Some("1962-11-26"));
    }

    #[test]
    fn the_text_layer_reads_as_typed() {
        assert_eq!(
            adala_arabic_text("بـمـوجـب\u{202B}ه\u{202C} \u{FEFB} ٤٥"),
            "بموجبه لا 45"
        );
    }

    #[test]
    fn arabic_articles_are_cut_on_their_heads_in_sequence() {
        let text = "ظهير شريف رقم 1.03.194\nينفذ وينشر بالجريدة الرسمية القانون رقم 65.99.\n\
            وحرر بتطوان في 14 من رجب 1424 (11 سبتمبر 2003).\n- 3 -\n\
            الكتاب الأول\nالمادة الأولى\nيعتبر أجيرا كل شخص.\n\
            1 - الجريدة الرسمية عدد 5167 بتاريخ 13 شوال 1424\n\
            المادة 2\nتطبق أحكام المادة 1 من هذا القانون.\nالمادة 5 من القانون رقم 12.00 تنسخ.\n\
            المادة 2 مكررة - نص مكرر.\nالمادة 1-3\nنص المادة 3-1.\nالمادة الرابعة\nنص.\n";
        let (at, signed) = adala_arabic_promulgation(text).unwrap();
        assert!(signed.starts_with("وحرر بتطوان"));
        let articles = adala_arabic_articles(&text[at..]);
        let nums: Vec<(&str, &str)> = articles
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str()))
            .collect();
        assert_eq!(
            nums,
            [
                ("1", "يعتبر أجيرا كل شخص."),
                (
                    "2",
                    "تطبق أحكام المادة 1 من هذا القانون.\nالمادة 5 من القانون رقم 12.00 تنسخ."
                ),
                ("2 bis", "نص مكرر."),
                ("3-1", "نص المادة 3-1."),
                ("4", "نص.")
            ]
        );
        assert_eq!(articles[0].title_path.as_deref(), Some("الكتاب الأول"));
        assert_eq!(ordinal("الحادية عشرة"), Some(11));
        assert_eq!(ordinal("الثالثة والعشرون"), Some(23));
        assert_eq!(ordinal("المذكورة"), None);
    }

    #[test]
    fn a_text_whose_articles_are_fusul_keeps_its_fasl_heads() {
        let articles = adala_arabic_articles("الفصل الأول\nنص أول.\nالفصل 2\nنص ثان.\n");
        let nums: Vec<&str> = articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(nums, ["1", "2"]);
        let articles =
            adala_arabic_articles("الفصل الأول: أحكام عامة\nالمادة 1\nنص.\nالمادة 2\nنص.\n");
        assert_eq!(
            articles[1].title_path.as_deref(),
            Some("الفصل الأول: أحكام عامة")
        );
    }

    #[test]
    fn a_decree_numbered_in_words_is_cut_on_its_madda_and_approves_nothing_it_annexes() {
        let text = "مرسوم رقم 2.26.816\nبناء على الدستور، ولاسيما الفصل 93 منه؛\n\
            رسمت ما يلي:\nالمادة الأولى\nيصادق على البرنامج.\nالمادة الثانية\nنص.\n\
            وحرر بالرباط في 14 من جمادى الأولى 1448 (26 أكتوبر 2026).\n";
        assert_eq!(adala_arabic_promulgation(text), None);
        let nums: Vec<String> = adala_arabic_articles(text)
            .into_iter()
            .map(|a| a.num)
            .collect();
        assert_eq!(nums, ["1", "2"]);
        let dahir =
            "ينفد وينشر بالجريدة الرسمية القانون رقم 63.17.\nوحرر بتطوان في 7 ذي الحجة 1440.\n";
        assert!(adala_arabic_promulgation(dahir).is_some());
    }
}
