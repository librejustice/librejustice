//! The text a query names by an identifier: a Légifrance text id, a BOFiP id, a CELEX, an
//! ELI, a NOR, its nature and official number, or a usual name. PURE.

use std::sync::LazyLock;

use regex::Regex;

use crate::aliases::NamedTextTarget;

/// What identifies a text, in the shape of the column that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextIdentity {
    /// `legal_text.text_uid`: a Légifrance text id, a BOFiP id, or the uid of an EU act.
    Uid(String),
    /// `legal_text.slug`.
    Slug(String),
    /// `legal_text.nor`.
    Nor(String),
    /// `legal_text.eli`, in the Légifrance URL form the column holds.
    Eli(String),
    /// A nature and its official number, read off the title.
    Act(ActNumber),
}

/// An official number with the nature that carries it: « règlement (UE)
/// 2016/679 » is `reglement` and `2016/679`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActNumber {
    /// Folded nature word, one of [`NATURES`].
    pub nature: &'static str,
    /// The number as the title index holds it, one token: `2016/679`, `2019-1333`.
    pub number: String,
}

/// Natures written before an official number, folded.
const NATURES: [&str; 7] = [
    "reglement",
    "directive",
    "decision",
    "loi",
    "ordonnance",
    "decret",
    "arrete",
];

/// Words that may stand between the nature and its number: the abbreviation of
/// « numéro », the Union's acronyms, and the qualifiers of a nature.
const BEFORE_NUMBER: [&str; 12] = [
    "n",
    "no",
    "numero",
    "ue",
    "ce",
    "cee",
    "euratom",
    "pesc",
    "d",
    "execution",
    "delegue",
    "organique",
];

/// The title index's split, words cut at hyphens: `2016/679` stays one token,
/// as it is one term of the index.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{N}+(?:[./-]\p{N}+)+|[\p{L}\p{N}]+").expect("token pattern"));

static LEGIFRANCE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:JORFTEXT|LEGITEXT|KALITEXT|KALICONT)\d{12}\b").expect("id pattern")
});

/// Never after a hyphen: slugs such as `…-def-emat-boi-coat-aero-…` carry the word.
static BOFIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^\w-]|%3D)(BOI(?:-[A-Z0-9]+)+)").expect("bofip pattern")
});

/// Sector 3 names an act; sector 0 its consolidated versions, `-YYYYMMDD` after.
static CELEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[03]\d{4}[RL]\d{4}\b").expect("celex pattern"));

static ELI_EU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)data\.europa\.eu/eli/(reg|dir)/(\d{4})/(\d+)\b").expect("eu eli pattern")
});

static ELI_FR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/eli/[^\s?#]+").expect("eli pattern"));

static NOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[A-Z]{4}\d{7}[A-Z]\b").expect("nor pattern"));

/// A text the query names, and whether naming it is all the query does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedIdentity {
    pub identity: TextIdentity,
    /// Nothing around the name but [`FILLERS`] and the act's date: the query is
    /// the name, where « RGPD droit d'accès » also asks something of the text.
    pub whole: bool,
}

/// What may stand around a name without asking anything of the text: articles
/// and prepositions, an identifier's label, the Union's acronyms, an act's authors.
const FILLERS: [&str; 27] = [
    "le",
    "la",
    "les",
    "l",
    "du",
    "de",
    "des",
    "d",
    "et",
    "au",
    "aux",
    "a",
    "n",
    "no",
    "numero",
    "nor",
    "celex",
    "eli",
    "ue",
    "ce",
    "cee",
    "euratom",
    "pesc",
    "parlement",
    "europeen",
    "conseil",
    "commission",
];

/// The identity `query` names, the most explicit form first: a Légifrance id,
/// a BOFiP id, a CELEX, an ELI, a NOR, a nature and number, then a usual name.
pub fn text_identity(query: &str) -> Option<NamedIdentity> {
    let (identity, rest) = identify(query)?;
    let whole = rest.iter().all(|t| is_filler(t));
    Some(NamedIdentity { identity, whole })
}

fn identify(query: &str) -> Option<(TextIdentity, Vec<String>)> {
    if let Some(m) = LEGIFRANCE_TEXT.find(query) {
        let uid = TextIdentity::Uid(m.as_str().to_uppercase());
        return Some((uid, around(query, m.range())));
    }
    if let Some((range, uid)) = bofip(query) {
        return Some((TextIdentity::Uid(uid), around(query, range)));
    }
    if let Some((m, uid)) = CELEX.find(query).and_then(|m| {
        celex_text_uid(&format!("3{}", m.as_str()[1..].to_uppercase())).map(|uid| (m, uid))
    }) {
        return Some((TextIdentity::Uid(uid), around(query, m.range())));
    }
    if let Some(c) = ELI_EU.captures(query) {
        let kind = match c[1].to_lowercase().as_str() {
            "reg" => "REG",
            _ => "DIR",
        };
        let seq = c[3].trim_start_matches('0');
        let uid = TextIdentity::Uid(format!("EU/{kind}/{seq}-{}", &c[2]));
        return Some((uid, around(query, c.get(0)?.range())));
    }
    if let Some(m) = ELI_FR.find(query) {
        let path = m.as_str().trim_end_matches('/');
        let eli = TextIdentity::Eli(format!("https://www.legifrance.gouv.fr{path}"));
        return Some((eli, around(query, m.range())));
    }
    if let Some(m) = NOR.find(query) {
        let nor = TextIdentity::Nor(m.as_str().to_uppercase());
        return Some((nor, around(query, m.range())));
    }
    let tokens = tokens(query);
    if let Some((i, (act, j))) = (0..tokens.len()).find_map(|i| act_at(&tokens, i).map(|a| (i, a)))
    {
        let rest = tokens[..i]
            .iter()
            .chain(&tokens[j + 1..])
            .cloned()
            .collect();
        return Some((TextIdentity::Act(act), rest));
    }
    let (target, rest) = crate::aliases::anchored_alias(query)?;
    let identity = match target {
        NamedTextTarget::Uid(uid) => TextIdentity::Uid(uid),
        NamedTextTarget::Slug(slug) => TextIdentity::Slug(slug),
        NamedTextTarget::SlugPatterns(_) => return None,
    };
    Some((identity, rest))
}

/// The tokens of `query` outside the word that holds `range`: a URL is one word.
fn around(query: &str, range: std::ops::Range<usize>) -> Vec<String> {
    let start = query[..range.start]
        .trim_end_matches(|c: char| !c.is_whitespace())
        .len();
    let end = query[range.end..]
        .find(char::is_whitespace)
        .map_or(query.len(), |i| range.end + i);
    tokens(&format!("{} {}", &query[..start], &query[end..]))
}

/// A filler, a day of the month or a year: « du 6 octobre 2026 » dates the act.
fn is_filler(token: &str) -> bool {
    FILLERS.contains(&token)
        || crate::aliases::FR_MONTHS.contains(&token)
        || token == "1er"
        || token
            .parse::<u16>()
            .is_ok_and(|n| (1..=31).contains(&n) || (1700..=2100).contains(&n))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Article,
    Section,
    Text,
}

/// In precedence order: a pasted permalink names the text before the article
/// or the section, and the narrowest target is the one meant.
const LEGIFRANCE_PREFIXES: [(&str, IdKind); 6] = [
    ("LEGIARTI", IdKind::Article),
    ("JORFARTI", IdKind::Article),
    ("KALIARTI", IdKind::Article),
    ("LEGISCTA", IdKind::Section),
    ("LEGITEXT", IdKind::Text),
    ("JORFTEXT", IdKind::Text),
];

/// A Légifrance id is recognised by its shape, not by the key carrying it:
/// callers put it in `textId` as readily as in `url`, bare or inside a pasted
/// permalink. The digit run bounds it, so a trailing `/2024-01-01` or a query
/// string cannot enter the id.
pub fn legifrance_id(value: &str) -> Option<(IdKind, &str)> {
    LEGIFRANCE_PREFIXES.iter().find_map(|&(prefix, kind)| {
        let at = value.find(prefix)?;
        let start = at + prefix.len();
        let digits = value[start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        (digits > 0).then(|| (kind, &value[at..start + digits]))
    })
}

/// `text_uid` of the BOFiP document `value` names.
pub fn bofip_uid(value: &str) -> Option<String> {
    bofip(value).map(|(_, uid)| uid)
}

/// The id and where it stands. A permalink appends the version's `-YYYYMMDD`,
/// and no id has an eight-digit segment of its own.
fn bofip(value: &str) -> Option<(std::ops::Range<usize>, String)> {
    let m = BOFIP.captures(value)?.get(1)?;
    let id = m.as_str().to_uppercase();
    let id = match id.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
            head.to_string()
        }
        _ => id,
    };
    Some((m.range(), id))
}

/// The first nature and number `query` carries, wherever it stands.
pub fn act_number(query: &str) -> Option<ActNumber> {
    let tokens = tokens(query);
    (0..tokens.len()).find_map(|i| act_at(&tokens, i).map(|(act, _)| act))
}

/// The title's own number, at its head: « Règlement (UE) 2025/2518 modifiant le
/// règlement (UE) 2016/679 » is 2025/2518.
pub fn head_act_number(title: &str) -> Option<ActNumber> {
    act_at(&tokens(title), 0).map(|(act, _)| act)
}

/// `text_uid` of the act a sector-3 CELEX names: `32016R0679` is
/// `EU/REG/679-2016`, `31977L0388` is `EU/DIR/388-1977`.
pub fn celex_text_uid(celex: &str) -> Option<String> {
    let ch: Vec<char> = celex.chars().collect();
    if ch.len() < 7 || ch[0] != '3' {
        return None;
    }
    let year: String = ch[1..5].iter().collect();
    let seq: String = ch[6..].iter().collect();
    if !year.chars().all(|c| c.is_ascii_digit()) || !seq.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let seq = match seq.trim_start_matches('0') {
        "" => "0",
        s => s,
    };
    let kind = match ch[5] {
        'R' => "REG",
        'L' => "DIR",
        _ => return None,
    };
    Some(format!("EU/{kind}/{seq}-{year}"))
}

/// The CELEX of a text [`celex_text_uid`] names: `EU/REG/679-2016` is `32016R0679`.
pub fn text_uid_celex(text_uid: &str) -> Option<String> {
    let (kind, rest) = text_uid.strip_prefix("EU/")?.split_once('/')?;
    let letter = match kind {
        "REG" => 'R',
        "DIR" => 'L',
        _ => return None,
    };
    let (seq, year) = rest.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    (digits(seq) && digits(year) && year.len() == 4).then(|| format!("3{year}{letter}{seq:0>4}"))
}

fn tokens(s: &str) -> Vec<String> {
    TOKEN
        .find_iter(s)
        .map(|m| lj_fold::fold(m.as_str()))
        .collect()
}

/// The act whose nature stands at `i`, and the index of its number.
fn act_at(tokens: &[String], i: usize) -> Option<(ActNumber, usize)> {
    let nature = NATURES
        .iter()
        .find(|n| Some(**n) == tokens.get(i).map(String::as_str))?;
    let j = (i + 1..tokens.len()).find(|&j| !BEFORE_NUMBER.contains(&tokens[j].as_str()))?;
    let number = &tokens[j];
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let mut parts = number.split(['/', '-']);
    let pair = matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), None) if digits(a) && digits(b)
    );
    pair.then(|| {
        let act = ActNumber {
            nature,
            number: number.clone(),
        };
        (act, j)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(nature: &'static str, number: &str) -> Option<TextIdentity> {
        Some(TextIdentity::Act(ActNumber {
            nature,
            number: number.into(),
        }))
    }

    #[test]
    fn reads_every_form_of_identity() {
        let uid = |u: &str| Some(TextIdentity::Uid(u.into()));
        let slug = |s: &str| Some(TextIdentity::Slug(s.into()));
        let cases: Vec<(&str, Option<TextIdentity>)> = vec![
            ("JORFTEXT000039480084", uid("JORFTEXT000039480084")),
            (
                "https://www.legifrance.gouv.fr/loda/id/LEGITEXT000006069565/",
                uid("LEGITEXT000006069565"),
            ),
            ("kalicont000005635552", uid("KALICONT000005635552")),
            ("BOI-RSA-CHAMP-20-40-10-20", uid("BOI-RSA-CHAMP-20-40-10-20")),
            ("boi-djc-covid19-20 § 30", uid("BOI-DJC-COVID19-20")),
            (
                "https://bofip.impots.gouv.fr/bofip/5123-PGP.html/identifiant=BOI-RSA-CHAMP-20-40-10-20-20250429",
                uid("BOI-RSA-CHAMP-20-40-10-20"),
            ),
            (
                "bofip.impots.gouv.fr/bofip/1234-PGP.html/identifiant%3DBOI-ANNX-000415-20240117",
                uid("BOI-ANNX-000415"),
            ),
            ("32016R0679", uid("EU/REG/679-2016")),
            ("celex 32019L1937 article 6", uid("EU/DIR/1937-2019")),
            ("02016R0679-20160504", uid("EU/REG/679-2016")),
            (
                "http://data.europa.eu/eli/reg/2016/679/oj",
                uid("EU/REG/679-2016"),
            ),
            (
                "https://www.legifrance.gouv.fr/eli/decret/2026/10/6/TRSD2612399D/jo/texte",
                Some(TextIdentity::Eli(
                    "https://www.legifrance.gouv.fr/eli/decret/2026/10/6/TRSD2612399D/jo/texte"
                        .into(),
                )),
            ),
            (
                "TRSD2612399D",
                Some(TextIdentity::Nor("TRSD2612399D".into())),
            ),
            (
                "nor jusc1927307d",
                Some(TextIdentity::Nor("JUSC1927307D".into())),
            ),
            ("règlement (UE) 2016/679", act("reglement", "2016/679")),
            ("Règlement (CEE) n° 1408/71", act("reglement", "1408/71")),
            (
                "Dublin III règlement (UE) n° 604/2013",
                act("reglement", "604/2013"),
            ),
            (
                "règlement d'exécution (UE) 2023/1201",
                act("reglement", "2023/1201"),
            ),
            ("directive 2008/115/CE", act("directive", "2008/115")),
            (
                "directive (UE) 2019/1937 article 6",
                act("directive", "2019/1937"),
            ),
            (
                "décret n° 2026-929 du 6 octobre 2026",
                act("decret", "2026-929"),
            ),
            ("Décret n°95-889 du 7 août 1995", act("decret", "95-889")),
            ("loi 2026-307", act("loi", "2026-307")),
            ("loi organique n° 2001-692", act("loi", "2001-692")),
            (
                "préavis ordonnance 2017-1387 licenciement",
                act("ordonnance", "2017-1387"),
            ),
            (
                "RGPD article 5",
                uid("EU/REG/679-2016"),
            ),
            (
                "rgpd",
                uid("EU/REG/679-2016"),
            ),
            (
                "loi du 6 juillet 1989",
                uid("JORFTEXT000000509310"),
            ),
            ("article 145 cpc", slug("code-de-procedure-civile")),
            ("CEDH article 8", uid("INTL/CEDH")),
            ("cide article 3", uid("INTL/CIDE")),
            (
                "DSA modération des contenus",
                uid("EU/REG/2065-2022"),
            ),
        ];
        for (query, want) in cases {
            assert_eq!(text_identity(query).map(|n| n.identity), want, "{query}");
        }
    }

    #[test]
    fn the_name_alone_is_the_whole_query() {
        for (query, whole) in [
            ("règlement (UE) 2016/679", true),
            ("32016R0679", true),
            ("nor jusc1927307d", true),
            (
                "https://www.legifrance.gouv.fr/loda/id/LEGITEXT000006069565/",
                true,
            ),
            ("décret n° 2026-929 du 6 octobre 2026", true),
            (
                "règlement (UE) 2016/679 du Parlement européen et du Conseil du 27 avril 2016",
                true,
            ),
            ("le RGPD", true),
            ("BOI-BIC-PROV-50", true),
            ("BOI-BIC-PROV-50 n° 130", false),
            ("loi du 6 juillet 1989", true),
            ("rgpd droit d'accès", false),
            ("CEDH article 8", false),
            ("celex 32019L1937 article 6", false),
            ("Dublin III règlement (UE) n° 604/2013", false),
            ("préavis ordonnance 2017-1387 licenciement", false),
        ] {
            assert_eq!(
                text_identity(query).map(|n| n.whole),
                Some(whole),
                "{query}"
            );
        }
    }

    #[test]
    fn names_no_text() {
        for query in [
            "2016/679",
            "décret du 2 octobre 2026",
            "article 1240",
            "délai de recours contentieux refus implicite",
            "décision du 06/10/2026",
            "le conducteur qui tue un piéton",
            "arrêté préfectoral",
            "constitution de partie civile",
            "/norm/fr/instruction-n-2862-def-emat-boi-coat-aero-relative-a-la-reglementation",
        ] {
            assert_eq!(text_identity(query), None, "{query}");
        }
    }

    #[test]
    fn head_number_is_the_titles_own() {
        let cases = [
            (
                "Règlement (UE) 2025/2518 du Parlement européen modifiant le règlement (UE) 2016/679",
                Some("2025/2518"),
            ),
            ("Directive 2008/115/CE (retour des ressortissants)", Some("2008/115")),
            ("Règlement (UE) n° 604/2013 (Dublin III)", Some("604/2013")),
            ("LOI n° 2018-771 du 5 septembre 2018", Some("2018-771")),
            ("Rapport au Président de la République relatif à l'ordonnance n° 2019-861", None),
            ("Code civil", None),
        ];
        for (title, want) in cases {
            assert_eq!(
                head_act_number(title).map(|a| a.number),
                want.map(str::to_string),
                "{title}"
            );
        }
    }

    #[test]
    fn celex_maps_to_the_existing_uid_format() {
        assert_eq!(
            celex_text_uid("32016R0679").as_deref(),
            Some("EU/REG/679-2016")
        );
        assert_eq!(
            celex_text_uid("31977L0388").as_deref(),
            Some("EU/DIR/388-1977")
        );
        assert_eq!(
            celex_text_uid("32001R0044").as_deref(),
            Some("EU/REG/44-2001")
        );
        assert_eq!(celex_text_uid("32016D0679"), None);
    }

    #[test]
    fn a_uid_gives_back_the_celex_it_was_named_from() {
        for celex in ["32016R0679", "31977L0388", "32001R0044", "31962R0017"] {
            let uid = celex_text_uid(celex).unwrap();
            assert_eq!(text_uid_celex(&uid).as_deref(), Some(celex));
        }
        assert_eq!(text_uid_celex("EU/REG/1408-71"), None);
        assert_eq!(text_uid_celex("EU/DEC/1-2020"), None);
    }
}
