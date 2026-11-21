//! A codified text's page on an Irosoft LIMS site, LégisQuébec's or New Brunswick's.

use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{Child, ConsolidatedArticle, Lines, Tree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimsDocument {
    /// A New Brunswick regulation's title is its subject alone: `Général`.
    pub title: String,
    /// A regulation's enabling act.
    pub enabling: Option<String>,
    /// The day an act was assented to or a regulation filed, ISO.
    pub enacted: Option<String>,
    /// The day the consolidation is current to, ISO.
    pub asof: Option<String>,
    pub repealed: bool,
    /// The day a repealed text stopped applying, ISO, when the page's language reads it.
    pub ended: Option<String>,
    /// Each article under its code at the site, `se:1_1` or `sc-nb:1`, the same in both languages.
    pub articles: Vec<(String, ConsolidatedArticle)>,
}

const FRAGMENT: &str = r#"<div xmlns="http://www.w3.org/1999/xhtml""#;

static DIV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(/?)div\b[^>]*?(/?)>").unwrap());
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<meta name="dc\.title" content="\s*-?\s*([^"]*)""#).unwrap());
static BARE_AMPERSAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&((?:[A-Za-z]+|#[0-9]+|#x[0-9A-Fa-f]+);)?").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static DAY: &str = r"(\d{1,2}(?:er)?\s+\p{L}+\s+\d{4})";
/// New Brunswick's note, `La présente loi est refondue au 12 juin 2026.`, then LégisQuébec's header.
static ASOF: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(&format!(r"refondue? au\s+{DAY}")).unwrap(),
        Regex::new(&format!(r"À jour au\s+{DAY}")).unwrap(),
    ]
});
static ENACTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:Sanctionnée|Déposé) le\s+{DAY}")).unwrap());

/// The embedded document, from its opening `div` to the one closing it.
fn fragment(html: &str) -> Result<&str, String> {
    let start = html.find(FRAGMENT).ok_or("no embedded document")?;
    let mut depth = 0usize;
    for m in DIV.captures_iter(&html[start..]) {
        if !m[2].is_empty() {
            continue;
        }
        if m[1].is_empty() {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Ok(&html[start..start + m.get(0).expect("a match").end()]);
            }
        }
    }
    Err("the embedded document never closes".to_string())
}

/// LégisQuébec leaves `&` bare in its links' query strings.
fn escape_ampersands(xml: &str) -> std::borrow::Cow<'_, str> {
    if !xml.contains('&') {
        return xml.into();
    }
    BARE_AMPERSAND
        .replace_all(xml, |c: &regex::Captures| match c.get(1) {
            None => "&amp;".to_string(),
            Some(reference) => format!("&{}", reference.as_str()),
        })
        .into_owned()
        .into()
}

fn iso(day: &str) -> Option<String> {
    lj_core::dates::parse_date_fr(day).map(|d| d.to_string())
}

struct Page<'a> {
    tree: &'a Tree,
}

impl Page<'_> {
    fn class(&self, idx: usize) -> &str {
        self.tree.attr(idx, "class").unwrap_or_default()
    }

    fn id(&self, idx: usize) -> &str {
        self.tree.attr(idx, "id").unwrap_or_default()
    }

    fn nodes(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        self.tree.nodes[idx]
            .children
            .iter()
            .filter_map(|c| match c {
                Child::Node(i) => Some(*i),
                Child::Text(_) => None,
            })
    }

    /// The navigation a page lays over its text: history links, language switches, hidden copies.
    fn chrome(&self, idx: usize) -> bool {
        let class = self.class(idx);
        self.tree.nodes[idx].tag == "img"
            || class.contains("HistoryLink")
            || class.contains("linkOtherLang")
            || class.split_whitespace().any(|c| c == "Hidden")
    }

    fn note(&self, idx: usize) -> bool {
        self.class(idx).to_lowercase().contains("historicalnote")
    }

    fn collect(&self, idx: usize, skip: Option<usize>, out: &mut String) {
        for c in &self.tree.nodes[idx].children {
            match c {
                Child::Text(t) => out.push_str(t),
                Child::Node(i) if Some(*i) != skip && !self.chrome(*i) && !self.note(*i) => {
                    if self.tree.nodes[*i].tag == "br" {
                        out.push(' ');
                    }
                    self.collect(*i, skip, out);
                }
                Child::Node(_) => {}
            }
        }
    }

    /// The text under `idx` without its chrome, its history notes nor `skip`, whitespace folded.
    fn text(&self, idx: usize, skip: Option<usize>) -> String {
        let mut out = String::new();
        self.collect(idx, skip, &mut out);
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn each(&self, idx: usize, visit: &mut dyn FnMut(usize)) {
        for i in self.nodes(idx).collect::<Vec<_>>() {
            visit(i);
            self.each(i, visit);
        }
    }

    fn find(&self, idx: usize, hit: &dyn Fn(usize) -> bool) -> Option<usize> {
        self.nodes(idx)
            .find_map(|i| if hit(i) { Some(i) } else { self.find(i, hit) })
    }

    /// `CHAPITRE I - APPLICATION ET INTERPRÉTATION`, `II - CHAMP D’APPLICATION`.
    fn heading(&self, idx: usize) -> String {
        let id = self.id(idx);
        let title = self.find(idx, &|i| {
            self.id(i).starts_with(&format!("{id}-t1")) || self.class(i).starts_with("TitleText")
        });
        let label = self
            .find(idx, &|i| self.class(i).to_lowercase().starts_with("label"))
            .map_or_else(|| self.text(idx, title), |l| self.text(l, None));
        match title.map(|t| self.text(t, None)).filter(|t| !t.is_empty()) {
            None => label,
            Some(title) if label.is_empty() => title,
            Some(title) if label.ends_with(['\u{2014}', '-']) => format!("{label} {title}"),
            Some(title) => format!("{label} - {title}"),
        }
    }

    /// One line per block, a table one line per row, its cells apart; history notes go to `nota`.
    fn lay_out(&self, idx: usize, skip: Option<usize>, lines: &mut Lines, nota: &mut Vec<String>) {
        for c in &self.tree.nodes[idx].children {
            let i = match c {
                Child::Text(t) => {
                    lines.push(t);
                    continue;
                }
                Child::Node(i) => *i,
            };
            if self.chrome(i) || Some(i) == skip {
                continue;
            }
            if self.note(i) {
                nota.push(self.text(i, None));
                continue;
            }
            match self.tree.nodes[i].tag.as_str() {
                "tr" => {
                    let cells: Vec<String> = self
                        .nodes(i)
                        .map(|cell| self.text(cell, None))
                        .filter(|cell| !cell.is_empty())
                        .collect();
                    lines.line_break();
                    lines.push(&cells.join(" | "));
                    lines.line_break();
                }
                "div" | "p" | "table" | "tbody" | "br" | "li" => {
                    lines.line_break();
                    self.lay_out(i, skip, lines, nota);
                    lines.line_break();
                }
                _ => self.lay_out(i, skip, lines, nota),
            }
        }
    }

    fn article(
        &self,
        idx: usize,
        (num, skip): (String, Option<usize>),
        headings: &[String],
    ) -> ConsolidatedArticle {
        let (mut lines, mut nota) = (Lines::new(|line| line), Vec::new());
        self.lay_out(idx, skip, &mut lines, &mut nota);
        ConsolidatedArticle {
            num,
            title_path: (!headings.is_empty()).then(|| headings.join(" > ")),
            texte: lines.finish().join("\n"),
            nota: (!nota.is_empty()).then(|| nota.join("\n")),
        }
    }

    /// `ANNEXE ABROGATIVE` reads `Annexe abrogative`, `ANNEXE IV` reads `Annexe IV`.
    fn schedule_num(&self, idx: usize) -> Option<(usize, String)> {
        let node = self.find(idx, &|i| {
            self.class(i).to_lowercase().starts_with("label-schedule")
        })?;
        let label = self.text(node, None);
        let words: Vec<String> = label
            .split(' ')
            .enumerate()
            .map(|(n, word)| {
                let numeral = word.chars().all(|c| "IVXLC".contains(c))
                    || word.chars().any(|c| c.is_ascii_digit())
                    || word.chars().count() == 1;
                match (n, numeral) {
                    (_, true) => word.to_string(),
                    (0, false) => {
                        let lower = word.to_lowercase();
                        let mut chars = lower.chars();
                        chars.next().map_or_else(String::new, |first| {
                            first.to_uppercase().chain(chars).collect()
                        })
                    }
                    (_, false) => word.to_lowercase(),
                }
            })
            .collect();
        Some((node, words.join(" ")))
    }

    /// A heading's depth is its id's, `ga:l_iii-gb:s_9-h1` two deep, wherever the page lays it.
    fn walk(
        &self,
        idx: usize,
        headings: &mut Vec<String>,
        articles: &mut Vec<(String, ConsolidatedArticle)>,
    ) -> Result<(), String> {
        for i in self.nodes(idx).collect::<Vec<_>>() {
            let id = self.id(i);
            if let Some(path) = id.strip_prefix("ga:").and_then(|p| p.strip_suffix("-h1")) {
                headings.truncate(path.split('-').count() - 1);
                headings.push(self.heading(i));
            } else if let Some(num) = id.strip_prefix("se:").filter(|n| !n.contains('-')) {
                let article = self.article(i, (num.replace('_', "."), None), headings);
                articles.push((id.to_string(), article));
            } else if id.starts_with("sc-") {
                let (label, num) = self
                    .schedule_num(i)
                    .ok_or_else(|| format!("schedule {id} without its label"))?;
                headings.clear();
                articles.push((
                    id.to_string(),
                    self.article(i, (num, Some(label)), headings),
                ));
            } else if !id.starts_with("na-") && !id.starts_with("nk-") {
                self.walk(i, headings, articles)?;
            }
        }
        Ok(())
    }

    /// LégisQuébec's identification: its days, an act's assent first, and whether it was repealed,
    /// the last day then being the repeal's.
    fn identification(&self) -> (Vec<String>, bool) {
        let Some(id) = self.find(0, &|i| self.class(i) == "Identification") else {
            return (Vec::new(), false);
        };
        let mut groups: Vec<usize> = Vec::new();
        self.each(id, &mut |i| {
            let parent = self.tree.nodes[i].parent;
            if self.class(i).contains("date-txt-statut") && !groups.contains(&parent) {
                groups.push(parent);
            }
        });
        let days = groups
            .into_iter()
            .filter_map(|g| iso(&self.text(g, None)))
            .collect();
        let repealed = self.nodes(id).any(|i| self.class(i) == "Repealed");
        (days, repealed)
    }

    fn enabling(&self) -> Option<String> {
        if let Some(i) = self.find(0, &|i| self.class(i).contains("LongTitle-enabling")) {
            return Some(self.text(i, None));
        }
        let block = self.find(0, &|i| self.class(i) == "regulation-id-enabling")?;
        let act = self.find(block, &|i| self.class(i) == "Border")?;
        let name = self.find(act, &|i| self.class(i) == "mt-12pt")?;
        Some(self.text(name, None)).filter(|n| !n.is_empty())
    }
}

/// A text's page: its document's sections and schedules in page order, under their headings.
pub fn lims_document(html: &str) -> Result<LimsDocument, String> {
    let title = TITLE
        .captures(html)
        .map(|c| html_escape::decode_html_entities(c[1].trim()).into_owned())
        .filter(|t| !t.is_empty())
        .ok_or("no title")?;
    let tree = Tree::parse(&escape_ampersands(fragment(html)?))?;
    let page = Page { tree: &tree };
    let (mut headings, mut articles) = (Vec::new(), Vec::new());
    page.walk(0, &mut headings, &mut articles)?;
    let mut seen = std::collections::HashSet::new();
    if let Some((_, a)) = articles.iter().find(|(_, a)| !seen.insert(a.num.as_str())) {
        return Err(format!("article {} twice", a.num));
    }
    let plain = TAG.replace_all(html, "");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let (days, repealed) = page.identification();
    let enacted = match ENACTED.captures(&plain) {
        Some(c) => iso(&c[1]),
        None => days.first().filter(|_| !repealed).cloned(),
    };
    Ok(LimsDocument {
        title,
        enabling: page.enabling(),
        enacted,
        repealed,
        ended: days.last().filter(|_| repealed).cloned(),
        asof: ASOF
            .iter()
            .find_map(|re| re.captures(&plain))
            .and_then(|c| iso(&c[1])),
        articles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEBEC: &str = r##"<html><head><meta name="dc.title" content=" - Loi sur l’accès aux documents"></head><body>
<div class="text-end">À jour au 15 octobre 2026</div>
<?xml version="1.0" encoding="UTF-8"?>
<div xmlns="http://www.w3.org/1999/xhtml" xmlns:integrity="http://ref.irosoft.com/integrity/"><div class="Identification"><div class="Identification-Id"><span>chapitre </span><span>A-2.1</span></div><span class="Hidden"><span class="date-txt-statut">23<span> </span></span><span class="Hidden">06</span><span>juin </span><span class="date-txt-statut">1982</span></span></div>
<div id="ga:l_i"><div class="Heading Heading" id="ga:l_i-h1"><div class="Label-group4"><span>CHAPITRE </span>I</div><div class="TitleText-group4 group4" id="ga:l_i-h1-t1-nb:1">APPLICATION</div></div>
<div class="section" id="se:1_1"><div class="Subsection SubsectionFirst" id="se:1_1-ss:1"><div class="HistoryLink"><a href="/fr/version/lc/a-2.1?code=se:1_1"><img src="/img/history.png"/></a></div><a class="linkOtherLang" href="#se:1_1"><span class="Label-Section">1.1<span>.</span></span></a> <span class="Subsection">La présente loi s’applique:</span><div class="Paragraph" id="se:1_1-ss:1-p1:1"><span class="Label-Z LabelBase">1°</span> aux documents;</div></div><div class="HistoricalNote"><div class="ligne"> </div><span><span class="Hidden">2006, c. 22</span><span>2006, c. 22, a. 1</span><span>.</span></span></div></div>
<div id="ga:l_i-gb:l_i"><div class="Heading Heading" id="ga:l_i-gb:l_i-h1"><span class="Label-group6"><span>§ </span>1<span>. &#8212; </span></span><span class="TitleText-group6 group6">Relations</span></div>
<div class="section" id="se:2"><div class="Subsection SubsectionFirst" id="se:2-ss:1"><span class="Subsection Repealed-Text">(Abrogé).</span></div></div></div></div>
<div class="Schedule-abrogative" id="sc-nb:1"><div class="ScheduleHeading"><a class="linkOtherLang" href="#sc-nb:1"><span class="Label-ScheduleHeading">ANNEXE A</span></a></div><div class="ScheduleContent"><table><tbody><tr><td>Loi</td><td>Articles 1 à 3</td></tr></tbody></table></div></div>
<div class="note-block" id="nk-nb:1"><div>DISPOSITIONS TRANSITOIRES</div></div>
</div><div class="footer">Fin</div></body></html>"##;

    const NEW_BRUNSWICK: &str = r##"<html><head><meta name="dc.title" content=" - Général"></head><body>
<div class="text-end d-none">À jour au 1<sup>er</sup> janvier 2024</div>
<div xmlns="http://www.w3.org/1999/xhtml"><div class="regulation-id"><div>RÈGLEMENT DU</div><div class="long-title-regulation">NOUVEAU-BRUNSWICK 84-20</div></div><div class="regulation-id-enabling"><div class="mt-12pt">pris en vertu de la</div><div class="Border"><div class="mt-12pt">Loi sur le paiement <br/>des services médicaux</div>(D.C. 84-64)</div></div><div class="regulation-date"><div class="Border" id="id"><div class="body right italic mb-12pt">Déposé le 13 février 1984</div></div></div>
<div class="Border" id="ga:l_ii-h1"><a class="partitions"/><span class="label disp-block"><div class="head-5 bold">II</div></span><div class="head-5" id="ga:l_ii-h1-t1-nb:1">CHAMP D’APPLICATION</div></div>
<div class="Border" id="se:2"><a class="partitions"/><div class="body Section"><div class="body section"><div class="Border head-3 sectionheading" id="se:2-sh">Application</div><span class="label "><span class="label bold mr-20pt"><a class="linkOtherLang" href="#" data-code="se:2"> 2</a></span>Le présent règlement s’applique</span><div class="Border" id="se:2-p1:a"><div class="body Paragraph"><div class="indent-1-1 paragraph"><span class="label"><span class=""><span class="text-normal">
</span>a<span class="text-normal">)
</span></span></span>aux médecins;</div></div></div><div class="body historicalnote">84-78; 2014-87</div></div></div></div>
<div class="Border" id="ga:l_ii-gb:s_3-h1"><div class="head-5" id="ga:l_ii-gb:s_3-h1-t1-nb:1">Frais</div></div>
<div class="Border" id="se:3"><div class="body Section"><div class="body section"><span class="label "><span class="label bold mr-20pt"><a class="linkOtherLang" href="#" data-code="se:3">3</a></span><span class="repealed-parent-text">Abrogé : 2014-87</span></span></div></div></div>
<div class="Border" id="ga:l_iii-h1"><span class="label disp-block"><div class="head-5 bold">III</div></span><div class="head-5" id="ga:l_iii-h1-t1-nb:1">FIN</div></div>
<div class="Border" id="se:4"><div class="body Section"><div class="body section">Le présent règlement entre en vigueur le 1<span class="Superscript">er</span> avril 1984.</div></div></div>
<div class="Border" id="sc-nb:1"><a class="partitions"/><div class="head-1"><span class="label"><div class="label-schedule-heading">ANNEXE 0.1</div></span><div class="title-text-schedule">FORMULE</div><div>Abrogé : 2014-87</div><div class="body historicalnote">2014-87</div></div></div>
<div class="Border" id="na-nb:1"><div class="body notabene"><span class="bold">N.B. </span><span class="text-notabene">Le présent règlement est refondu au 15 septembre 2026.</span></div></div>
</div></body></html>"##;

    fn nums(doc: &LimsDocument) -> Vec<&str> {
        doc.articles.iter().map(|(_, a)| a.num.as_str()).collect()
    }

    fn article(doc: &LimsDocument, n: usize) -> &ConsolidatedArticle {
        &doc.articles[n].1
    }

    #[test]
    fn a_quebec_page_reads_as_its_sections_under_their_headings_its_schedule_one_article() {
        let doc = lims_document(QUEBEC).unwrap();
        assert_eq!(doc.title, "Loi sur l’accès aux documents");
        assert_eq!(doc.enacted.as_deref(), Some("1982-06-23"));
        assert_eq!(doc.asof.as_deref(), Some("2026-10-15"));
        assert_eq!(doc.enabling, None);
        assert!(!doc.repealed);
        assert_eq!(nums(&doc), ["1.1", "2", "Annexe A"]);
        assert_eq!(doc.articles[2].0, "sc-nb:1");
        let first = article(&doc, 0);
        assert_eq!(
            first.title_path.as_deref(),
            Some("CHAPITRE I - APPLICATION")
        );
        assert_eq!(
            first.texte,
            "La présente loi s’applique:\n1° aux documents;"
        );
        assert_eq!(first.nota.as_deref(), Some("2006, c. 22, a. 1."));
        let repealed = article(&doc, 1);
        assert_eq!(
            repealed.title_path.as_deref(),
            Some("CHAPITRE I - APPLICATION > § 1. \u{2014} Relations")
        );
        assert_eq!(repealed.texte, "(Abrogé).");
        assert_eq!(article(&doc, 2).texte, "Loi | Articles 1 à 3");
        assert_eq!(article(&doc, 2).title_path, None);
    }

    #[test]
    fn a_repealed_quebec_text_ends_on_its_identification_s_last_day() {
        let page = QUEBEC.replace(
            r#"<span class="date-txt-statut">1982</span></span></div>"#,
            r#"<span class="date-txt-statut">1982</span></span><span class="Hidden"><span class="date-txt-statut">15 </span>novembre <span class="date-txt-statut">2000</span></span><div class="Repealed">Abrogée, 2000, c. 40, a. 43.</div></div>"#,
        );
        let doc = lims_document(&page).unwrap();
        assert!(doc.repealed);
        assert_eq!(doc.ended.as_deref(), Some("2000-11-15"));
        assert_eq!(doc.enacted, None);
    }

    #[test]
    fn a_new_brunswick_page_takes_its_headings_beside_its_sections_by_their_depth() {
        let doc = lims_document(NEW_BRUNSWICK).unwrap();
        assert_eq!(doc.title, "Général");
        assert_eq!(
            doc.enabling.as_deref(),
            Some("Loi sur le paiement des services médicaux")
        );
        assert_eq!(doc.enacted.as_deref(), Some("1984-02-13"));
        assert_eq!(doc.asof.as_deref(), Some("2026-09-15"));
        assert_eq!(nums(&doc), ["2", "3", "4", "Annexe 0.1"]);
        let first = article(&doc, 0);
        assert_eq!(
            first.title_path.as_deref(),
            Some("II - CHAMP D’APPLICATION")
        );
        assert_eq!(
            first.texte,
            "Application\nLe présent règlement s’applique\na) aux médecins;"
        );
        assert_eq!(first.nota.as_deref(), Some("84-78; 2014-87"));
        assert_eq!(
            article(&doc, 1).title_path.as_deref(),
            Some("II - CHAMP D’APPLICATION > Frais")
        );
        assert_eq!(article(&doc, 1).texte, "Abrogé : 2014-87");
        assert_eq!(article(&doc, 2).title_path.as_deref(), Some("III - FIN"));
        assert_eq!(
            article(&doc, 2).texte,
            "Le présent règlement entre en vigueur le 1er avril 1984."
        );
        assert_eq!(article(&doc, 3).texte, "FORMULE\nAbrogé : 2014-87");
        assert_eq!(article(&doc, 3).nota.as_deref(), Some("2014-87"));
    }
}
