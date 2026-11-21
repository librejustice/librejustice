//! How a place receives a journal: one line per journal, place and matter,
//! under one of four regimes, dated and sourced. And the matter of an article,
//! read from its code, its number in the code civil, or the title of its text,
//! never from its prose. Pure, embedded tables.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;

use crate::applicability::journal;

const RULES_TSV: &str = include_str!("../data/journal_rules.tsv");
const CODE_MATTERS_TSV: &str = include_str!("../data/code_matters.tsv");

/// The code civil, whose matter follows the number of the article.
const CODE_CIVIL: &str = "LEGITEXT000006070721";

/// What a line can name: the lists of the organic laws, cut down to what a
/// code, a range of the code civil or a title carries.
pub const MATTERS: &[&str] = &[
    "administrative_procedure",
    "administrative_relations",
    "civil_law",
    "civil_procedure",
    "civil_rights",
    "commerce",
    "consumer",
    "criminal_law",
    "criminal_procedure",
    "customs",
    "defence",
    "electoral",
    "energy",
    "environment",
    "expropriation",
    "foreigners",
    "health",
    "housing",
    "insurance",
    "labour",
    "money_credit",
    "nationality",
    "parental_authority",
    "personal_status",
    "post_telecom",
    "research",
    "road_traffic",
    "roads",
    "social_protection",
    "succession_matrimonial",
    "tax",
    "tourism",
    "urban_planning",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Regime {
    /// The place receives the text.
    Default,
    /// Only when a text names the place.
    Express,
    /// The matter passed to the place with no local instrument known: the
    /// national text stays there in its wording of the day of the transfer.
    Frozen,
    /// A known local instrument replaces the text.
    Excluded,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Regime::Default => "default",
            Regime::Express => "express",
            Regime::Frozen => "frozen",
            Regime::Excluded => "excluded",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" => Regime::Default,
            "express" => Regime::Express,
            "frozen" => Regime::Frozen,
            "excluded" => Regime::Excluded,
            _ => return None,
        })
    }
}

/// One line of the table: from `since`, the first day, to `until`, the first
/// day no longer covered. `matter` `None` is the `*` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub journal: &'static str,
    pub place: &'static str,
    pub matter: Option<&'static str>,
    pub regime: Regime,
    pub since: Option<Date>,
    pub until: Option<Date>,
    pub source: &'static str,
}

impl Rule {
    pub fn covers(&self, day: Date) -> bool {
        self.since.is_none_or(|s| s <= day) && self.until.is_none_or(|u| day < u)
    }

    /// Covers the stretch that starts at `start`, `None` being the open past.
    fn covers_from(&self, start: Option<Date>) -> bool {
        self.since.is_none_or(|s| start.is_some_and(|d| s <= d))
            && self.until.is_none_or(|u| start.is_none_or(|d| d < u))
    }
}

fn overlap(a: &Rule, b: &Rule) -> bool {
    let starts_before_end = |s: Option<Date>, e: Option<Date>| match (s, e) {
        (Some(s), Some(e)) => s < e,
        _ => true,
    };
    starts_before_end(a.since, b.until) && starts_before_end(b.since, a.until)
}

static RULES: LazyLock<BTreeMap<&'static str, Vec<Rule>>> = LazyLock::new(|| {
    let mut by_journal: BTreeMap<&'static str, Vec<Rule>> = BTreeMap::new();
    for l in RULES_TSV.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let f: Vec<&'static str> = l.split('\t').collect();
        assert_eq!(f.len(), 7, "journal_rules.tsv: {l}");
        let date = |s: &str| {
            (!s.is_empty()).then(|| {
                s.parse::<Date>()
                    .unwrap_or_else(|e| panic!("journal_rules.tsv: {s}: {e}"))
            })
        };
        let matter = match f[2] {
            "*" => None,
            m if MATTERS.contains(&m) => Some(m),
            m => panic!("journal_rules.tsv: unknown matter {m}"),
        };
        let rule = Rule {
            journal: f[0],
            place: f[1],
            matter,
            regime: Regime::parse(f[3])
                .unwrap_or_else(|| panic!("journal_rules.tsv: unknown regime {}", f[3])),
            since: date(f[4]),
            until: date(f[5]),
            source: f[6],
        };
        assert!(
            journal(rule.journal).is_some(),
            "journal_rules.tsv: journal {} is not in journals.tsv",
            rule.journal
        );
        assert!(rule.place.starts_with("geo:"), "journal_rules.tsv: {l}");
        assert!(!rule.source.is_empty(), "journal_rules.tsv: unsourced {l}");
        assert!(
            rule.regime != Regime::Frozen || rule.since.is_some(),
            "journal_rules.tsv: a frozen line names the day of its transfer, {l}"
        );
        if let (Some(s), Some(u)) = (rule.since, rule.until) {
            assert!(s < u, "journal_rules.tsv: empty line {l}");
        }
        by_journal.entry(rule.journal).or_default().push(rule);
    }
    for rules in by_journal.values() {
        for (i, a) in rules.iter().enumerate() {
            for b in &rules[i + 1..] {
                assert!(
                    !(a.place == b.place && a.matter == b.matter && overlap(a, b)),
                    "journal_rules.tsv: two lines of {} {} {:?} overlap",
                    a.journal,
                    a.place,
                    a.matter
                );
            }
        }
    }
    by_journal
});

/// The journal whose lines a text of `journal_code` resolves on: the BOCC and
/// the BOFiP resolve on the JORF's.
pub fn resolution_journal(journal_code: &str) -> &str {
    journal(journal_code)
        .and_then(|j| j.defaults_from)
        .unwrap_or(journal_code)
}

/// The lines a text of `journal_code` resolves on; none for a journal the
/// table does not name.
pub fn rules_of(journal_code: &str) -> &'static [Rule] {
    RULES
        .get(resolution_journal(journal_code))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The line that says how `place` receives a text of `journal_code` in
/// `matter` on `day`: the matter's line covering the day, else the `*` line.
/// `None`: the journal does not reach the place that day.
pub fn resolve(
    journal_code: &str,
    place: &str,
    matter: Option<&str>,
    day: Date,
) -> Option<&'static Rule> {
    let rules = rules_of(journal_code);
    let covering = |m: Option<&str>| {
        rules
            .iter()
            .find(|r| r.place == place && r.matter == m && r.covers(day))
    };
    matter
        .and_then(|m| covering(Some(m)))
        .or_else(|| covering(None))
}

/// Whether a journal reaches one of `path` by default on some day: how the
/// supranational orders a reader meets are known.
pub fn reaches(journal_code: &str, path: &[String]) -> bool {
    rules_of(journal_code)
        .iter()
        .any(|r| r.regime == Regime::Default && path.iter().any(|p| p == r.place))
}

/// The seat roots of the courts a reader of `path` answers to beside their own:
/// those ruling on the law of a journal that reaches the place by default.
pub fn court_orders(path: &[String]) -> Vec<&'static str> {
    crate::applicability::journals()
        .iter()
        .filter(|j| reaches(j.code, path))
        .filter_map(|j| j.courts)
        .collect()
}

/// A stretch on which a place receives the texts of a key by default, with
/// the source of the line that sets each bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultSpan {
    pub place: String,
    pub since: Option<Date>,
    pub until: Option<Date>,
    pub since_source: String,
    pub until_source: String,
}

impl DefaultSpan {
    pub fn undated(place: &str, source: &str) -> Self {
        DefaultSpan {
            place: place.to_string(),
            since: None,
            until: None,
            since_source: source.to_string(),
            until_source: source.to_string(),
        }
    }
}

/// The default spans of one place for one matter: the days are cut at every
/// bound of the lines that can decide, each stretch resolved, and the
/// stretches `default` lines decide kept, contiguous ones as one span. A
/// bound is sourced by the line that sets it: the span's own line when the
/// bound is that line's, else the neighbour that takes over.
fn spans_at(rules: &[Rule], place: &str, matter: Option<&str>) -> Vec<DefaultSpan> {
    let lines: Vec<&Rule> = rules
        .iter()
        .filter(|r| r.place == place && (r.matter.is_none() || r.matter == matter))
        .collect();
    let mut bounds: Vec<Date> = lines
        .iter()
        .flat_map(|r| [r.since, r.until])
        .flatten()
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let starts: Vec<Option<Date>> = std::iter::once(None)
        .chain(bounds.into_iter().map(Some))
        .collect();
    let mut stretches: Vec<(Option<Date>, Option<Date>, Option<&Rule>)> = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().flatten();
        let pick = |specific: bool| {
            lines
                .iter()
                .copied()
                .find(|r| r.matter.is_some() == specific && r.covers_from(start))
        };
        let line = pick(true).or_else(|| pick(false));
        match stretches.last_mut() {
            Some(last) if same_line(last.2, line) => last.1 = end,
            _ => stretches.push((start, end, line)),
        }
    }
    let is_default = |k: usize| {
        stretches
            .get(k)
            .and_then(|s| s.2)
            .is_some_and(|r| r.regime == Regime::Default)
    };
    let mut out = Vec::new();
    let mut k = 0;
    while k < stretches.len() {
        if !is_default(k) {
            k += 1;
            continue;
        }
        let mut last_k = k;
        while is_default(last_k + 1) {
            last_k += 1;
        }
        let (since, first) = (stretches[k].0, stretches[k].2.expect("a default stretch"));
        let (until, last) = (
            stretches[last_k].1,
            stretches[last_k].2.expect("a default stretch"),
        );
        let since_source = match since {
            Some(_) if first.since != since => stretches[k - 1].2.unwrap_or(first).source,
            _ => first.source,
        };
        let until_source = match until {
            Some(_) if last.until != until => {
                stretches
                    .get(last_k + 1)
                    .and_then(|s| s.2)
                    .unwrap_or(last)
                    .source
            }
            _ => last.source,
        };
        out.push(DefaultSpan {
            place: place.to_string(),
            since,
            until,
            since_source: since_source.to_string(),
            until_source: until_source.to_string(),
        });
        k = last_k + 1;
    }
    out
}

fn same_line(a: Option<&Rule>, b: Option<&Rule>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Every key a journal of the table can stand for, the journal alone and the
/// journal with each matter, with their spans place by place.
static KEY_SPANS: LazyLock<BTreeMap<String, Vec<DefaultSpan>>> = LazyLock::new(|| {
    let mut out = BTreeMap::new();
    for (j, rules) in RULES.iter() {
        let mut places: Vec<&str> = rules.iter().map(|r| r.place).collect();
        places.sort_unstable();
        places.dedup();
        let spans = |m: Option<&str>| -> Vec<DefaultSpan> {
            places.iter().flat_map(|p| spans_at(rules, p, m)).collect()
        };
        out.insert(j.to_string(), spans(None));
        for m in MATTERS {
            out.insert(format!("{j}:{m}"), spans(Some(m)));
        }
    }
    out
});

/// The key an article's defaults fold onto: the journal it resolves on, with
/// its matter when that journal has lines for it. `None` for a journal the
/// table does not name.
pub fn rule_key(journal_code: &str, matter: Option<&str>) -> Option<String> {
    let j = resolution_journal(journal_code);
    let rules = RULES.get(j)?;
    Some(
        match matter.filter(|m| rules.iter().any(|r| r.matter == Some(*m))) {
            Some(m) => format!("{j}:{m}"),
            None => j.to_string(),
        },
    )
}

/// The journal and the matter a key names.
pub fn parse_key(key: &str) -> (&str, Option<&str>) {
    match key.split_once(':') {
        Some((j, m)) => (j, Some(m)),
        None => (key, None),
    }
}

/// The default spans a key stands for. `None` for a key this binary's table
/// cannot unfold.
pub fn key_spans(key: &str) -> Option<&'static [DefaultSpan]> {
    KEY_SPANS.get(key).map(Vec::as_slice)
}

/// The keys whose spans reach one of `path` on `day`: what the corpus of a
/// place asks for beside its places.
pub fn keys_reaching(path: &[String], day: Date) -> Vec<String> {
    KEY_SPANS
        .iter()
        .filter(|(_, spans)| {
            spans.iter().any(|s| {
                path.contains(&s.place)
                    && s.since.is_none_or(|d| d <= day)
                    && s.until.is_none_or(|u| day < u)
            })
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// The keys whose line reaches one of `path` only on mention on `day`: a text
/// under one of them that names no place there is a doubt, not a text that
/// does not apply.
pub fn keys_on_mention(path: &[String], day: Date) -> Vec<String> {
    KEY_SPANS
        .keys()
        .filter(|key| {
            let (journal, matter) = parse_key(key);
            path.iter()
                .find_map(|p| resolve(journal, p, matter, day))
                .is_some_and(|line| line.regime == Regime::Express)
        })
        .cloned()
        .collect()
}

/// Every key with its spans, for the mirror the SQL passes read.
pub fn all_key_spans() -> impl Iterator<Item = (&'static str, &'static [DefaultSpan])> {
    KEY_SPANS.iter().map(|(k, s)| (k.as_str(), s.as_slice()))
}

struct CodeMatter {
    first: Option<u32>,
    last: Option<u32>,
    matter: &'static str,
}

static CODE_MATTERS: LazyLock<BTreeMap<&'static str, Vec<CodeMatter>>> = LazyLock::new(|| {
    let mut by_text: BTreeMap<&'static str, Vec<CodeMatter>> = BTreeMap::new();
    for l in CODE_MATTERS_TSV
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
    {
        let f: Vec<&'static str> = l.split('\t').collect();
        assert_eq!(f.len(), 5, "code_matters.tsv: {l}");
        assert!(
            MATTERS.contains(&f[3]),
            "code_matters.tsv: unknown matter {l}"
        );
        let number = |s: &str| {
            (!s.is_empty()).then(|| {
                s.parse::<u32>()
                    .unwrap_or_else(|e| panic!("code_matters.tsv: {s}: {e}"))
            })
        };
        let (first, last) = (number(f[1]), number(f[2]));
        assert_eq!(first.is_some(), last.is_some(), "code_matters.tsv: {l}");
        by_text.entry(f[0]).or_default().push(CodeMatter {
            first,
            last,
            matter: f[3],
        });
    }
    for (uid, rows) in &by_text {
        assert_eq!(
            rows.iter().filter(|r| r.first.is_none()).count(),
            1,
            "code_matters.tsv: {uid} needs exactly one whole-text line"
        );
    }
    by_text
});

/// Not « loi de finances »: most titles that name one distribute credits, and a
/// finance law mixes the tax with the budget.
static TAX_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bimp[oô]ts?\b|\btaxes?\b|\bfiscal").unwrap());

/// The matter of an article. The BOFiP is tax law whole; a code has its line,
/// the code civil one per range of its numbers; a JORF text without a code is
/// tax law when its title says so. `None` when nothing does: the article then
/// follows the `*` lines.
pub fn matter_of(
    journal_code: &str,
    text_uid: &str,
    num_key: &str,
    title: &str,
) -> Option<&'static str> {
    if journal_code == "BOFIP" {
        return Some("tax");
    }
    if let Some(rows) = CODE_MATTERS.get(text_uid) {
        let digits: String = num_key.chars().take_while(char::is_ascii_digit).collect();
        let number = digits
            .parse::<u32>()
            .ok()
            .filter(|_| text_uid == CODE_CIVIL);
        let ranged = number.and_then(|n| {
            rows.iter()
                .find(|r| r.first.is_some_and(|f| f <= n) && r.last.is_some_and(|l| n <= l))
        });
        return ranged
            .or_else(|| rows.iter().find(|r| r.first.is_none()))
            .map(|r| r.matter);
    }
    (journal_code == "JORF" && TAX_TITLE.is_match(title)).then_some("tax")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn spans_of<'a>(key: &str, place: &str) -> Vec<&'a DefaultSpan>
    where
        'static: 'a,
    {
        key_spans(key)
            .unwrap()
            .iter()
            .filter(|s| s.place == place)
            .collect()
    }

    #[test]
    fn the_tables_parse_and_every_matter_is_named_by_a_line() {
        assert!(!rules_of("JORF").is_empty());
        assert_eq!(rules_of("BOCC"), rules_of("JORF"));
        assert!(rules_of("JO-MA").is_empty());
        for m in MATTERS {
            assert!(
                rules_of("JORF").iter().any(|r| r.matter == Some(*m)),
                "{m} names no line"
            );
        }
        assert!(!CODE_MATTERS.is_empty());
    }

    #[test]
    fn a_matter_line_wins_over_the_star_line_on_its_days_only() {
        let tax_yt = |day: &str| resolve("JORF", "geo:YT", Some("tax"), d(day)).unwrap();
        assert_eq!(tax_yt("2009-06-01").regime, Regime::Express);
        assert_eq!(tax_yt("2012-06-01").regime, Regime::Excluded);
        assert_eq!(tax_yt("2015-06-01").regime, Regime::Default);
        assert_eq!(tax_yt("2015-06-01").matter, None);
        assert_eq!(
            resolve("JORF", "geo:PF", Some("labour"), d("2020-01-01"))
                .unwrap()
                .regime,
            Regime::Excluded
        );
        assert_eq!(
            resolve("JORF", "geo:PF", None, d("2020-01-01"))
                .unwrap()
                .regime,
            Regime::Express
        );
        assert_eq!(
            resolve("BOFIP", "geo:BL", Some("tax"), d("2020-01-01"))
                .unwrap()
                .regime,
            Regime::Default
        );
        assert!(resolve("JORF", "geo:BE", None, d("2020-01-01")).is_none());
        assert!(resolve("JOUE", "geo:GB", None, d("2021-06-01")).is_none());
    }

    #[test]
    fn a_span_bound_is_sourced_by_the_line_that_sets_it() {
        let yt = spans_of("JORF:tax", "geo:YT");
        assert_eq!(yt.len(), 1);
        assert_eq!(yt[0].since, Some(d("2014-01-01")));
        assert!(yt[0].since_source.contains("2013-837"));
        assert_eq!(yt[0].until, None);

        let nationality = spans_of("JORF:nationality", "geo:YT");
        assert_eq!(nationality.len(), 1, "deux lignes contiguës, une étendue");
        assert_eq!(nationality[0].since, Some(d("2001-07-13")));
        assert!(nationality[0].since_source.contains("2001-616"));
        assert_eq!(nationality[0].until, None);

        let bl = spans_of("JORF", "geo:BL");
        assert_eq!((bl.len(), bl[0].since), (1, Some(d("1946-03-19"))));
        let mf = spans_of("JORF:urban_planning", "geo:MF");
        assert_eq!(mf[0].until, Some(d("2012-01-01")));
        assert!(mf[0].until_source.contains("LO 6314-3"));

        let star = spans_of("JORF", "geo:YT");
        assert_eq!(star.len(), 1);
        assert_eq!(star[0].since, Some(d("2008-01-01")));
        assert!(star[0].since_source.contains("2010-1486"));

        let ch = spans_of("RO", "geo:CH");
        assert_eq!((ch[0].since, ch[0].until), (None, None));
        assert!(spans_of("JORF", "geo:PF").is_empty());
        assert_eq!(
            spans_of("JORF:administrative_procedure", "geo:PF")[0].since,
            Some(d("2001-01-01"))
        );
    }

    #[test]
    fn a_key_names_the_matter_only_when_the_journal_has_lines_for_it() {
        assert_eq!(rule_key("JORF", None).as_deref(), Some("JORF"));
        assert_eq!(rule_key("JORF", Some("tax")).as_deref(), Some("JORF:tax"));
        assert_eq!(rule_key("BOFIP", Some("tax")).as_deref(), Some("JORF:tax"));
        assert_eq!(rule_key("BOCC", None).as_deref(), Some("JORF"));
        assert_eq!(rule_key("JOUE", Some("tax")).as_deref(), Some("JOUE"));
        assert_eq!(rule_key("JO-MA", None), None);
        assert_eq!(key_spans("JOUE:tax"), key_spans("JOUE"));
        assert_eq!(parse_key("JORF:tax"), ("JORF", Some("tax")));
        assert!(key_spans("JORF:nothing").is_none());
    }

    #[test]
    fn a_place_asks_for_the_keys_that_reach_it_that_day() {
        let path = |p: &str| vec![p.to_string(), "geo:FR".to_string()];
        let yt_2010 = keys_reaching(&path("geo:YT"), d("2010-01-01"));
        assert!(yt_2010.contains(&"JORF".to_string()));
        assert!(!yt_2010.contains(&"JORF:tax".to_string()));
        assert!(keys_reaching(&path("geo:YT"), d("2015-01-01")).contains(&"JORF:tax".to_string()));
        let pf = keys_reaching(&path("geo:PF"), d("2020-01-01"));
        assert!(pf.contains(&"JORF:administrative_procedure".to_string()));
        assert!(pf.contains(&"JOPF".to_string()));
        assert!(!pf.contains(&"JORF".to_string()));
        let flanders = vec!["geo:BE-VLG".to_string(), "geo:BE".to_string()];
        assert!(keys_reaching(&flanders, d("2020-01-01")).contains(&"MB".to_string()));
        let pf_mention = keys_on_mention(&path("geo:PF"), d("2020-01-01"));
        assert!(pf_mention.contains(&"JORF".to_string()));
        assert!(pf_mention.contains(&"JORF:criminal_law".to_string()));
        assert!(!pf_mention.contains(&"JORF:labour".to_string()));
        assert!(!pf_mention.contains(&"JORF:civil_law".to_string()));
        assert!(!pf_mention.contains(&"JORF:administrative_procedure".to_string()));
        assert!(keys_on_mention(&path("geo:FR-M"), d("2020-01-01")).is_empty());
    }

    #[test]
    fn the_code_civil_reads_its_matter_from_the_article_number() {
        let cc = |num: &str| matter_of("JORF", CODE_CIVIL, num, "Code civil");
        assert_eq!(cc("9"), Some("civil_rights"));
        assert_eq!(cc("16-14"), Some("civil_rights"));
        assert_eq!(cc("21-2"), Some("nationality"));
        assert_eq!(cc("311-25"), Some("personal_status"));
        assert_eq!(cc("371-1"), Some("parental_authority"));
        assert_eq!(cc("515-8"), Some("personal_status"));
        assert_eq!(cc("720"), Some("succession_matrimonial"));
        assert_eq!(cc("1099-1"), Some("succession_matrimonial"));
        assert_eq!(cc("1240"), Some("civil_law"));
        assert_eq!(cc("1387"), Some("succession_matrimonial"));
        assert_eq!(cc("2224"), Some("civil_law"));
    }

    #[test]
    fn a_text_without_a_code_is_tax_law_by_its_title_alone() {
        let jorf = |title: &str| matter_of("JORF", "JORFTEXT000000000001", "1", title);
        assert_eq!(
            jorf("LOI n° 2019-1479 du 28 décembre 2019 de finances pour 2020"),
            None
        );
        assert_eq!(
            jorf("Décret portant répartition des crédits ouverts par la loi de finances pour 1993"),
            None
        );
        assert_eq!(
            jorf("Décret relatif à la taxe d'apprentissage"),
            Some("tax")
        );
        assert_eq!(jorf("Arrêté relatif à l'impôt sur le revenu"), Some("tax"));
        assert_eq!(
            jorf("Décret portant réforme de la fiscalité locale"),
            Some("tax")
        );
        assert_eq!(jorf("Décret relatif à la taxation d'office"), None);
        assert_eq!(jorf("Décret relatif aux taxis"), None);
        assert_eq!(matter_of("BOCC", "KALICONT000005635691", "1", "Taxe"), None);
        assert_eq!(matter_of("BOFIP", "BOI-IR-BASE-10", "1", ""), Some("tax"));
        assert_eq!(
            matter_of(
                "JORF",
                "LEGITEXT000006069577",
                "1649-a",
                "Code général des impôts"
            ),
            Some("tax")
        );
        assert_eq!(
            matter_of(
                "JORF",
                "LEGITEXT000006070716",
                "700",
                "Code de procédure civile"
            ),
            Some("civil_procedure")
        );
    }
}
