//! The history of a Belgian text from Justel's pages: an archived version holds until the day its
//! page names, which opens the form the next one shows.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_store::repository::StoredArticleVersion;

use super::super::legal_corpus::{article_status, CorpusArticle, CorpusVersion};

/// An article of a page, its body normalised as the base stores it.
pub(super) struct Article {
    pub num: String,
    pub title_path: Option<String>,
    pub texte: String,
    pub amended: Vec<NaiveDate>,
}

const SUFFIXES: [&str; 10] = [
    "bis",
    "ter",
    "quater",
    "quinquies",
    "sexies",
    "septies",
    "octies",
    "novies",
    "nonies",
    "decies",
];

/// `7bis` as `7 bis`, the way a text whose article URLs were minted so keeps them.
fn spaced(num: &str) -> String {
    let digits = num.bytes().take_while(u8::is_ascii_digit).count();
    let (head, tail) = num.split_at(digits);
    if digits > 0 && SUFFIXES.contains(&tail) {
        format!("{head} {tail}")
    } else {
        num.to_string()
    }
}

/// A book of a code that a document carries among the articles of the law enacting it: its
/// articles' numbers start with `prefix`, and `label` heads their title path.
pub(super) struct Book {
    pub prefix: &'static str,
    pub label: &'static str,
}

impl Book {
    /// The label, then the document's headings below the ones the label already names.
    fn path(&self, path: Option<&str>) -> String {
        let key = |s: &str| -> String {
            s.chars()
                .flat_map(char::to_lowercase)
                .filter(char::is_ascii_alphanumeric)
                .collect()
        };
        let label: Vec<String> = self.label.split(" > ").map(key).collect();
        let segments: Vec<&str> = path.map(|p| p.split(" > ").collect()).unwrap_or_default();
        let named = segments
            .iter()
            .take_while(|s| {
                let k = key(s);
                !k.is_empty() && label.iter().any(|l| l.contains(&k))
            })
            .count();
        std::iter::once(self.label)
            .chain(segments[named..].iter().copied())
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

/// One spelling per article key, the first met: an archive's `346.2` is the current page's `346-2`.
#[derive(Default)]
pub(super) struct Spellings(HashMap<String, String>);

impl Spellings {
    fn spell(&mut self, num: String) -> String {
        self.0
            .entry(lj_norm::article_key::identity_key(&num))
            .or_insert(num)
            .clone()
    }
}

pub(super) fn articles(
    page: Vec<lj_parse::JustelArticle>,
    spaced_suffix: bool,
    book: Option<&Book>,
    spellings: &mut Spellings,
) -> Result<Vec<Article>> {
    let mut seen = HashSet::new();
    page.into_iter()
        .filter(|a| book.is_none_or(|b| a.num.starts_with(b.prefix)))
        .map(|a| {
            let amended = a
                .amended
                .iter()
                .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow!("article {}: {e}", a.num))?;
            Ok(Article {
                num: spellings.spell(if spaced_suffix { spaced(&a.num) } else { a.num }),
                title_path: match book {
                    Some(b) => Some(b.path(a.title_path.as_deref())),
                    None => a.title_path,
                },
                texte: lj_core::normalizer::normalize_chars(&a.texte),
                amended,
            })
        })
        // An act that embeds an agreement numbers its articles again: the act's own come first.
        .filter(|a| a.as_ref().map_or(true, |a| seen.insert(a.num.clone())))
        .collect()
}

struct Form {
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    texte: String,
}

/// An article's forms so far, and the day the latest page that showed it, or left it out of a
/// whole text, stopped applying.
#[derive(Default)]
struct History {
    forms: Vec<Form>,
    edge: Option<NaiveDate>,
    absent: bool,
    title_path: Option<String>,
}

impl History {
    fn present(&mut self, a: &Article, until: Option<NaiveDate>, born: Option<NaiveDate>) {
        if a.title_path.is_some() {
            self.title_path.clone_from(&a.title_path);
        }
        if !self.absent {
            if let Some(last) = self.forms.last_mut() {
                if last.texte == a.texte || last.end.is_none() {
                    last.texte.clone_from(&a.texte);
                    last.end = match (last.end, until) {
                        (Some(end), Some(until)) => Some(end.max(until)),
                        (_, until) => until,
                    };
                    self.edge = last.end;
                    return;
                }
            }
        }
        let start = if self.forms.is_empty() {
            let amended = a
                .amended
                .iter()
                .copied()
                .filter(|d| until.is_none_or(|u| *d < u))
                .max();
            [self.edge, amended, born].into_iter().flatten().max()
        } else {
            self.edge
        };
        if let (Some(start), Some(until)) = (start, until) {
            if until <= start {
                return;
            }
        }
        self.forms.push(Form {
            start,
            end: until,
            texte: a.texte.clone(),
        });
        self.edge = until;
        self.absent = false;
    }

    /// Left out of a whole text that held until `until`: a form read open is withdrawn.
    fn absent(&mut self, until: Option<NaiveDate>) {
        if self.forms.last().is_some_and(|f| f.end.is_none()) {
            self.forms.pop();
        }
        if let Some(until) = until {
            self.edge = Some(self.edge.map_or(until, |e| e.max(until)));
        }
        self.absent = true;
    }

    fn repeal(&mut self, on: NaiveDate) {
        if let Some(last) = self.forms.last_mut().filter(|f| f.end.is_none()) {
            if last.start.is_some_and(|s| s >= on) {
                self.forms.pop();
            } else {
                last.end = Some(on);
            }
        }
    }
}

/// A document of the text: the articles its pages show, the day it entered into force when every
/// amendment since bears a note, and the end of the latest whole version read.
pub(super) struct Document {
    nums: HashSet<String>,
    born: Option<NaiveDate>,
    whole_until: Option<NaiveDate>,
}

impl Document {
    pub fn new(current: &[Article], born: Option<NaiveDate>) -> Self {
        Self {
            nums: current.iter().map(|a| a.num.clone()).collect(),
            born,
            whole_until: None,
        }
    }
}

/// The articles of a text as Justel's pages are read, oldest first, beginning from the versions
/// the base holds when the pages read only continue them.
#[derive(Default)]
pub(super) struct Histories {
    by_num: HashMap<String, History>,
    seen: Vec<String>,
    after: HashMap<String, Option<String>>,
}

impl Histories {
    pub fn from_stored(stored: &[StoredArticleVersion], spellings: &mut Spellings) -> Self {
        let open = NaiveDate::from_ymd_opt(1, 1, 1).unwrap();
        let mut by_position: Vec<&StoredArticleVersion> = stored.iter().collect();
        by_position.sort_by_key(|v| v.position);
        let mut out = Self::default();
        let mut previous: Option<String> = None;
        for v in by_position {
            let num = spellings.spell(v.num.clone());
            if !out.by_num.contains_key(&num) {
                out.seen.push(num.clone());
                out.after.insert(num.clone(), previous.replace(num.clone()));
            }
            let h = out.by_num.entry(num).or_default();
            h.forms.push(Form {
                start: (v.date_debut != open).then_some(v.date_debut),
                end: v.date_fin,
                texte: v.texte.clone(),
            });
            h.title_path.clone_from(&v.title_path);
        }
        for h in out.by_num.values_mut() {
            h.forms.sort_by_key(|f| f.start);
            h.edge = h.forms.last().and_then(|f| f.end);
        }
        out
    }

    fn history(&mut self, num: &str, doc: &Document) -> &mut History {
        if !self.by_num.contains_key(num) {
            self.seen.push(num.to_string());
        }
        self.by_num
            .entry(num.to_string())
            .or_insert_with(|| History {
                edge: doc.whole_until,
                absent: doc.whole_until.is_some(),
                ..History::default()
            })
    }

    /// One page of `doc`: an archived version holding until `until`, or the current text.
    pub fn observe(
        &mut self,
        doc: &mut Document,
        page: &[Article],
        until: Option<NaiveDate>,
        whole: bool,
    ) {
        let mut previous: Option<String> = None;
        for a in page {
            self.history(&a.num, doc).present(a, until, doc.born);
            self.after
                .insert(a.num.clone(), previous.replace(a.num.clone()));
            doc.nums.insert(a.num.clone());
        }
        if whole {
            let shown: HashSet<&str> = page.iter().map(|a| a.num.as_str()).collect();
            let missing: Vec<String> = doc
                .nums
                .iter()
                .filter(|n| !shown.contains(n.as_str()))
                .cloned()
                .collect();
            for num in missing {
                self.history(&num, doc).absent(until);
            }
            if until.is_some() {
                doc.whole_until = until;
            }
        }
    }

    /// The whole document repealed on `on`: each of its articles still open closes that day.
    pub fn repeal(&mut self, doc: &Document, on: NaiveDate) {
        for num in &doc.nums {
            if let Some(h) = self.by_num.get_mut(num) {
                h.repeal(on);
            }
        }
    }

    /// The text's articles in the order of its current pages, an article they no longer show after
    /// the one it followed when last shown.
    pub fn into_articles(mut self, current: &[String]) -> Vec<CorpusArticle> {
        let mut order: Vec<String> = Vec::new();
        let mut placed: HashSet<String> = HashSet::new();
        for num in current {
            if placed.insert(num.clone()) {
                order.push(num.clone());
            }
        }
        for num in std::mem::take(&mut self.seen) {
            if placed.contains(&num) {
                continue;
            }
            let at = self
                .after
                .get(&num)
                .and_then(Option::as_ref)
                .and_then(|prev| order.iter().position(|n| n == prev))
                .map_or(order.len(), |i| i + 1);
            order.insert(at, num.clone());
            placed.insert(num);
        }
        order
            .into_iter()
            .filter_map(|num| {
                let h = self.by_num.remove(&num)?;
                if h.forms.is_empty() {
                    return None;
                }
                let versions = h
                    .forms
                    .into_iter()
                    .map(|f| CorpusVersion {
                        date_debut: f.start.map(|d| d.to_string()),
                        date_fin: f.end.map(|d| d.to_string()),
                        status: article_status(&f.texte).to_string(),
                        texte: f.texte,
                        source: None,
                        source_url: None,
                        source_asof: None,
                        source_upstream_url: None,
                    })
                    .collect();
                Some(CorpusArticle {
                    num,
                    num_key: None,
                    title_path: h.title_path,
                    nota: None,
                    texte: None,
                    texte_original: None,
                    lang_original: None,
                    translation: None,
                    versions: Some(versions),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn art(num: &str, texte: &str, amended: &[&str]) -> Article {
        Article {
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            amended: amended.iter().map(|d| day(d)).collect(),
        }
    }

    fn windows(a: &CorpusArticle) -> Vec<(Option<&str>, Option<&str>, &str)> {
        a.versions
            .as_ref()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v.date_debut.as_deref(),
                    v.date_fin.as_deref(),
                    v.texte.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn a_book_keeps_its_own_articles_under_its_label() {
        let book = Book {
            prefix: "2.3.",
            label: "LIVRE 2, TITRE 3. - Les relations patrimoniales des couples",
        };
        let page = |num: &str, path: &str| lj_parse::JustelArticle {
            num: num.to_string(),
            title_path: Some(path.to_string()),
            texte: "Texte.".to_string(),
            amended: Vec::new(),
        };
        let kept = articles(
            vec![
                page("2", "CHAPITRE 2. - Modifications du Code civil"),
                page(
                    "2.3.1",
                    "Titre 3. Les relations patrimoniales des couples > Chapitre 1er. Conventions",
                ),
            ],
            false,
            Some(&book),
            &mut Spellings::default(),
        )
        .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].title_path.as_deref(),
            Some("LIVRE 2, TITRE 3. - Les relations patrimoniales des couples > Chapitre 1er. Conventions")
        );
    }

    #[test]
    fn a_suffix_is_spaced_only_after_a_bare_number() {
        assert_eq!(spaced("7bis"), "7 bis");
        assert_eq!(spaced("120quinquies/1"), "120quinquies/1");
        assert_eq!(spaced("1er"), "1er");
        assert_eq!(spaced("I.9"), "I.9");
    }

    /// Two amendments changed article 41, each shown alone in the version it closed; a later whole
    /// version and the current page date article 42's insertion by its absence before.
    #[test]
    fn each_archived_version_dates_the_form_after_it() {
        let current = [
            art("41", "C", &["1999-04-19"]),
            art("42", "N", &["2014-05-25"]),
        ];
        let mut doc = Document::new(&current, None);
        let mut h = Histories::default();
        h.observe(
            &mut doc,
            &[art("41", "A", &[])],
            Some(day("1997-04-12")),
            false,
        );
        h.observe(
            &mut doc,
            &[art("41", "B", &["1997-04-12"])],
            Some(day("1999-04-19")),
            false,
        );
        h.observe(
            &mut doc,
            &[art("41", "C", &["1999-04-19"])],
            Some(day("2014-05-25")),
            true,
        );
        h.observe(&mut doc, &current, None, true);
        let out = h.into_articles(&["41".to_string(), "42".to_string()]);
        assert_eq!(
            windows(&out[0]),
            [
                (None, Some("1997-04-12"), "A"),
                (Some("1997-04-12"), Some("1999-04-19"), "B"),
                (Some("1999-04-19"), None, "C"),
            ]
        );
        assert_eq!(windows(&out[1]), [(Some("2014-05-25"), None, "N")]);
    }

    /// An amendment published later but in force earlier gives a version that never applied.
    #[test]
    fn a_version_whose_window_closed_before_it_opened_never_applied() {
        let current = [art("1", "C", &[])];
        let mut doc = Document::new(&current, Some(day("1994-02-27")));
        let mut h = Histories::default();
        h.observe(
            &mut doc,
            &[art("1", "A", &[])],
            Some(day("2009-01-01")),
            true,
        );
        h.observe(
            &mut doc,
            &[art("1", "B", &[])],
            Some(day("2005-09-01")),
            true,
        );
        h.observe(&mut doc, &current, None, true);
        let out = h.into_articles(&["1".to_string()]);
        assert_eq!(
            windows(&out[0]),
            [
                (Some("1994-02-27"), Some("2009-01-01"), "A"),
                (Some("2009-01-01"), None, "C"),
            ]
        );
    }

    /// A run that reads the versions archived since its last visit continues the stored history.
    #[test]
    fn a_new_archive_closes_the_form_the_base_holds_open() {
        let stored = [
            StoredArticleVersion {
                num: "5".to_string(),
                position: Some(0),
                title_path: None,
                nota: None,
                texte: "A".to_string(),
                date_debut: day("0001-01-01"),
                date_fin: Some(day("2020-01-01")),
            },
            StoredArticleVersion {
                num: "5".to_string(),
                position: Some(0),
                title_path: None,
                nota: None,
                texte: "B".to_string(),
                date_debut: day("2020-01-01"),
                date_fin: None,
            },
        ];
        let current = [
            art("5", "C", &["2026-07-24"]),
            art("5/1", "D", &["2026-07-24"]),
        ];
        let mut doc = Document::new(&current, None);
        let mut h = Histories::from_stored(&stored, &mut Spellings::default());
        h.observe(
            &mut doc,
            &[art("5", "B", &[])],
            Some(day("2026-07-24")),
            true,
        );
        h.observe(&mut doc, &current, None, true);
        let out = h.into_articles(&["5".to_string(), "5/1".to_string()]);
        assert_eq!(
            windows(&out[0]),
            [
                (None, Some("2020-01-01"), "A"),
                (Some("2020-01-01"), Some("2026-07-24"), "B"),
                (Some("2026-07-24"), None, "C"),
            ]
        );
        assert_eq!(windows(&out[1]), [(Some("2026-07-24"), None, "D")]);
    }

    #[test]
    fn a_repealed_document_closes_its_open_articles() {
        let current = [art("1", "A", &[]), art("2", "(Abrogé)", &["2005-04-01"])];
        let mut doc = Document::new(&current, None);
        let mut h = Histories::default();
        h.observe(&mut doc, &current, None, true);
        h.repeal(&doc, day("2026-09-01"));
        let out = h.into_articles(&["1".to_string(), "2".to_string()]);
        assert_eq!(windows(&out[0]), [(None, Some("2026-09-01"), "A")]);
        assert_eq!(
            windows(&out[1]),
            [(Some("2005-04-01"), Some("2026-09-01"), "(Abrogé)")]
        );
    }

    /// Justel's archives write with a dot an article its current page numbers with a hyphen.
    #[test]
    fn two_spellings_of_one_number_are_one_article() {
        let page = |num: &str, texte: &str| lj_parse::JustelArticle {
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            amended: Vec::new(),
        };
        let mut spellings = Spellings::default();
        let current = articles(vec![page("346-2", "C")], false, None, &mut spellings).unwrap();
        let archived = articles(vec![page("346.2", "A")], false, None, &mut spellings).unwrap();
        let mut doc = Document::new(&current, None);
        let mut h = Histories::default();
        h.observe(&mut doc, &archived, Some(day("2012-08-20")), true);
        h.observe(&mut doc, &current, None, true);
        let out = h.into_articles(&["346-2".to_string()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].num, "346-2");
        assert_eq!(
            windows(&out[0]),
            [
                (None, Some("2012-08-20"), "A"),
                (Some("2012-08-20"), None, "C")
            ]
        );
    }
}
