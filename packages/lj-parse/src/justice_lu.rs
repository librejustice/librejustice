//! A Luxembourg decision published by the Administration judiciaire on data.public.lu into a
//! [`Decision`] and its `source_fields`: the court from its dataset, the date and the numbers from
//! the head of its text, the file name's date when the head gives none.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

use crate::entscheidsuche::language;
use crate::reflow_page_text;

/// A court of the judicial order, by the slugs of the datasets that hold its decisions.
#[derive(Debug)]
pub struct LuCourt {
    pub slugs: &'static [&'static str],
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
}

pub const LU_PLACE: &str = "geo:LU";

/// Every dataset of the Administration judiciaire but the JUDOC summaries opens on one of these.
pub const LU_COURTS: [LuCourt; 8] = [
    LuCourt {
        slugs: &["cour-de-cassation"],
        jurisdiction_type: "COUR_CASSATION",
        code: "lu_cour_cassation",
        label: "Cour de cassation (Luxembourg)",
    },
    LuCourt {
        slugs: &["cour-superieure-de-justice"],
        jurisdiction_type: "COUR_APPEL",
        code: "lu_cour_appel",
        label: "Cour d'appel (Luxembourg)",
    },
    LuCourt {
        slugs: &["conseil-superieur-de-la-securite-sociale"],
        jurisdiction_type: "COUR_APPEL",
        code: "lu_cour_appel_securite_sociale",
        label: "Conseil supérieur de la sécurité sociale (Luxembourg)",
    },
    LuCourt {
        slugs: &["tribunal-darrondissement-luxembourg"],
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "lu_tribunal_premiere_instance_luxembourg",
        label: "Tribunal d'arrondissement de Luxembourg (Luxembourg)",
    },
    LuCourt {
        slugs: &["tribunal-darrondissement-diekirch"],
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "lu_tribunal_premiere_instance_diekirch",
        label: "Tribunal d'arrondissement de Diekirch (Luxembourg)",
    },
    LuCourt {
        slugs: &["justice-de-paix-luxembourg"],
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "lu_tribunal_premiere_instance_justice_paix_luxembourg",
        label: "Justice de paix de Luxembourg (Luxembourg)",
    },
    LuCourt {
        slugs: &["justice-de-paix-esch-sur-alzette"],
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "lu_tribunal_premiere_instance_justice_paix_esch_sur_alzette",
        label: "Justice de paix d'Esch-sur-Alzette (Luxembourg)",
    },
    LuCourt {
        slugs: &["justice-de-paix-diekirch", "justice-de-paix-diekrich"],
        jurisdiction_type: "TRIBUNAL_PREMIERE_INSTANCE",
        code: "lu_tribunal_premiere_instance_justice_paix_diekirch",
        label: "Justice de paix de Diekirch (Luxembourg)",
    },
];

/// The court whose decisions a dataset holds, by its slug.
pub fn lu_court(dataset: &str) -> Option<&'static LuCourt> {
    LU_COURTS
        .iter()
        .find(|c| c.slugs.iter().any(|s| dataset.starts_with(s)))
}

fn lu_court_by_code(code: &str) -> Option<&'static LuCourt> {
    LU_COURTS.iter().find(|c| c.code == code)
}

/// A file of a dataset's yearly archive.
#[derive(Debug, Clone, Copy)]
pub struct LuMember<'a> {
    pub dataset: &'a str,
    pub dataset_title: &'a str,
    /// The archive the file sits in, `2026.zip`.
    pub archive: &'a str,
    pub name: &'a str,
    /// `pdf` or `html`.
    pub format: &'a str,
}

#[derive(Debug)]
pub struct JusticeLuParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static LuCourt,
}

/// The non-blank characters a decision text holds at least.
const DECISION_FLOOR: usize = 200;
/// The lines of the text head the date and the registry number are read from.
const HEAD_LINES: usize = 14;
/// The lines that open on the decision's own number, before the citations of other decisions.
const NUMBER_LINES: usize = 3;

const MONTHS: &str = "janvier|février|fevrier|mars|avril|mai|juin|juillet|août|aout|septembre|octobre|novembre|décembre|decembre";
const WEEKDAY: &str = r"(?:(?:lundi|mardi|mercredi|jeudi|vendredi|samedi|dimanche),?\s+)?";
pub(crate) const SMALL: &str = r"(?:un|deux|trois|quatre|cinq|six|sept|huit|neuf|dix(?:sept|huit|neuf)?|onze|douze|treize|quatorze|quinze|seize|vingts?|trente|quarante|cinquante|soixante|et)\b";

static NUMERIC_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bdu\s+(\d{1,2})\s?[./]\s?(\d{1,2})\s?[./]\s?(\d{4})\b").expect("regex")
});
static DIGIT_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\bdu\s+{WEEKDAY}(1er|\d{{1,2}})\s+({MONTHS})\s+(\d{{4}})\b"
    ))
    .expect("regex")
});
static SPELLED_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\bdu\s+{WEEKDAY}(premier|{SMALL}(?:[\s-]+{SMALL})*)\s+({MONTHS})\s+((?:deux\s+mil(?:le)?|mil(?:le)?\s+neuf\s+cents?)(?:[\s-]+{SMALL})*)"
    ))
    .expect("regex")
});
static FILE_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})(\d{2})(\d{2})_").expect("regex"));
static REGISTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bnum[ée]ro\s+([A-Z]{2,5}-\d{4}-\d{3,6}(?:-\d+)?|\d{1,6})\s+du\s+(?:registre|r[ôo]le)\b",
    )
    .expect("regex")
});
static REGISTRY_SHORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bno\.?\s*du\s+reg\.?\s*:\s*([A-Z]{2,5}\s*\d{4}/\d{2,5})").expect("regex")
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\bn[°o]\.?\s*:?\s*|\b(?:civil|commercial)\s+)([0-9]+[A-Z0-9]*\s*/\s*[0-9]+)",
    )
    .expect("regex")
});

fn month_number(month: &str) -> Option<i8> {
    Some(match month.to_lowercase().as_str() {
        "janvier" => 1,
        "février" | "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" | "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "décembre" | "decembre" => 12,
        _ => return None,
    })
}

/// A French number from 1 to 99 in words, `quatre-vingt` read as one word, `dixhuit` as `dix-huit`.
pub(crate) fn small_number(words: &str) -> Option<i16> {
    let words = words
        .to_lowercase()
        .replace('-', " ")
        .replace("quatre vingts", "qv")
        .replace("quatre vingt", "qv");
    let mut total = 0;
    for word in words.split_whitespace().filter(|w| *w != "et") {
        total += match word {
            "premier" | "un" => 1,
            "deux" => 2,
            "trois" => 3,
            "quatre" => 4,
            "cinq" => 5,
            "six" => 6,
            "sept" => 7,
            "huit" => 8,
            "neuf" => 9,
            "dix" => 10,
            "dixsept" => 17,
            "dixhuit" => 18,
            "dixneuf" => 19,
            "onze" => 11,
            "douze" => 12,
            "treize" => 13,
            "quatorze" => 14,
            "quinze" => 15,
            "seize" => 16,
            "vingt" => 20,
            "trente" => 30,
            "quarante" => 40,
            "cinquante" => 50,
            "soixante" => 60,
            "qv" => 80,
            _ => return None,
        };
    }
    (total > 0).then_some(total)
}

pub(crate) fn year_in_words(words: &str) -> Option<i16> {
    let words = words.to_lowercase().replace('-', " ");
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    for (prefix, base) in [
        ("deux mille", 2000),
        ("deux mil", 2000),
        ("mille neuf cents", 1900),
        ("mil neuf cents", 1900),
        ("mille neuf cent", 1900),
        ("mil neuf cent", 1900),
    ] {
        if let Some(rest) = words.strip_prefix(prefix) {
            let rest = rest.trim();
            return if rest.is_empty() {
                Some(base)
            } else {
                Some(base + small_number(rest)?)
            };
        }
    }
    None
}

fn iso(year: i16, month: i8, day: i8) -> Option<String> {
    jiff::civil::Date::new(year, month, day)
        .ok()
        .map(|d| d.to_string())
}

/// The first date the head states, `du 08.01.2026`, `du jeudi, 8 janvier 2026` or `du six février
/// deux mille vingt-six`.
fn head_date(head: &str) -> Option<String> {
    let numeric = NUMERIC_DATE.captures(head).and_then(|c| {
        let date = iso(c[3].parse().ok()?, c[2].parse().ok()?, c[1].parse().ok()?)?;
        Some((c.get(0)?.start(), date))
    });
    let digits = DIGIT_DATE.captures(head).and_then(|c| {
        let day = if c[1].eq_ignore_ascii_case("1er") {
            1
        } else {
            c[1].parse().ok()?
        };
        let date = iso(c[3].parse().ok()?, month_number(&c[2])?, day)?;
        Some((c.get(0)?.start(), date))
    });
    let spelled = SPELLED_DATE.captures(head).and_then(|c| {
        let day = small_number(&c[1]).filter(|d| (1..=31).contains(d))?;
        let date = iso(year_in_words(&c[3])?, month_number(&c[2])?, day as i8)?;
        Some((c.get(0)?.start(), date))
    });
    [numeric, digits, spelled]
        .into_iter()
        .flatten()
        .min_by_key(|(at, _)| *at)
        .map(|(_, date)| date)
}

fn file_date(name: &str) -> Option<String> {
    let c = FILE_DATE.captures(name)?;
    iso(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)
}

/// The decision's number first, `9/26`, read from the head's first lines, then its registry or
/// roll number, `CAL-2024-01041`, the first the head names: the next one is the attacked
/// decision's.
fn head_numbers(head: &[&str]) -> Vec<String> {
    let opening = head[..head.len().min(NUMBER_LINES)].join(" ");
    let whole = head.join(" ");
    let mut numbers: Vec<String> = NUMBER
        .captures(&opening)
        .map(|c| c[1].split_whitespace().collect())
        .into_iter()
        .collect();
    let registry = REGISTRY
        .captures(&whole)
        .map(|c| c[1].to_string())
        .or_else(|| {
            REGISTRY_SHORT
                .captures(&whole)
                .map(|c| c[1].split_whitespace().collect::<Vec<_>>().join(" "))
        });
    if let Some(registry) = registry.filter(|r| !numbers.contains(r)) {
        numbers.push(registry);
    }
    numbers
}

/// The file name without its extension and the `-accessible` and `_pseudonymisé` the publisher
/// appends, every character but a letter, a digit, `.`, `_` and `-` read as `_`.
fn stem(name: &str) -> String {
    let base = name.rsplit_once('.').map_or(name, |(base, _)| base);
    let base = base.strip_suffix("-accessible").unwrap_or(base);
    let base = base.rfind("_pseudonymis").map_or(base, |at| &base[..at]);
    base.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The chamber the dataset's title names after the court, `Chambre 2 Civil`.
fn chamber_of(dataset_title: &str) -> Option<String> {
    dataset_title
        .split_once(" - ")
        .map(|(_, chamber)| chamber.trim().to_string())
        .filter(|c| !c.is_empty())
}

fn decision_of(
    source_uid: String,
    member_name: String,
    court: &LuCourt,
    chamber: Option<String>,
    date: String,
    numbers: Vec<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid,
        member_name,
        ecli: None,
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: numbers.first().cloned(),
        numero_dossiers: (!numbers.is_empty()).then_some(numbers),
        numero_role: None,
        date_lecture: Some(date),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

fn numbers_of(source_fields: &Value) -> Vec<String> {
    source_fields
        .get("numbers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Parses a file's text: a PDF's as `pdftotext` reads it, an HTML's one paragraph a line.
pub fn parse_justice_lu(member: &LuMember<'_>, text: &str) -> Result<JusticeLuParsed> {
    let fail = |why: String| {
        CoreError::Xml(format!(
            "justice-lu {}/{}: {why}",
            member.dataset, member.name
        ))
    };
    let court =
        lu_court(member.dataset).ok_or_else(|| fail("no court holds this dataset".into()))?;
    let text = match member.format {
        "pdf" => reflow_page_text(text),
        "html" => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        other => return Err(fail(format!("format {other}"))),
    };
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(fail("no decision text".into()));
    }
    let head_lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(HEAD_LINES)
        .collect();
    let head = head_lines.join(" ");
    let stem = stem(member.name);
    // An archive holds a year's decisions: a date in its year wins, the head's before the file
    // name's, which the clerk types by hand.
    let year = member.archive.strip_suffix(".zip").map(|y| format!("{y}-"));
    let dates: Vec<String> = [head_date(&head), file_date(&stem)]
        .into_iter()
        .flatten()
        .collect();
    let date = dates
        .iter()
        .find(|d| year.as_ref().is_some_and(|y| d.starts_with(y.as_str())))
        .or(dates.first())
        .cloned()
        .ok_or_else(|| fail("no date".into()))?;
    let numbers = head_numbers(&head_lines);
    let chamber = chamber_of(member.dataset_title);
    let source_uid = format!("justice-lu/{}/{stem}", member.dataset);
    let source_fields = json!({
        "dataset": member.dataset,
        "dataset_title": member.dataset_title,
        "archive": member.archive,
        "file": member.name,
        "format": member.format,
        "court": court.code,
        "chamber": chamber,
        "date": date,
        "numbers": numbers,
        "source_url": format!("https://data.public.lu/fr/datasets/{}/", member.dataset),
        "lang": language(&text),
    });
    let decision = decision_of(source_uid, stem, court, chamber, date, numbers, text);
    Ok(JusticeLuParsed {
        decision,
        source_fields,
        court,
    })
}

/// Branch of [`Decision::from_source_fields`] for `justice-lu/`, on what the parse stored.
pub(crate) fn from_source_fields_justice_lu(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let court = field("court")
        .and_then(lu_court_by_code)
        .unwrap_or_else(|| panic!("stored justice-lu court not taken ({source_uid})"));
    let date = field("date")
        .unwrap_or_else(|| panic!("source_fields justice-lu without a date ({source_uid})"));
    let stem = source_uid.rsplit('/').next().unwrap_or(source_uid);
    decision_of(
        source_uid.to_string(),
        stem.to_string(),
        court,
        field("chamber").map(str::to_string),
        date.to_string(),
        numbers_of(source_fields),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "La Cour de cassation considère que le moyen n'est pas fondé et que \
        l'arrêt attaqué a fait une exacte application de la loi en retenant que le contrat avait \
        été résilié aux torts du demandeur. Par ces motifs, la Cour rejette le pourvoi, condamne \
        le demandeur en cassation aux frais et dépens de l'instance en cassation.";

    fn member<'a>(
        dataset: &'a str,
        title: &'a str,
        archive: &'a str,
        name: &'a str,
    ) -> LuMember<'a> {
        LuMember {
            dataset,
            dataset_title: title,
            archive,
            name,
            format: "pdf",
        }
    }

    #[test]
    fn the_head_dates_a_decision_whose_file_name_misdates_it() {
        let head = "Arrêt N° 9/26 - XII- COM\nAudience publique du six février deux mille \
            vingt-six\nNuméro CAL-2024-01041 du rôle\nComposition:\n";
        let parsed = parse_justice_lu(
            &member(
                "cour-superieure-de-justice-chambre-12",
                "Cour supérieure de justice - Chambre 12",
                "2026.zip",
                "20250206_CA12_CAL-2024-01041_pseudonymisé-accessible.pdf",
            ),
            &format!("{head}{BODY}"),
        )
        .unwrap();
        let d = &parsed.decision;
        assert_eq!(d.date_lecture.as_deref(), Some("2026-02-06"));
        assert_eq!(
            d.numero_dossiers,
            Some(vec!["9/26".to_string(), "CAL-2024-01041".to_string()])
        );
        assert_eq!(d.chamber.as_deref(), Some("Chambre 12"));
        assert_eq!(
            d.source_uid,
            "justice-lu/cour-superieure-de-justice-chambre-12/20250206_CA12_CAL-2024-01041"
        );
        assert_eq!(d.jurisdiction_source_code.as_deref(), Some("lu_cour_appel"));
        let rebuilt = from_source_fields_justice_lu(
            &d.texte_integral_clean,
            &parsed.source_fields,
            &d.source_uid,
        );
        assert_eq!(rebuilt.numero_dossiers, d.numero_dossiers);
        assert_eq!(rebuilt.date_lecture, d.date_lecture);
        assert_eq!(rebuilt.chamber, d.chamber);
    }

    #[test]
    fn each_head_the_courts_write_gives_its_date() {
        for (head, date) in [
            ("N° 01 / 2026\ndu 08.01.2026\nNuméro CAS-2025-00093 du registre", "2026-01-08"),
            ("N° 14 / 07.\ndu 1.3.2007.\nNuméro 2405 du registre.", "2007-03-01"),
            ("N° 1/26\ndu 5 janvier 2026\nAudience publique du lundi, cinq janvier", "2026-01-05"),
            ("No. 1/2026\nAudience publique du jeudi, 8 janvier 2026\n(Not. 4914/24/XD)", "2026-01-08"),
            ("Jugement civil no. 2026TALCH24/00002\nAudience publique du jeudi, vingt-neuf janvier deux mille vingt-six.", "2026-01-29"),
            ("No. du reg.: CARE 2025/0132\nNo. : 2026/0012\nCONSEIL SUPERIEUR DE LA SECURITE SOCIALE\nAudience publique du huit janvier deux mille vingt-six", "2026-01-08"),
            ("N° 13/99.\ndu 11.02.1999.\nAudience publique du jeudi, onze février mil neuf cent quatre-ym<^-dixneuf.", "1999-02-11"),
        ] {
            let head: String = head.lines().collect::<Vec<_>>().join(" ");
            assert_eq!(head_date(&head).as_deref(), Some(date), "{head}");
        }
    }

    #[test]
    fn each_head_the_courts_write_gives_its_numbers() {
        for (head, numbers) in [
            ("N° 01 / 2026\ndu 08.01.2026\nNuméro CAS-2025-00093 du registre\nVu l'arrêt attaqué rendu le 12 juin 2025 sous le numéro CAL-2023-00408 du rôle", vec!["01/2026", "CAS-2025-00093"]),
            ("Jugement civil no. 2026TALCH24/00002\nAudience publique du jeudi\nNuméro TAL-2025-07526 du rôle", vec!["2026TALCH24/00002", "TAL-2025-07526"]),
            ("Jugement civil 2026TALCH24/00003\nAudience publique du jeudi\nNuméro TAL-2024-00114 du rôle\nComposition :\nle jugement no 155/24 du 3 mai 2024", vec!["2026TALCH24/00003", "TAL-2024-00114"]),
            ("GRAND-DUCHE DU LUXEMBOURG\nNo. du reg.: CARE 2025/0132\nNo. : 2026/0012", vec!["2026/0012", "CARE 2025/0132"]),
            ("N° 14 / 07.\ndu 1.3.2007.\nNuméro 2405 du registre.", vec!["14/07", "2405"]),
        ] {
            let lines: Vec<&str> = head.lines().collect();
            assert_eq!(head_numbers(&lines), numbers, "{head}");
        }
    }

    #[test]
    fn a_file_name_loses_the_marks_the_publisher_appends() {
        assert_eq!(
            stem("20260925_CAS-2026-00114_156_pseudonymisée-accessible.pdf"),
            "20260925_CAS-2026-00114_156"
        );
        assert_eq!(
            stem("20260226 _ CCSS20250228 + CCSS20250243-accessible.pdf"),
            "20260226___CCSS20250228___CCSS20250243"
        );
    }

    #[test]
    fn a_head_dated_outside_its_archive_s_year_yields_to_the_file_name() {
        let cass = |name, text: &str| {
            parse_justice_lu(
                &member("cour-de-cassation-1", "Cour de Cassation", "1985.zip", name),
                &format!("{text}\n{BODY}"),
            )
            .unwrap()
            .decision
        };
        let unread = cass(
            "19850117_CASS_580_pseudonymisé-accessible.pdf",
            "Audience publique du dix-sept janvier mil neuf cent\n/ p\\\nquatre-vingt-cinq.",
        );
        assert_eq!(unread.date_lecture.as_deref(), Some("1985-01-17"));
        let undated = cass(
            "19850117_CASS_595_pseudonymisé-accessible.pdf",
            "JV 2/8^ porie\ndu :!7c1o1985.",
        );
        assert_eq!(undated.date_lecture.as_deref(), Some("1985-01-17"));
        assert_eq!(undated.chamber, None);
    }

    #[test]
    fn a_dataset_no_court_holds_is_refused() {
        let summary = member(
            "extraits-de-decisions-judiciaires-fiches-judoc",
            "Extraits de décisions judiciaires (fiches JUDOC)",
            "judoc.zip",
            "20230524_1.pdf",
        );
        assert!(parse_justice_lu(&summary, &format!("du 24.05.2023\n{BODY}")).is_err());
    }
}
