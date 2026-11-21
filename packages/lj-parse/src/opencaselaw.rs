//! An Obwalden entry of OpenCaseLaw's dataset into a [`Decision`]: the body that ruled and the
//! day, read from the line of the entry that names them, the docket from the report's reference.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use lj_core::decision::Decision;

use crate::entscheidsuche::{court, decision_of, language, plain_text, SwissCourt};

/// The bodies an Obwalden entry is ruled by, by an id of ours.
pub const OBWALDEN_COURTS: [SwissCourt; 10] = [
    court(
        "OW_OG",
        "COUR_APPEL",
        "ch_cour_appel_obwald",
        "Tribunal supérieur (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_obwald",
        "Tribunal administratif (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_KG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_obwald",
        "Tribunal cantonal (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_VSG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_obwald_assurances",
        "Tribunal des assurances (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_SRK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_obwald_fiscal",
        "Commission de recours en matière fiscale (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_SK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_obwald_estimation",
        "Commission d'estimation en matière d'expropriation (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_RR",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_obwald_conseil_etat",
        "Conseil d'État (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_DEP",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_obwald",
        "Départements et offices (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_KESB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_obwald_protection_enfant_adulte",
        "Autorité de protection de l'enfant et de l'adulte (Obwald, Suisse)",
        "geo:CH-OW",
    ),
    court(
        "OW_AK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_obwald_avocats",
        "Commission des avocats (Obwald, Suisse)",
        "geo:CH-OW",
    ),
];

/// Each Obwalden body's German name, the one its decisions write.
pub fn obwalden_names() -> Vec<(&'static str, Value)> {
    OBWALDEN_COURTS
        .iter()
        .map(|court| {
            let name = match court.id {
                "OW_OG" => "Obergericht",
                "OW_VG" => "Verwaltungsgericht",
                "OW_KG" => "Kantonsgericht",
                "OW_VSG" => "Versicherungsgericht",
                "OW_SRK" => "Steuerrekurskommission",
                "OW_SK" => "Schätzungskommission in Enteignungssachen",
                "OW_RR" => "Regierungsrat",
                "OW_DEP" => "Departemente und Ämter",
                "OW_KESB" => "Kindes- und Erwachsenenschutzbehörde",
                "OW_AK" => "Anwaltskommission",
                other => panic!("an Obwalden body without a German name: {other}"),
            };
            let named = json!({ "de": { "name": format!("{name} (Obwalden, Schweiz)") } });
            (court.code, named)
        })
        .collect()
}

fn obwalden_court(id: &str) -> Option<&'static SwissCourt> {
    OBWALDEN_COURTS.iter().find(|c| c.id == id)
}

/// The body an entry's line names, by the words it opens on once its article is cut:
/// `des Verwaltungsgerichts`, `der Obergerichtskommission`, `des Amts für Justiz`.
fn issuer_court(issuer: &str) -> Option<&'static str> {
    const OPENINGS: [(&str, &str); 12] = [
        ("verwaltungsgericht", "OW_VG"),
        ("obergericht", "OW_OG"),
        ("og", "OW_OG"),
        ("kantonsgericht", "OW_KG"),
        ("rekurskommission für sozialversicherung", "OW_VSG"),
        ("versicherungsgericht", "OW_VSG"),
        ("steuerrekurskommission", "OW_SRK"),
        ("schätzungskommission", "OW_SK"),
        ("regierungsrat", "OW_RR"),
        ("kindes- und erwachsenen", "OW_KESB"),
        ("anwaltskommission", "OW_AK"),
        ("bodenrechtskommission", "OW_DEP"),
    ];
    let words = issuer.trim().to_lowercase();
    let words = ["des ", "der ", "dem "]
        .iter()
        .find_map(|a| words.strip_prefix(a))
        .unwrap_or(&words);
    let words = ["kantonalen ", "obwaldner "]
        .iter()
        .find_map(|a| words.strip_prefix(a))
        .unwrap_or(words);
    OPENINGS
        .iter()
        .find(|(opening, _)| {
            words
                .strip_prefix(opening)
                .is_some_and(|rest| *opening != "og" || rest.is_empty())
        })
        .map(|(_, id)| *id)
        .or_else(|| (words.starts_with("amt") || words.contains("departement")).then_some("OW_DEP"))
}

/// The line of an entry naming its ruling, its body and its day: `Entscheid des
/// Verwaltungsgerichts vom 26./27. Mai 2015 (B 14/005).`, `Urteil der Obergerichtskommission
/// vom 20. Mai 1977`, `Entscheid vom 27.10.1970 i.S. Rekurs F. A. gegen Gemeinderat Sarnen.`
static ACT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*(?:Teil|Zwischen|End|Präsidial|Abschreibungs)?(?:[Ee]ntscheid|[Uu]rteil|[Bb]eschluss|[Vv]erfügung)e?\b(?P<issuer>[^\n]{0,160}?)[ \t]+(?:vom|vorn|v\.)[ \t]*(?P<dates>\d[^\n]{0,400})$",
    )
    .expect("ACT_LINE compiles")
});

static NAMED_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?P<day>\d{1,2})\.?[ \t]*(?P<month>januar|februar|märz|april|mai|juni|juli|august|september|oktober|november|dezember)[ \t]+(?P<year>\d{4})")
        .expect("NAMED_DATE compiles")
});

static NUMERIC_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?P<day>\d{1,2})\.(?P<month>\d{1,2})\.(?P<year>\d{4}|\d{2})\b")
        .expect("NUMERIC_DATE compiles")
});

/// A case number in brackets after the day: `(B 14/005)`.
static CASE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((?P<case>[A-Z][A-Za-z]{0,5} ?\d{2,4}/\d{1,4})\)").expect("CASE_NUMBER compiles")
});

/// The report's reference, `VVGE 1966/70 Nr. 19`, `OGVE-2014-15-54.htm#`.
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<report>AbR|VVGE|OGVE)[ -](?P<from>\d{4})[/_-](?P<to>\d{2,4})[ -](?:Nr\.? ?)?(?P<number>\d+)\b")
        .expect("REFERENCE compiles")
});

fn german_month(month: &str) -> Option<i8> {
    Some(match month.to_lowercase().as_str() {
        "januar" => 1,
        "februar" => 2,
        "märz" => 3,
        "april" => 4,
        "mai" => 5,
        "juni" => 6,
        "juli" => 7,
        "august" => 8,
        "september" => 9,
        "oktober" => 10,
        "november" => 11,
        "dezember" => 12,
        _ => return None,
    })
}

/// The last day the line states before it names another court: a judgment deliberated over two
/// days takes the second, `vom 15./18. März 1982`, a corrected day its correction, `vom 23. Juli
/// 1988 [recte: 23. Juni 1988]`, and `vom 15. April 1985 und des Bundesgerichts vom 16. März 1986`
/// or `vom 31. Oktober 2019 (VK 19/003); vom Bundesgericht [...] vom 4. Mai 2020` the first.
fn act_date(dates: &str) -> Option<jiff::civil::Date> {
    let dates = dates.split(" und de").next().unwrap_or(dates);
    let dates = dates.split(';').next().unwrap_or(dates);
    let named = NAMED_DATE.captures_iter(dates).filter_map(|c| {
        Some((
            c.get(0)?.start(),
            c["year"].parse::<i16>().ok()?,
            german_month(&c["month"])?,
            c["day"].parse::<i8>().ok()?,
        ))
    });
    let numeric = NUMERIC_DATE.captures_iter(dates).filter_map(|c| {
        let year: i16 = c["year"].parse().ok()?;
        let year = match year {
            0..=29 => 2000 + year,
            30..=99 => 1900 + year,
            _ => year,
        };
        Some((
            c.get(0)?.start(),
            year,
            c["month"].parse::<i8>().ok()?,
            c["day"].parse::<i8>().ok()?,
        ))
    });
    let (_, year, month, day) = named.chain(numeric).max_by_key(|(at, ..)| *at)?;
    jiff::civil::Date::new(year, month, day).ok()
}

/// Why an entry is not loaded.
#[derive(Debug, PartialEq)]
pub enum OpenCaseLawRefused {
    /// An essay, the register: its reference names no entry of a report.
    NotAnEntry,
    /// An opinion, a report, a rule text: no line names a ruling.
    NoRuling,
    /// Its line states no day.
    NoDay,
    /// Its line's day falls decades before its report's years, a misprint: `vom 2. März 1887` in
    /// the VVGE 1987/88.
    DayBeforeReport,
    /// A body outside [`OBWALDEN_COURTS`], as its line names it.
    BodyNotTaken(String),
}

#[derive(Debug)]
pub struct OpenCaseLawParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static SwissCourt,
}

/// The first characters of an entry its line naming the ruling sits in, after the reference,
/// the statutes and the headnote.
const HEAD: usize = 4_000;

/// Parses a row of a court's file. An entry that names no ruling, an opinion, a report, a rule
/// text, an essay, or that a body outside [`OBWALDEN_COURTS`] ruled, is refused.
pub fn parse_opencaselaw(
    row: &Map<String, Value>,
) -> Result<OpenCaseLawParsed, OpenCaseLawRefused> {
    let field = |k: &str| row.get(k).and_then(Value::as_str).map(str::trim);
    let id = field("decision_id").expect("decision_id is a column without nulls");
    let (reference, first_year) = REFERENCE
        .captures(field("docket_number").unwrap_or_default())
        .map(|c| {
            let to = &c["to"];
            let reference = format!(
                "{} {}/{} Nr. {}",
                &c["report"],
                &c["from"],
                &to[to.len() - 2..],
                &c["number"]
            );
            (reference, c["from"].parse::<i16>().expect("four digits"))
        })
        .ok_or(OpenCaseLawRefused::NotAnEntry)?;
    let text = plain_text(field("full_text").unwrap_or_default());
    let head: String = text.chars().take(HEAD).collect();
    let act = ACT_LINE
        .captures(&head)
        .ok_or(OpenCaseLawRefused::NoRuling)?;
    let date = act_date(&act["dates"]).ok_or(OpenCaseLawRefused::NoDay)?;
    // Over the 2,206 entries of February 2026, a day falls at most two years before its report's;
    // the four that fall 29 to 100 years before carry a digit misprinted, 1955 in the VVGE 1985/86.
    if date.year() < first_year - 5 {
        return Err(OpenCaseLawRefused::DayBeforeReport);
    }
    let issuer = act["issuer"].trim();
    let court_id = if issuer.is_empty() {
        (reference.starts_with("VVGE") && date < jiff::civil::date(1974, 1, 1)).then_some("OW_RR")
    } else {
        issuer_court(issuer)
    }
    .ok_or_else(|| OpenCaseLawRefused::BodyNotTaken(issuer.to_string()))?;
    let mut dockets = vec![reference.clone()];
    if let Some(case) = CASE_NUMBER.captures(&act["dates"]) {
        dockets.push(case["case"].to_string());
    }
    let source_fields = json!({
        "opencaselaw_id": id,
        "court": court_id,
        "issuer": issuer,
        "act_line": act[0].trim(),
        "date": date.to_string(),
        "dockets": dockets,
        "regeste": field("regeste"),
        "dataset_date": field("decision_date"),
        "source_url": field("source_url"),
        "lang": language(&text),
    });
    let court = obwalden_court(court_id).expect("an issuer names a court of the table");
    Ok(OpenCaseLawParsed {
        decision: decision_of(
            format!("opencaselaw/{id}"),
            id,
            court,
            None,
            date.to_string(),
            dockets,
            text,
        ),
        source_fields,
        court,
    })
}

/// Branch of [`Decision::from_source_fields`] for `opencaselaw/`, on what the parse stored.
pub(crate) fn from_source_fields_opencaselaw(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let id = source_uid
        .strip_prefix("opencaselaw/")
        .unwrap_or(source_uid);
    let court = field("court")
        .and_then(obwalden_court)
        .unwrap_or_else(|| panic!("stored opencaselaw court not taken ({source_uid})"));
    let date = field("date")
        .unwrap_or_else(|| panic!("source_fields opencaselaw without a date ({source_uid})"));
    let dockets = source_fields
        .get("dockets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    decision_of(
        source_uid.to_string(),
        id,
        court,
        None,
        date.to_string(),
        dockets,
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "Aus den Erwägungen:\n1. Die Beschwerdeführerin rügt, die Vorinstanz habe \
        den Sachverhalt unvollständig festgestellt und das rechtliche Gehör verletzt. Nach der \
        Rechtsprechung des Bundesgerichts ist der Anspruch auf rechtliches Gehör formeller Natur, \
        weshalb seine Verletzung grundsätzlich zur Aufhebung des angefochtenen Entscheids führt. \
        Die Beschwerde ist daher gutzuheissen und die Sache zur neuen Beurteilung zurückzuweisen.";

    fn row(id: &str, docket: &str, head: &str) -> Map<String, Value> {
        let row = json!({
            "decision_id": id,
            "docket_number": docket,
            "decision_date": "2015-11-26",
            "full_text": format!("{docket}\nArt. 29 Abs. 2 BV\nVerletzung des rechtlichen Gehörs.\n{head}\n{BODY}"),
        });
        row.as_object().unwrap().clone()
    }

    #[test]
    fn an_entry_is_placed_with_the_body_and_the_day_its_line_names() {
        let parsed = parse_opencaselaw(&row(
            "ow_gerichte_OGVE 2014_15 Nr. 21",
            "OGVE-2014-15-21.htm#",
            "Entscheid des Verwaltungsgerichts vom 26./27. Mai 2015 (B 14/005).",
        ))
        .unwrap();
        assert_eq!(parsed.court.id, "OW_VG");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2015-05-27"));
        assert_eq!(
            parsed.decision.numero_dossiers,
            Some(vec![
                "OGVE 2014/15 Nr. 21".to_string(),
                "B 14/005".to_string()
            ])
        );
        let rebuilt = from_source_fields_opencaselaw(
            &parsed.decision.texte_integral_clean,
            &parsed.source_fields,
            &parsed.decision.source_uid,
        );
        assert_eq!(
            rebuilt.jurisdiction_source_code,
            parsed.decision.jurisdiction_source_code
        );
        assert_eq!(rebuilt.numero_dossiers, parsed.decision.numero_dossiers);
    }

    #[test]
    fn the_bodies_are_typed_by_the_words_their_line_opens_on() {
        let court = |head: &str, docket: &str| {
            parse_opencaselaw(&row("x", docket, head))
                .map(|p| p.court.id)
                .unwrap_or("refused")
        };
        let ab = "AbR 1990/91 Nr. 28";
        let vv = "VVGE 1966/70 Nr. 45";
        assert_eq!(
            court(
                "Entscheid der Obergerichtskommission vorn 3. Oktober 1990",
                ab
            ),
            "OW_OG"
        );
        assert_eq!(court("Urteile der Rekurskommission für Sozialversicherung vom 8. Oktober und 16. Dezember 1987", ab), "OW_VSG");
        assert_eq!(
            court(
                "Entscheid des Regierungsrates vom 15. Mai 1984 (Nr. 48).",
                vv
            ),
            "OW_RR"
        );
        assert_eq!(
            court(
                "Entscheid des Gewerbe- und Fürsorgedepartementes vom 3. Mai 1999.",
                vv
            ),
            "OW_DEP"
        );
        assert_eq!(
            court("Verfügung des Amts für Justiz vom 5. April 2016.", vv),
            "OW_DEP"
        );
        assert_eq!(
            court(
                "Entscheid vom 25.11.1969 i.S. Rekurs O. H. gegen Gemeinderat Engelberg.",
                vv
            ),
            "OW_RR"
        );
        assert_eq!(
            court("Entscheid des Kantonsrats vom 1. Juli 2004.", vv),
            "refused"
        );
        assert_eq!(
            court("Stellungnahme des Rechtsdienstes vom 27. Juli 1995.", vv),
            "refused"
        );
        assert_eq!(
            court(
                "Entscheid des Regierungsrates vom 6. Mai 1986.",
                "VVGE 1999/00 Anhang I"
            ),
            "refused"
        );
        assert_eq!(
            court(
                "Entscheid des Regierungsrates vom 2. März 1887 (Nr. 1318).",
                "VVGE 1987/88 Nr. 30"
            ),
            "refused"
        );
    }

    #[test]
    fn the_day_is_the_last_before_another_court() {
        let day = |line: &str| act_date(line).map(|d| d.to_string());
        assert_eq!(day("21 Dezember 1993").as_deref(), Some("1993-12-21"));
        assert_eq!(
            day("27.10.1970 i.S. Rekurs F. A.").as_deref(),
            Some("1970-10-27")
        );
        assert_eq!(day("1.4.69.").as_deref(), Some("1969-04-01"));
        assert_eq!(
            day("23. Juli 1988 [recte: 23. Juni 1988]").as_deref(),
            Some("1988-06-23")
        );
        assert_eq!(
            day("15. April 1985 und des Bundesgerichts vom 16. März 1986.").as_deref(),
            Some("1985-04-15")
        );
    }

    #[test]
    fn a_long_line_is_dated_by_its_first_ruling() {
        let parsed = parse_opencaselaw(&row(
            "ow_gerichte_OGVE 2018_19 Nr. 30",
            "OGVE 2018/19 Nr. 30",
            "Entscheid des Verwaltungsgerichts vom 31. Oktober 2019 (VK 19/003); vom Bundesgericht \
             im Verfahren 9C_804/2019 vom 4. Mai 2020 betreffend vorliegende Fragestellungen bestätigt.",
        ))
        .unwrap();
        assert_eq!(parsed.court.id, "OW_VG");
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2019-10-31"));
    }
}
