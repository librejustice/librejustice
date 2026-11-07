//! Judicial DILA funds (CASS, INCA, CAPP), schema `TEXTE_JURI_JUDI`: the
//! chamber as Judilibre codes it, every number the record writes, files
//! holding several judgments cut in sections, and what the reading of the
//! historical stock corrected.

use super::*;
use lj_core::decision::AttackedRef;
use lj_core::docket_key::docket_key;
use std::collections::HashMap;

// ── Corrections read on the stock ───────────────────────────────────────────

/// What the reading of the stock changed in a record, or a section of it:
/// its numbers, its date, its court.
#[derive(Debug)]
struct Correction {
    section: Option<u32>,
    numbers: Option<Vec<&'static str>>,
    date: Option<&'static str>,
    court: Option<&'static str>,
}

static CORRECTIONS: LazyLock<HashMap<&'static str, Vec<Correction>>> = LazyLock::new(|| {
    let mut out: HashMap<&'static str, Vec<Correction>> = HashMap::new();
    for line in include_str!("../data/dila_juri_corrections.tsv").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let cols: Vec<&'static str> = line.split('\t').collect();
        let [juritext, section, numbers, date, court] = cols[..] else {
            panic!("dila_juri_corrections.tsv: five columns expected: {line}");
        };
        let some = |v: &'static str| (!v.is_empty()).then_some(v);
        out.entry(juritext).or_default().push(Correction {
            section: some(section).map(|k| k.parse().expect("section number")),
            numbers: some(numbers).map(|n| n.split('|').collect()),
            date: some(date),
            court: some(court),
        });
    }
    out
});

fn correction(juritext: &str, section: Option<u32>) -> Option<&'static Correction> {
    CORRECTIONS
        .get(juritext)?
        .iter()
        .find(|r| r.section == section)
}

fn corrected_sections(juritext: &str) -> Vec<u32> {
    CORRECTIONS
        .get(juritext)
        .map(|rows| rows.iter().filter_map(|r| r.section).collect())
        .unwrap_or_default()
}

/// The `source_uid`s a judicial file is written under: one per section when
/// the reading cut it, the file's own otherwise.
pub fn judicial_record_uids(prefix: &str, juritext: &str) -> Vec<String> {
    let sections = corrected_sections(juritext);
    if sections.is_empty() {
        return vec![format!("{prefix}/{juritext}")];
    }
    sections
        .into_iter()
        .map(|k| format!("{prefix}/{juritext}/{k}"))
        .collect()
}

pub(crate) fn juritext_and_section(source_uid: &str) -> (&str, Option<u32>) {
    let mut parts = source_uid.split('/').skip(1);
    let juritext = parts.next().unwrap_or_default();
    (juritext, parts.next().and_then(|k| k.parse().ok()))
}

// ── Vocabulary ──────────────────────────────────────────────────────────────

/// `FORMATION` of the Cour de cassation funds as the chamber code Judilibre writes.
fn chamber_code(formation: &str) -> Option<&'static str> {
    Some(match formation {
        "CHAMBRE_CIVILE_1" => "civ1",
        "CHAMBRE_CIVILE_2" => "civ2",
        "CHAMBRE_CIVILE_3" => "civ3",
        "CHAMBRE_SOCIALE" => "soc",
        "CHAMBRE_COMMERCIALE" => "comm",
        "CHAMBRE_CRIMINELLE" => "cr",
        "CHAMBRE_MIXTE" => "mi",
        "ASSEMBLEE_PLENIERE" => "pl",
        "ORDONNANCE_PREMIER_PRESIDENT" => "ordo",
        "CHAMBRES_REUNIES" => "creun",
        _ => return None,
    })
}

/// A formation no chamber code names.
fn formation_label(formation: &str) -> Option<&'static str> {
    Some(match formation {
        "AVIS" => "Formation pour avis",
        "CHAMBRE_CIVILE" => "Chambre civile",
        "COMMISSION_REPARATION_DETENTION" => "Commission nationale de réparation des détentions",
        "COMMISSION_REVISION" => "Commission de révision",
        "COUR_REVISION" => "Cour de révision",
        "COMMISSION_REEXAMEN" => "Commission de réexamen",
        "JURIDICTION_NATIONALE_LIBERTE_CONDITIONNELLE" => {
            "Juridiction nationale de la libération conditionnelle"
        }
        _ => return None,
    })
}

fn nature_label(nature: &str) -> String {
    match nature {
        "ARRET" => "Arrêt",
        "ORDONNANCE" => "Ordonnance",
        "AVIS" => "Avis",
        "AUTRES_DECISIONS" => "Autre",
        other => other,
    }
    .to_string()
}

static CAPP_CA_LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*cour d'appel (?:de |d'|du |des )?(.+)$").unwrap());

/// Referential code of the court of a CAPP record: the one its reading
/// established, or the appeal court its label names.
fn capp_court(juritext: &str, label: Option<&str>) -> Option<String> {
    if let Some(court) = correction(juritext, None).and_then(|c| c.court) {
        return Some(court.to_string());
    }
    let city = CAPP_CA_LABEL_RE.captures(label?)?;
    Some(format!("ca_{}", lj_fold::slug(&city[1], '_')))
}

/// Type of a referential court code: a closed court takes its successor's,
/// the conseils de prud'hommes alone have their own.
fn court_code_type(code: &str) -> Option<&'static str> {
    Some(match code.split('_').next()? {
        "ca" | "tsa" => "CA",
        "cph" => "CPH",
        "tcom" => "TCOM",
        "tc" => "TC",
        "tj" | "tgi" | "ti" | "prox" | "tass" | "tpi" | "tpbr" => "TJ",
        _ => return None,
    })
}

/// Type of the court a CAPP record names in `JURIDICTION`.
fn capp_jurisdiction_type(label: &str) -> Option<&'static str> {
    let l = lj_fold::fold(label);
    const TYPES: &[(&str, &str)] = &[
        ("cour d'appel", "CA"),
        ("tribunal superieur d'appel", "CA"),
        ("chambre de l'application des peines", "CA"),
        ("conseil de prud", "CPH"),
        ("tribunal de commerce", "TCOM"),
        ("tribunal des conflits", "TC"),
        ("tribunal de grande instance", "TJ"),
        ("tribunal judiciaire", "TJ"),
        ("tribunal d'instance", "TJ"),
        ("juridiction de proximite", "TJ"),
        ("juge de proximite", "TJ"),
        ("tribunal des affaires de securite sociale", "TJ"),
        ("tribunal de premiere instance", "TJ"),
        ("tribunal correctionnel", "TJ"),
        ("tribunal paritaire", "TJ"),
        ("trib.", "TJ"),
    ];
    TYPES
        .iter()
        .find(|(prefix, _)| l.starts_with(prefix))
        .map(|(_, t)| *t)
}

static ATTACKED_DATE_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",\s*\d{4}-\d{2}-\d{2}\s*$").unwrap());

static AVOCATS_HEAD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(av\.\s*demandeur\s*:|demandeur(?:\s+av\b\.?)?)|(?:avocats?|avou[ée]s?)\s*:)\s*",
    )
    .unwrap()
});
static AVOCATS_DEFENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i),?\s*d[ée]fenseur\b").unwrap());
static AVOCATS_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|,\s*|\s+et\s+)((?:la\s+|les\s+)?(?:scp|selarl|selas|selafa|sarl|aarpi|soci[ée]t[ée]\s+civile\s+professionnelle|cabinet)\b|(?:mes?|mm?\.?|mmes?|mlle|ma[iî]tre)(?:\s|$))|\s((?:la|les)\s+scp\b)",
    )
    .unwrap()
});
static AVOCATS_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:mes?|mm?\.?|mmes?|mlle|ma[iî]tre)(?:\s+|$)").unwrap());
static AVOCATS_FIRM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:la\s+|les\s+)?(?:soci[ée]t[ée]\s+civile\s+professionnelle\b)?").unwrap()
});
static AVOCATS_NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*\([^)]*\)|,?\s*avocats?\s+au\s+barreau\b[^,]*|\s+repr[ée]sentant\b[^,]*")
        .unwrap()
});
static AVOCATS_NAMES_SEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*,\s*|\s+et\s+").unwrap());

/// The counsel of `AVOCATS`: `(applicant, listed)`. The first named is the
/// applicant's when the field opens on « Demandeur », every other one is listed
/// with no side. An item opens on a firm form or a title; a firm keeps the
/// commas between its partners, a title introduces one surname or several.
fn avocats(raw: &str) -> (Option<String>, Vec<String>) {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = AVOCATS_NOISE_RE.replace_all(&flat, "");
    let (applicant, body) = match AVOCATS_HEAD_RE.captures(&flat) {
        Some(c) => (c.get(1).is_some(), &flat[c.get(0).map_or(0, |m| m.end())..]),
        None => (false, &flat[..]),
    };
    let (body, defence) = match AVOCATS_DEFENCE_RE.find(body) {
        Some(m) => (&body[..m.start()], Some(&body[m.end()..])),
        None => (body, None),
    };
    let mut values = Vec::new();
    for item in avocats_items(body) {
        match AVOCATS_TITLE_RE.find(item) {
            Some(title) => values.extend(avocats_names(&item[title.end()..])),
            None => {
                let form = AVOCATS_FIRM_RE.find(item).map_or(0, |m| m.end());
                let firm = item[form..].trim_matches([' ', ',', '.', ';', ':']);
                values.push(if item[..form].to_lowercase().contains("civile") {
                    format!("SCP {firm}")
                } else {
                    firm.to_string()
                });
            }
        }
    }
    values.extend(defence.map(avocats_names).unwrap_or_default());
    values.retain(|v| v.chars().count() > 2 && !v.chars().any(|c| c.is_ascii_digit()));
    if applicant && !values.is_empty() {
        let first = values.remove(0);
        (Some(first), values)
    } else {
        (None, values)
    }
}

/// The items of an `AVOCATS` list, each opening on a firm form or a title.
fn avocats_items(body: &str) -> Vec<&str> {
    let mut cuts: Vec<(usize, usize)> = AVOCATS_ITEM_RE
        .captures_iter(body)
        .filter_map(|c| {
            let item = c.get(1).or(c.get(2))?;
            Some((c.get(0)?.start(), item.start()))
        })
        .collect();
    if cuts.first().is_none_or(|&(_, at)| at > 0) {
        cuts.insert(0, (0, 0));
    }
    (0..cuts.len())
        .map(|i| &body[cuts[i].1..cuts.get(i + 1).map_or(body.len(), |c| c.0)])
        .collect()
}

/// The surnames after a title, up to the first sentence break, a title repeated
/// before each one dropped.
fn avocats_names(names: &str) -> Vec<String> {
    let names = names.split(". ").next().unwrap_or_default();
    AVOCATS_NAMES_SEP_RE
        .split(names)
        .map(|n| {
            let n = &n[AVOCATS_TITLE_RE.find(n).map_or(0, |m| m.end())..];
            n.trim_matches([' ', ',', '.', ';', ':']).to_string()
        })
        .collect()
}

// ── Text ────────────────────────────────────────────────────────────────────

static BREAK_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\b[^>]*>|</?p\b[^>]*>").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

/// Text of `BLOC_TEXTUEL/CONTENU`, which these funds write as real `<p>` and
/// `<br clear="none"/>` elements: each becomes a line break before the
/// entities are decoded. `None` when the element is absent or empty.
pub(crate) fn contenu_text(raw: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(raw);
    let bloc = s.find("<BLOC_TEXTUEL")?;
    let open = bloc + s[bloc..].find("<CONTENU")?;
    let gt = open + s[open..].find('>')?;
    if s[..gt].ends_with('/') {
        return None;
    }
    let close = gt + s[gt..].find("</CONTENU>")?;
    let text = BREAK_TAG_RE.replace_all(&s[gt + 1..close], "\n");
    let text = TAG_RE.replace_all(&text, "");
    let text = quick_xml::escape::unescape(&text)
        .map(|t| t.into_owned())
        .unwrap_or_else(|_| text.into_owned());
    let text = clean_texte(&BREAK_TAG_RE.replace_all(&text, "\n"));
    (!text.trim().is_empty()).then_some(text)
}

/// Whitespace runs collapsed to one space, with the byte offset each collapsed
/// byte comes from, so a position read on the collapsed text cuts the original.
fn collapse_ws(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut origin = Vec::with_capacity(s.len() + 1);
    let mut in_ws = false;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if in_ws {
                continue;
            }
            in_ws = true;
            out.push(' ');
            origin.push(i);
        } else {
            in_ws = false;
            out.push(c);
            origin.extend(std::iter::repeat_n(i, c.len_utf8()));
        }
    }
    origin.push(s.len());
    (out, origin)
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Byte offset `n` characters before `at`, or 0.
fn back_chars(s: &str, at: usize, n: usize) -> usize {
    s[..at]
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(0, |(i, _)| i)
}

/// At most `n` characters from `at`.
fn ahead(s: &str, at: usize, n: usize) -> &str {
    let rest = &s[at..];
    let end = rest.char_indices().nth(n).map_or(rest.len(), |(i, _)| i);
    &rest[..end]
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ── Numbers ─────────────────────────────────────────────────────────────────

fn pourvoi_display(key7: &str) -> String {
    format!("{}-{}.{}", &key7[..2], &key7[2..4], &key7[4..])
}

fn ascii_key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `a À b` between two pourvois of the same year and series, up to sixty.
fn expand_range(from: &str, to: &str) -> Option<Vec<String>> {
    if from[..4] != to[..4] {
        return None;
    }
    let (a, b): (u32, u32) = (from[4..].parse().ok()?, to[4..].parse().ok()?);
    (a < b && b - a <= 60).then(|| {
        (a..=b)
            .map(|x| pourvoi_display(&format!("{}{x:03}", &from[..4])))
            .collect()
    })
}

static STRUCTURED_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(\d{2})\s*-\s*(\d{5})\s*(?:à|a|au)\s*(\d{2})\s*-\s*(\d{5})\s*$").unwrap()
});
static STRUCTURED_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)et|[,;]|n\s*°.*n\s*°|bis\s*$").unwrap());

/// A structured `NUMERO_AFFAIRE` of the Cour de cassation funds, repaired:
/// pairs glued on fourteen digits, `a ET b`, `N° a N° b`, compressed suffixes
/// (`6670212213214215`) and `a À b` split, `BIS` and the chamber letter of
/// INCA (`V 88-17.459`) dropped, a pourvoi written with spaces rejoined. Under
/// five alphanumerics it is not a pourvoi (`1`, `N`). Any other value stays as
/// written, the old lettered forms (`66-MI.121`) included.
fn repair_structured(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let key = ascii_key(raw);
    if key.len() < 5 {
        return Vec::new();
    }
    if let Some(c) = STRUCTURED_RANGE_RE.captures(raw) {
        let from = format!("{}{}", &c[1], &c[2]);
        let to = format!("{}{}", &c[3], &c[4]);
        return expand_range(&from, &to)
            .unwrap_or_else(|| vec![pourvoi_display(&from), pourvoi_display(&to)]);
    }
    if key.len() == 8 && key.as_bytes()[0].is_ascii_lowercase() && all_digits(&key[1..]) {
        return vec![pourvoi_display(&key[1..])];
    }
    let glued = all_digits(&key)
        && (key.len() == 14 || (key.len() > 7 && (key.len() - 7).is_multiple_of(3)));
    let split = glued || STRUCTURED_SPLIT_RE.is_match(raw);
    if !split && !raw.contains(char::is_whitespace) {
        return vec![raw.to_string()];
    }
    let parts: Vec<&str> = if split {
        key.split("et").flat_map(|p| p.split('n')).collect()
    } else {
        vec![key.as_str()]
    };
    let mut out = Vec::new();
    for part in parts {
        let part = part.strip_suffix("bis").unwrap_or(part);
        if !all_digits(part) {
            if !split {
                out.push(raw.split_whitespace().collect::<String>());
            }
            continue;
        }
        match part.len() {
            7 => out.push(pourvoi_display(part)),
            14 => {
                out.push(pourvoi_display(&part[..7]));
                out.push(pourvoi_display(&part[7..]));
            }
            n if n > 7 && (n - 7).is_multiple_of(3) => {
                out.push(pourvoi_display(&part[..7]));
                for i in (7..n).step_by(3) {
                    out.push(pourvoi_display(&format!(
                        "{}{}",
                        &part[..4],
                        &part[i..i + 3]
                    )));
                }
            }
            _ if !split => out.push(raw.split_whitespace().collect::<String>()),
            _ => {}
        }
    }
    out
}

/// A pourvoi read at byte `i`, `NN-NN.NNN` with every spacing the 1960s
/// trailers use, `I` and `O` read as `1` and `0` past the first two digits:
/// its seven-digit key and its end.
fn pourvoi_at(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut j = i;
    let mut key = String::new();
    let skip_ws = |j: &mut usize| {
        while *j < b.len() && b[*j].is_ascii_whitespace() {
            *j += 1;
        }
    };
    let take = |j: &mut usize, key: &mut String, n: usize, letters: bool| -> Option<()> {
        for _ in 0..n {
            let c = *b.get(*j)?;
            key.push(match c {
                b'0'..=b'9' => c as char,
                b'I' | b'i' if letters => '1',
                b'O' | b'o' if letters => '0',
                _ => return None,
            });
            *j += 1;
        }
        Some(())
    };
    take(&mut j, &mut key, 2, false)?;
    skip_ws(&mut j);
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    skip_ws(&mut j);
    take(&mut j, &mut key, 2, true)?;
    skip_ws(&mut j);
    if b.get(j) == Some(&b'.') {
        j += 1;
    }
    skip_ws(&mut j);
    take(&mut j, &mut key, 3, true)?;
    if matches!(b.get(j), Some(b'0'..=b'9' | b'I' | b'i' | b'O' | b'o')) {
        return None;
    }
    Some((key, j))
}

static LIST_HEAD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bN\s*(?:[°O]\s*S?\s*|\s+)(?:DE\s+POURVOIS?\s*:?\s*)?").unwrap()
});
static LIST_SEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(,|ET\b|A\b|À|AU\b)\s*(N\s*[°O]\s*S?\s*)?").unwrap());

/// Pourvois of every `N° a, b ET c` list in `seg`, `a À b` expanded.
fn listed_pourvois(seg: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |k: String| {
        if !out.contains(&k) {
            out.push(k);
        }
    };
    for head in LIST_HEAD_RE.find_iter(seg) {
        let Some((first, mut end)) = pourvoi_at(seg, head.end()) else {
            continue;
        };
        let mut prev = first.clone();
        push(first);
        while let Some(sep) = LIST_SEP_RE.captures(&seg[end..]) {
            let word = sep.get(1).unwrap();
            let glued_word = word.start() == 0
                && word.as_str().chars().all(|c| c.is_ascii_alphabetic())
                && seg[..end].chars().next_back().is_some_and(is_word);
            if glued_word {
                break;
            }
            let at = end + sep.get(0).unwrap().end();
            let Some((key, next_end)) = pourvoi_at(seg, at) else {
                break;
            };
            let range = word.as_str() != ","
                && !word.as_str().eq_ignore_ascii_case("et")
                && sep.get(2).is_none();
            if range {
                if let Some(run) = expand_range(&prev, &key) {
                    for k in run {
                        push(ascii_key(&k));
                    }
                }
            }
            prev = key.clone();
            push(key);
            end = next_end;
        }
    }
    out
}

static PRESIDENT_COLON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)PRESIDENT\s*:").unwrap());
static PRESIDENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)PRESIDENT").unwrap());
static PAR_CES_MOTIFS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)PAR CES MOTIFS").unwrap());
static TRAILER_STOP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)M[EÊ]MES?\s+ESP[EÈ]CES?|A RAPPROCHER|DANS LE M[EÊ]ME SENS|EN SENS CONTRAIRE|SUR LE N\s*°\s*\d\b").unwrap()
});
static JOINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)JOINT[^;]{0,40}?(?:LES\s+|LE\s+)?POURVOIS?[^;]{0,300}").unwrap()
});

/// Pourvois of a Cour de cassation judgment's trailer, the window from the last
/// « PAR CES MOTIFS » to the last « PRESIDENT : », stopped where the related
/// judgments start; failing that, the pourvois its « JOINT » clause names.
/// On the 1960s judgments that also carry a structured number, it disagrees only
/// where the record is defective.
fn trailer_pourvois(text: &str) -> Vec<String> {
    let (t, _) = collapse_ws(text);
    let end = match PRESIDENT_COLON_RE.find_iter(&t).last() {
        Some(m) => m.start(),
        None => PRESIDENT_RE
            .find_iter(&t)
            .last()
            .map_or(t.len(), |m| m.start()),
    };
    let end = if end == 0 { t.len() } else { end };
    let start = match PAR_CES_MOTIFS_RE
        .find_iter(&t)
        .filter(|m| m.end() <= end)
        .last()
    {
        Some(p) => p.start().max(back_chars(&t, end, 1500)),
        None => back_chars(&t, end, 600),
    };
    let mut seg = &t[start..end];
    if let Some(stop) = TRAILER_STOP_RE.find(seg) {
        seg = &seg[..stop.start()];
    }
    let mut out = listed_pourvois(seg);
    if out.is_empty() {
        for m in JOINT_RE.find_iter(&t) {
            for k in listed_pourvois(m.as_str()) {
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
    }
    out
}

static INCA_HEADER_END_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)sur le rapport|apr[eè]s en avoir d[eé]lib[eé]r[eé]").unwrap()
});
static INCA_SUR_POURVOI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sur (?:le|les) pourvois?\s+n\s*°\s*s?\s*(.{0,160}?)(?:form[eé]|,\s*\d|\bpar\b|$)",
    )
    .unwrap()
});
static INCA_JOINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)joint\s+(?:au\s+)?(?:les\s+)?pourvois?\s+(?:n\s*°\s*s?\s*)?(.{0,200}?)(?:;|\bet\s+la\s+requ|\bformés?\b|$)").unwrap()
});
static INCA_CITED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(rabat|par arr[eê]t (?:de ce jour|du|en date)|faisant l'objet du|ayant [eé]t[eé]|sous le n|arr[eê]t (?:rendu|prononc[eé]) le|rapporte)").unwrap()
});
static INCA_NUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Z]\s?[/-]?\s?)?(\d{2}\s?-?\s?\d{2}\s?[.\s]?\s?\d{3})").unwrap()
});
static TRAILING_DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d").unwrap());

/// Pourvois written in `s`, neither glued to a word or a dot before nor
/// followed by another digit: their seven-digit keys.
fn inca_numbers(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let before_ok = !s[..i]
            .chars()
            .next_back()
            .is_some_and(|c| is_word(c) || c == '.');
        let hit = before_ok
            .then(|| INCA_NUM_RE.captures(&s[i..]))
            .flatten()
            .filter(|c| !TRAILING_DIGIT_RE.is_match(&s[i + c.get(0).unwrap().end()..]));
        match hit {
            Some(c) => {
                let key = ascii_key(&c[1]);
                out.push(key[key.len() - 7..].to_string());
                i += c.get(0).unwrap().end();
            }
            None => i += s[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// Pourvois an INCA judgment joins: the ones its heading names (« Sur le(s)
/// pourvoi(s) n° »), unless the heading cites them as an earlier judgment's,
/// and the ones its « joint les pourvois » clause names. Never a number its
/// reasons cite.
fn inca_joined_pourvois(text: &str) -> Vec<String> {
    let (t, _) = collapse_ws(text);
    let header_end = INCA_HEADER_END_RE
        .find(&t)
        .map_or_else(|| ahead(&t, 0, 1500).len(), |m| m.start());
    let header = &t[..header_end];
    let mut out: Vec<String> = Vec::new();
    for c in INCA_SUR_POURVOI_RE.captures_iter(header) {
        let at = c.get(0).unwrap().start();
        if INCA_CITED_RE.is_match(&header[back_chars(header, at, 120)..at]) {
            continue;
        }
        out.extend(inca_numbers(&c[1]));
    }
    for c in INCA_JOINT_RE.captures_iter(&t) {
        out.extend(inca_numbers(&c[1]));
    }
    let mut seen = Vec::new();
    out.retain(|k| {
        let fresh = !seen.contains(k);
        seen.push(k.clone());
        fresh
    });
    out
}

static ANCIEN_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]{4}[A-Z0-9]X(\d{4})X\d{2}X(\d{2})X(\d{5})X").unwrap());
static PUBLI_BULL_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bN\s*[°.]?\s*(\d+)\b").unwrap());

/// The Bulletin reference of a Cour de cassation judgment, decoded from
/// `ANCIEN_ID` (`CXCXAX1965X11X01X00611X000` is Bull. 1965, I, n° 611), or
/// read from `PUBLI_BULL` (`N° 611`) with the part its chamber publishes in.
fn bulletin_reference(
    ancien_id: Option<&str>,
    publi_bull: Option<&str>,
    chamber: Option<&str>,
    date: &str,
) -> Option<String> {
    const PARTS: &[&str] = &["I", "II", "III", "IV", "V"];
    let chamber_part = match chamber {
        Some("civ1") => Some(0),
        Some("civ2") => Some(1),
        Some("civ3") => Some(2),
        Some("comm") => Some(3),
        Some("soc") => Some(4),
        _ => None,
    };
    let (year, part, number) = match ancien_id.and_then(|a| ANCIEN_ID_RE.captures(a)) {
        Some(c) => {
            let part: usize = c[2].parse().ok()?;
            let number: u32 = c[3].parse().ok()?;
            (
                c[1].to_string(),
                chamber_part.and(part.checked_sub(1)),
                number,
            )
        }
        None => {
            let c = PUBLI_BULL_NUMBER_RE.captures(publi_bull?)?;
            (date.get(..4)?.to_string(), chamber_part, c[1].parse().ok()?)
        }
    };
    if number == 0 {
        return None;
    }
    Some(match (chamber, part.and_then(|p| PARTS.get(p))) {
        (Some("cr"), _) => format!("Bull. crim. {year}, n° {number}"),
        (_, Some(part)) => format!("Bull. {year}, {part}, n° {number}"),
        _ => format!("Bull. {year}, n° {number}"),
    })
}

/// Trailers stopped being where a judgment writes its number once the record
/// carried it structured: the reader runs on the decade it was measured on.
const TRAILER_READ_BEFORE: &str = "1970-01-01";

struct NumberInputs<'a> {
    fond: DilaFond,
    juritext: &'a str,
    section: Option<u32>,
    structured: &'a [String],
    text: &'a str,
    date: &'a str,
    ancien_id: Option<&'a str>,
    publi_bull: Option<&'a str>,
    chamber: Option<&'a str>,
}

/// Every number of a record: the structured ones repaired, those its text
/// writes, and failing all, the Bulletin reference. A disagreement keeps both.
/// A section, or a record whose reading named its numbers, takes those.
fn judicial_numbers(i: &NumberInputs<'_>) -> Vec<String> {
    if let Some(numbers) = correction(i.juritext, i.section).and_then(|c| c.numbers.as_ref()) {
        return numbers.iter().map(|n| n.to_string()).collect();
    }
    if i.section.is_some() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |n: String| {
        let key = docket_key(&n);
        if !out.iter().any(|o| docket_key(o) == key) {
            out.push(n);
        }
    };
    match i.fond {
        DilaFond::Capp => {
            for s in i.structured {
                let n: String = s.split_whitespace().collect();
                if !n.is_empty() {
                    push(n);
                }
            }
        }
        DilaFond::Cass => {
            i.structured
                .iter()
                .flat_map(|s| repair_structured(s))
                .for_each(&mut push);
            if i.date < TRAILER_READ_BEFORE {
                trailer_pourvois(i.text)
                    .iter()
                    .for_each(|k| push(pourvoi_display(k)));
            }
        }
        DilaFond::Inca => {
            i.structured
                .iter()
                .flat_map(|s| repair_structured(s))
                .for_each(&mut push);
            inca_joined_pourvois(i.text)
                .iter()
                .for_each(|k| push(pourvoi_display(k)));
        }
        DilaFond::Jade | DilaFond::Constit | DilaFond::Cnil => {
            unreachable!("judicial_numbers on an administrative fund")
        }
    }
    if out.is_empty() && i.fond == DilaFond::Cass {
        out.extend(bulletin_reference(
            i.ancien_id,
            i.publi_bull,
            i.chamber,
            i.date,
        ));
    }
    out
}

// ── Files holding several judgments ─────────────────────────────────────────

static LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(?\bARRETS?\s+N\s*[°O]\s*\d\s*\)?").unwrap());
static TRAILER_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    let num = r"(?:\d{2}\s*[-/.]\s*\d{2}\s*[.\s]?\s*\d{3}\b|\d{2}\s*-\s*\d{5}\b)";
    let no = r"N\s*[°O]S?\s*";
    Regex::new(&format!(r"(?i){no}{num}(?:\s*(?:,|ET)\s*(?:{no})?{num})*")).unwrap()
});
static PARTIES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bC\s*/|\bC\s+[A-Z]").unwrap());
static PRESIDENT_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:PREMIER )?PRESIDENT\s*:").unwrap());
static MARK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bARRETS?\s+N\s*[°O]\s*(\d)\b").unwrap());
static MARK_IS_ENTRY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\)?\s*[:.]?\s*N\s*[°O]S?\s*\d").unwrap());
static DISPOSITIF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)PAR CES MOTIFS|\bREJETTE\b|CASSE ET ANNULE|IRRECEVABLE").unwrap()
});

/// Where a collection's shared trailer starts: its first entry naming parties
/// (`N° … X C/ Y`) or label in the second half of the text, or the first
/// « PRESIDENT : ».
fn collection_trailer_start(t: &str) -> usize {
    let n = char_count(t);
    let mut start = t.len();
    for m in TRAILER_ENTRY_RE.find_iter(t) {
        if 2 * char_count(&t[..m.start()]) < n {
            continue;
        }
        let window = back_chars(t, m.start(), 160);
        let label = LABEL_RE.find_iter(&t[window..m.start()]).last();
        if label.is_some() || PARTIES_RE.is_match(ahead(t, m.end(), 220)) {
            start = start.min(label.map_or(m.start(), |l| window + l.start()));
        }
    }
    if let Some(p) = PRESIDENT_LINE_RE.find(t) {
        start = start.min(p.start());
    }
    start
}

/// Sections `(k, start, end)` of a collection, cut at its « ARRET N° k »
/// marks, its shared trailer left out. A first mark past the first judgment's
/// dispositif opens the section before it.
fn collection_sections(t: &str) -> Vec<(u32, usize, usize)> {
    let ts = collection_trailer_start(t);
    let mut marks: Vec<(usize, u32)> = Vec::new();
    for c in MARK_RE.captures_iter(&t[..ts]) {
        let m = c.get(0).unwrap();
        if t[back_chars(t, m.start(), 2)..m.start()].contains('(')
            || MARK_IS_ENTRY_RE.is_match(ahead(t, m.end(), 20))
        {
            continue;
        }
        marks.push((m.start(), c[1].parse().unwrap()));
    }
    if let Some(&(pos, k)) = marks.first() {
        if k >= 2 && char_count(&t[..pos]) > 300 && DISPOSITIF_RE.is_match(&t[..pos]) {
            marks.insert(0, (0, k - 1));
        }
    }
    marks
        .iter()
        .enumerate()
        .map(|(i, &(pos, k))| (k, pos, marks.get(i + 1).map_or(ts, |m| m.0)))
        .collect()
}

/// The units of a record: the whole text, or the sections its reading cut it
/// in. A cut that does not give back the sections the reading named is an error.
fn record_units(juritext: &str, text: &str) -> lj_core::error::Result<Vec<(Option<u32>, String)>> {
    let named = corrected_sections(juritext);
    if named.is_empty() {
        return Ok(vec![(None, text.to_string())]);
    }
    let (collapsed, origin) = collapse_ws(text);
    let sections = collection_sections(&collapsed);
    let mut found: Vec<u32> = sections.iter().map(|s| s.0).collect();
    let mut expected = named.clone();
    found.sort_unstable();
    expected.sort_unstable();
    if found != expected {
        return Err(lj_core::error::CoreError::Xml(format!(
            "DILA {juritext}: sections {found:?} where the reading named {expected:?}"
        )));
    }
    Ok(sections
        .into_iter()
        .map(|(k, start, end)| {
            let body = text[origin[start]..origin[end]].trim().to_string();
            (Some(k), body)
        })
        .collect())
}

// ── Decision ────────────────────────────────────────────────────────────────

fn field<'a>(sf: &'a Value, block: &str, key: &str) -> Option<&'a str> {
    sf.get(block)?
        .get(key)
        .and_then(json_text)
        .filter(|s| !s.is_empty())
}

/// A judicial record's `Decision` from its `source_fields` and the text of the
/// unit `source_uid` names. The parse goes through it too, so a decision
/// rebuilt from the stored columns is the parsed one.
pub(crate) fn judicial_decision(
    full_text: &str,
    sf: &Value,
    source_uid: &str,
    fond: DilaFond,
) -> Decision {
    let (juritext, section) = juritext_and_section(source_uid);
    let judi = "META_JURI_JUDI";
    let file_date = field(sf, "META_JURI", "DATE_DEC").unwrap_or_default();
    let date = correction(juritext, section)
        .and_then(|r| r.date)
        .unwrap_or(file_date);
    let jurisdiction_name = field(sf, "META_JURI", "JURIDICTION").map(str::to_string);
    let formation = field(sf, judi, "FORMATION");
    let mut location = None;
    let (jurisdiction_type, chamber, chamber_display) = match fond {
        DilaFond::Capp => {
            location = capp_court(juritext, jurisdiction_name.as_deref());
            let jurisdiction_type = location.as_deref().and_then(court_code_type).or_else(|| {
                jurisdiction_name
                    .as_deref()
                    .and_then(capp_jurisdiction_type)
            });
            (
                jurisdiction_type.map(str::to_string),
                formation.map(str::to_string),
                formation.map(str::to_string),
            )
        }
        _ => {
            let code = formation.and_then(chamber_code);
            let display = match code {
                Some(c) => vocab::chamber_label(Some(c)),
                None => formation.map(|f| formation_label(f).unwrap_or(f).to_string()),
            };
            (Some("CC".to_string()), code.map(str::to_string), display)
        }
    };
    let structured = json_texts(
        sf.get(judi)
            .and_then(|j| j.get("NUMEROS_AFFAIRES"))
            .and_then(|n| n.get("NUMERO_AFFAIRE")),
    );
    let publi_bull = sf.get(judi).and_then(|j| j.get("PUBLI_BULL"));
    let numbers = judicial_numbers(&NumberInputs {
        fond,
        juritext,
        section,
        structured: &structured,
        text: full_text,
        date,
        ancien_id: field(sf, "META_COMMUN", "ANCIEN_ID"),
        publi_bull: publi_bull.and_then(json_text),
        chamber: chamber.as_deref(),
    });
    let publication_codes = match fond {
        DilaFond::Capp => Vec::new(),
        _ => match publi_bull
            .and_then(|p| p.get("@publie"))
            .and_then(Value::as_str)
        {
            Some("oui") => vec!["b".to_string()],
            Some("non") => vec!["n".to_string()],
            _ => Vec::new(),
        },
    };
    let attacked_court = field(sf, judi, "FORM_DEC_ATT")
        .map(|c| ATTACKED_DATE_SUFFIX_RE.replace(c, "").trim().to_string())
        .filter(|c| !c.is_empty());
    let attacked_date = field(sf, judi, "DATE_DEC_ATT").map(str::to_string);
    let attacked = (attacked_court.is_some() || attacked_date.is_some()).then_some(AttackedRef {
        jurisdiction: attacked_court,
        number: None,
        date: attacked_date,
    });
    let type_recours = field(sf, "META_COMMUN", "NATURE").map(nature_label);
    let solution = field(sf, "META_JURI", "SOLUTION").map(str::to_string);
    let (avocat_requerant, listed_counsel) =
        field(sf, judi, "AVOCATS").map(avocats).unwrap_or_default();
    // Without a dispositif of the judicial drafting, a record may draft as the CE
    // does: the Tribunal des conflits, published with the Cour de cassation.
    let mut sections =
        unzoned_judicial_sections(full_text, matches!(fond, DilaFond::Cass | DilaFond::Inca));
    if !sections.iter().any(|s| s.kind == "dispositif") {
        let markers = extract_sections_xml(full_text);
        if !markers.is_empty() {
            sections = markers;
        }
    }
    let metadata_header = assemble_metadata_header_xml(
        jurisdiction_name.clone(),
        Some(date.to_string()),
        None,
        type_recours.clone(),
        solution.clone(),
        chamber_display,
    );
    Decision {
        source_uid: source_uid.to_string(),
        member_name: source_uid.to_string(),
        ecli: field(sf, "META_COMMUN", "ECLI")
            .or_else(|| field(sf, judi, "ECLI"))
            .map(str::to_string),
        jurisdiction_source_code: None,
        chamber,
        nac: None,
        jurisdiction_name,
        jurisdiction_type,
        jurisdiction_location: location,
        numero_dossier: numbers.first().cloned(),
        numero_dossiers: (!numbers.is_empty()).then_some(numbers),
        numero_role: None,
        date_lecture: Some(date.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours,
        solution,
        publication_codes,
        avocat_requerant,
        listed_counsel,
        texte_integral_raw: full_text.to_string(),
        texte_integral_clean: full_text.to_string(),
        sections,
        metadata_header,
        themes: json_texts(sf.get("SOMMAIRE").and_then(|s| s.get("SCT"))),
        attacked,
        parse_warnings: Vec::new(),
    }
}

/// A judicial record parsed into its units: one decision, or one per section
/// of a collection; the analysis alone when the record carries no text.
pub(crate) fn parse_judicial(
    raw: &[u8],
    member_path: &str,
    fond: DilaFond,
) -> lj_core::error::Result<Vec<DilaDoc>> {
    use lj_core::error::CoreError;
    let sf = build_source_fields_dila(raw);
    let id = field(&sf, "META_COMMUN", "ID")
        .ok_or_else(|| CoreError::Xml(format!("DILA {member_path}: META_COMMUN/ID absent")))?;
    if field(&sf, "META_JURI", "DATE_DEC").is_none() {
        return Err(CoreError::Xml(format!(
            "DILA {member_path}/{id}: META_JURI/DATE_DEC absent"
        )));
    }
    let uid = format!("{member_path}/{id}");
    if let Some(text) = contenu_text(raw) {
        return Ok(record_units(id, &text)?
            .into_iter()
            .map(|(k, body)| {
                let uid = k.map_or_else(|| uid.clone(), |k| format!("{uid}/{k}"));
                DilaDoc::Full(judicial_decision(&body, &sf, &uid, fond))
            })
            .collect());
    }
    let sommaire = sf.get("SOMMAIRE");
    let analysis = [
        json_texts(sommaire.and_then(|s| s.get("SCT"))),
        json_texts(sommaire.and_then(|s| s.get("ANA"))),
    ]
    .concat()
    .join("\n\n");
    let analysis = clean_texte(&analysis);
    if analysis.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![DilaDoc::Analysis(judicial_decision(
        &analysis, &sf, &uid, fond,
    ))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_numbers_repair_broken_encodings_only() {
        assert_eq!(repair_structured("94-18940"), vec!["94-18940"]);
        assert_eq!(repair_structured("66-MI.121"), vec!["66-MI.121"]);
        assert_eq!(repair_structured("05-CRD046"), vec!["05-CRD046"]);
        assert_eq!(repair_structured("1"), Vec::<String>::new());
        assert_eq!(repair_structured("N°"), Vec::<String>::new());
        assert_eq!(
            repair_structured("66131536613154"),
            vec!["66-13.153", "66-13.154"]
        );
        assert_eq!(
            repair_structured("6611365et6611804"),
            vec!["66-11.365", "66-11.804"]
        );
        assert_eq!(repair_structured("6240650bis"), vec!["62-40.650"]);
        assert_eq!(
            repair_structured("6670212213214215"),
            vec!["66-70.212", "66-70.213", "66-70.214", "66-70.215"]
        );
        assert_eq!(repair_structured("V 88-17.459"), vec!["88-17.459"]);
        assert_eq!(repair_structured("65-14 276"), vec!["65-14.276"]);
        assert_eq!(repair_structured("08 CRD 019"), vec!["08CRD019"]);
        assert_eq!(repair_structured("14-27266 et autres"), vec!["14-27.266"]);
        assert_eq!(
            repair_structured("03-40401 à 03-40404"),
            vec!["03-40.401", "03-40.402", "03-40.403", "03-40.404"]
        );
    }

    #[test]
    fn trailer_reads_lists_ranges_and_stops_at_related_judgments() {
        let text = "Attendu que... PAR CES MOTIFS : REJETTE LES POURVOIS. \
            N° 64-12.345 et 64-12.346 A 64-12.348. SOCIETE X C/ Y. \
            A RAPPROCHER : N° 63-11.111. PRESIDENT : M. DUPONT";
        assert_eq!(
            trailer_pourvois(text),
            vec!["6412345", "6412346", "6412347", "6412348"]
        );
        let ocr = "PAR CES MOTIFS : CASSE. N° 60-I2.3O5. DAME Z C/ W. PRESIDENT : M. X";
        assert_eq!(trailer_pourvois(ocr), vec!["6012305"]);
        let joint = "JOINT LES POURVOIS N° 61-10.001 ET 61-10.002 ; Attendu... REJETTE.";
        assert_eq!(trailer_pourvois(joint), vec!["6110001", "6110002"]);
    }

    #[test]
    fn inca_reads_joined_pourvois_never_a_cited_one() {
        let text = "Sur les pourvois n° 87-12.361 et 87-12.362 formés par la société X ; \
            Vu l'arrêt rendu le 3 mars 1986 sous le n° 85-10.000 ; \
            Sur le rapport de M. Y ; joint les pourvois n° 87-12.363 ; \
            Attendu que l'arrêt n° 84-11.111 a été cassé";
        assert_eq!(
            inca_joined_pourvois(text),
            vec!["8712361", "8712362", "8712363"]
        );
        let rabat = "Vu la requête en rabat de l'arrêt rendu sur le pourvoi n° 86-43.676 formé \
            par M. X ; Sur le rapport";
        assert!(inca_joined_pourvois(rabat).is_empty());
    }

    #[test]
    fn bulletin_reference_from_ancien_id_or_publi_bull() {
        assert_eq!(
            bulletin_reference(
                Some("CXCXAX1965X11X01X00611X000"),
                Some("N° 611"),
                Some("civ1"),
                "1965-11-15"
            )
            .as_deref(),
            Some("Bull. 1965, I, n° 611")
        );
        assert_eq!(
            bulletin_reference(None, Some("N. 348"), Some("soc"), "1966-03-29").as_deref(),
            Some("Bull. 1966, V, n° 348")
        );
        assert_eq!(
            bulletin_reference(None, Some("N° 12"), Some("cr"), "1962-01-10").as_deref(),
            Some("Bull. crim. 1962, n° 12")
        );
        assert_eq!(
            bulletin_reference(Some("CXCX8X1889X06X01X00218X000"), None, None, "1889-06-04")
                .as_deref(),
            Some("Bull. 1889, n° 218")
        );
    }

    #[test]
    fn contenu_markup_becomes_line_breaks() {
        let raw = r#"<TEXTE_JURI_JUDI><TEXTE><BLOC_TEXTUEL><CONTENU>
<p>LA COUR,</p>
<p>Décision déférée : <br clear="none"/>jugement du 21 mars 2011 &amp; suite</p>
</CONTENU></BLOC_TEXTUEL></TEXTE></TEXTE_JURI_JUDI>"#;
        assert_eq!(
            contenu_text(raw.as_bytes()).as_deref(),
            Some("LA COUR,\n\nDécision déférée :\njugement du 21 mars 2011 & suite")
        );
        assert_eq!(
            contenu_text(b"<X><BLOC_TEXTUEL><CONTENU/></BLOC_TEXTUEL></X>"),
            None
        );
    }

    #[test]
    fn collection_cut_at_its_marks_trailer_left_out() {
        let first = format!(
            "{} PAR CES MOTIFS : REJETTE. ",
            "Attendu que la caisse a refusé. ".repeat(12)
        );
        let t = format!(
            "{first}ARRET N° 2 SUR LE MOYEN UNIQUE : Attendu que. CASSE ET ANNULE. \
             ARRET N° 1 : N° 64-14.065 DIRECTEUR C/ CAISSE. ARRET N° 2 : N° 64-13.740 DAME X C/ CAISSE. \
             PRESIDENT : M. Y"
        );
        let secs = collection_sections(&t);
        assert_eq!(secs.iter().map(|s| s.0).collect::<Vec<_>>(), vec![1, 2]);
        assert!(t[secs[1].1..secs[1].2].starts_with("ARRET N° 2 SUR LE MOYEN"));
        assert!(!t[secs[1].1..secs[1].2].contains("64-13.740"));
    }

    const CASS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEXTE_JURI_JUDI>
<META>
<META_COMMUN>
<ID>JURITEXT000028760256</ID>
<ANCIEN_ID/>
<ORIGINE>JURI</ORIGINE>
<NATURE>ARRET</NATURE>
</META_COMMUN>
<META_SPEC>
<META_JURI>
<TITRE>Cour de cassation, Chambre mixte, 21 mars 2014, 12-20.002 12-20.003, Publié au bulletin</TITRE>
<DATE_DEC>2014-03-21</DATE_DEC>
<JURIDICTION>Cour de cassation</JURIDICTION>
<NUMERO>M1400278</NUMERO>
<SOLUTION>Cassation</SOLUTION>
</META_JURI>
<META_JURI_JUDI>
<NUMEROS_AFFAIRES>
<NUMERO_AFFAIRE>12-20002</NUMERO_AFFAIRE>
<NUMERO_AFFAIRE>12-20003</NUMERO_AFFAIRE>
</NUMEROS_AFFAIRES>
<PUBLI_BULL publie="oui">Bulletin 2014, Chambre mixte, n° 2</PUBLI_BULL>
<FORMATION>CHAMBRE_MIXTE</FORMATION>
<FORM_DEC_ATT>Conseil de prud'hommes de Saint-Etienne</FORM_DEC_ATT>
<DATE_DEC_ATT>2012-03-22</DATE_DEC_ATT>
<SIEGE_APPEL/>
<PRESIDENT>M. Lamanda (premier président)</PRESIDENT>
<AVOCATS>SCP Gatineau et Fattaccini</AVOCATS>
<ECLI>ECLI:FR:CCASS:2014:MI00278</ECLI>
</META_JURI_JUDI>
</META_SPEC>
</META>
<TEXTE>
<BLOC_TEXTUEL>
<CONTENU>
<p>LA COUR DE CASSATION, siégeant en CHAMBRE MIXTE, a rendu l'arrêt suivant :</p>
<p>Statuant sur les pourvois n° Q 12-20.002 et R 12-20.003 formés par la société ISS propreté,</p>
<p>Attendu que les heures de délégation sont de plein droit considérées comme temps de travail ;</p>
<p>PAR CES MOTIFS :<br clear="none"/>CASSE ET ANNULE, en toutes leurs dispositions, les jugements rendus le 22 mars 2012.</p>
</CONTENU>
</BLOC_TEXTUEL>
<SOMMAIRE>
<SCT ID="X" TYPE="PRINCIPAL">SECURITE SOCIALE, ASSURANCES SOCIALES - Maladie - Indemnité journalière</SCT>
<ANA ID="Y">Il résulte des articles L. 321-1 et L. 323-6 que...</ANA>
</SOMMAIRE>
</TEXTE>
<LIENS>
<LIEN cidtexte="LEGITEXT000006073189" sens="source" typelien="CITATION">Code de la sécurité sociale - article L321-1</LIEN>
<LIEN cidtexte="LEGITEXT000006073189" sens="source" typelien="CITATION">Code de la sécurité sociale - article L323-6</LIEN>
</LIENS>
</TEXTE_JURI_JUDI>"#;

    #[test]
    fn cass_record_maps_like_judilibre_and_round_trips() {
        let mut docs = parse_dila_doc(CASS_XML.as_bytes(), "dila-cass", DilaFond::Cass).unwrap();
        let Some(DilaDoc::Full(d)) = docs.pop() else {
            panic!("one full record expected");
        };
        assert_eq!(d.source_uid, "dila-cass/JURITEXT000028760256");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CC"));
        assert_eq!(d.chamber.as_deref(), Some("mi"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["12-20002".to_string(), "12-20003".to_string()][..])
        );
        assert_eq!(d.ecli.as_deref(), Some("ECLI:FR:CCASS:2014:MI00278"));
        assert_eq!(d.publication_codes, vec!["b"]);
        assert_eq!(d.type_recours.as_deref(), Some("Arrêt"));
        assert_eq!(
            d.themes,
            vec!["SECURITE SOCIALE, ASSURANCES SOCIALES - Maladie - Indemnité journalière"]
        );
        let attacked = d.attacked.clone().unwrap();
        assert_eq!(
            attacked.jurisdiction.as_deref(),
            Some("Conseil de prud'hommes de Saint-Etienne")
        );
        assert_eq!(attacked.date.as_deref(), Some("2012-03-22"));
        assert!(d
            .texte_integral_clean
            .contains("PAR CES MOTIFS :\nCASSE ET ANNULE"));
        let sf = build_source_fields_dila(CASS_XML.as_bytes());
        assert_eq!(
            sf.pointer("/META_JURI_JUDI/PUBLI_BULL/@publie")
                .and_then(Value::as_str),
            Some("oui")
        );
        assert_eq!(
            sf.pointer("/LIENS/LIEN/1/#text").and_then(Value::as_str),
            Some("Code de la sécurité sociale - article L323-6")
        );
        let rebuilt = <Decision as FromSourceFields>::from_source_fields(
            &d.texte_integral_clean,
            &sf,
            &d.source_uid,
        );
        assert_eq!(rebuilt, d);
    }

    #[test]
    fn corrections_table_parses_and_names_sections() {
        assert!(CORRECTIONS.len() > 2000);
        assert_eq!(corrected_sections("JURITEXT000006954504"), vec![1, 2]);
        assert_eq!(
            correction("JURITEXT000006975740", Some(2)).and_then(|c| c.date),
            Some("1967-11-23")
        );
    }

    #[test]
    fn capp_court_from_reading_or_label() {
        assert_eq!(
            capp_court(
                "JURITEXT000000000000",
                Some("Cour d'appel d'Aix-en-Provence")
            )
            .as_deref(),
            Some("ca_aix_en_provence")
        );
        assert_eq!(
            capp_court(
                "JURITEXT000006942281",
                Some("Tribunal de grande instance de Bordeaux")
            )
            .as_deref(),
            Some("tgi_bordeaux")
        );
        assert_eq!(court_code_type("tgi_bordeaux"), Some("TJ"));
        assert_eq!(court_code_type("cph_carcassonne"), Some("CPH"));
        assert_eq!(court_code_type("tsa_mamoudzou"), Some("CA"));
    }

    /// Spec: the forms `AVOCATS` takes in CASS records, a firm keeping its
    /// partners, a plural title several surnames, « Demandeur » the first named.
    #[test]
    fn avocats_splits_firms_and_titles_and_places_the_applicant() {
        let cases: &[(&str, Option<&str>, &[&str])] = &[
            ("Avocat : la SCP Coutard et Mayer.", None, &["SCP Coutard et Mayer"]),
            (
                "Avocats :la SCP Lyon-Caen, Fabiani et Thiriez, la SCP Ancel et Couturier-Heller .",
                None,
                &["SCP Lyon-Caen, Fabiani et Thiriez", "SCP Ancel et Couturier-Heller"],
            ),
            ("Avocats : M. Balat, la SCP Delaporte et Briard.", None, &["Balat", "SCP Delaporte et Briard"]),
            ("Avocats :MM. Capron, Roger .", None, &["Capron", "Roger"]),
            (
                "Avocats :La Société civile professionnelle Le Bret et de Lanouvelle, MM. Rouvière, Boulloche et Hennuyer.",
                None,
                &["SCP Le Bret et de Lanouvelle", "Rouvière", "Boulloche", "Hennuyer"],
            ),
            ("Me Georges, SCP Lyon-Caen, Fabiani et Thiriez", None, &["Georges", "SCP Lyon-Caen, Fabiani et Thiriez"]),
            ("SCP Peignot et Garreau, SCP Capron", None, &["SCP Peignot et Garreau", "SCP Capron"]),
            ("MM. Cossa, de Nervo (arrêt n° 1)", None, &["Cossa", "de Nervo"]),
            ("M. LE CESNE", None, &["LE CESNE"]),
            ("Demandeur M. Le Prado", Some("Le Prado"), &[]),
            ("Demandeur AV. MM. CELICE, SOURDILLAT", Some("CELICE"), &["SOURDILLAT"]),
            ("Av. Demandeur : SCP Calon Guiguet", Some("SCP Calon Guiguet"), &[]),
            ("Av. demandeur : Me Martin-Martinière, Me Mayer", Some("Martin-Martinière"), &["Mayer"]),
            ("Demandeur AV.", None, &[]),
            ("Demandeur AV. MM.", None, &[]),
            ("Avoué : Me David", None, &["David"]),
            ("Demandeur M. Defrénois, Boullez", Some("Defrénois"), &["Boullez"]),
            ("Demandeur AV MM RYZIGER, Défenseur VIDART", Some("RYZIGER"), &["VIDART"]),
            ("Demandeur M Riché, Défenseur M, Lemanissier", Some("Riché"), &["Lemanissier"]),
            (
                "Avocats : M. Verges, avocat au barreau de Paris, la SCP Lyon-Caen, Fabiani et Thiriez.",
                None,
                &["Verges", "SCP Lyon-Caen, Fabiani et Thiriez"],
            ),
            (
                "SCP Lyon-Caen, Fabiani et Thiriez la SCP Boré et Salve de Bruneton",
                None,
                &["SCP Lyon-Caen, Fabiani et Thiriez", "SCP Boré et Salve de Bruneton"],
            ),
            (
                "Avocats :la SCP Lyon-Caen, Fabiani et Thiriez, M. Le Prado. Soc., 21 juillet 1986, Bull. 1986",
                None,
                &["SCP Lyon-Caen, Fabiani et Thiriez", "Le Prado"],
            ),
        ];
        for (raw, applicant, listed) in cases {
            assert_eq!(
                avocats(raw),
                (
                    applicant.map(str::to_string),
                    listed.iter().map(|v| v.to_string()).collect()
                ),
                "{raw}"
            );
        }
    }
}
