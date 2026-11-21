//! Justice Laws' XML: the sections of one point-in-time state of a Canadian federal Act or
//! regulation, in either official language, and the index that names them all.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{Child, ConsolidatedArticle, Lines, Tree};

/// The provisions printed as a numbered line of their own: `(1)`, `a)`, `(i)`, `(A)`, `(I)`.
const PROVISIONS: &[&str] = &[
    "Section",
    "Subsection",
    "Paragraph",
    "Subparagraph",
    "Clause",
    "Subclause",
    "Definition",
    "Provision",
    "Oath",
];

/// The text of a number the Act never used: `[Pas d’articles 672.65 et 672.66]`,
/// `[No sections 672.65 and 672.66]`.
const NO_SECTION: [&str; 2] = ["[Pas d", "[No section"];

/// A placeholder for repealed sections: `157 et 158`, `594 à 596`, `6 and 7`, `3 to 9`.
static RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+) (et|à|and|to) (\S+)$").unwrap());

/// The key of the one article an instrument without sections is read as.
const WHOLE: &str = "unique";

/// The number of recitals coded as a section, in both languages.
const PREAMBLE: &str = "Préambule";

fn same(line: String) -> String {
    line
}

/// An `à` or `to` range is enumerated only when its ends differ in their last integer part alone.
fn covered(first: &str, link: &str, last: &str) -> Vec<String> {
    if link == "et" || link == "and" {
        return vec![first.to_string(), last.to_string()];
    }
    let ((p1, l1), (p2, l2)) = (
        first.rsplit_once('.').unwrap_or(("", first)),
        last.rsplit_once('.').unwrap_or(("", last)),
    );
    match (p1 == p2, l1.parse::<u32>(), l2.parse::<u32>()) {
        (true, Ok(a), Ok(b)) if a < b => (a..=b)
            .map(|n| {
                if p1.is_empty() {
                    n.to_string()
                } else {
                    format!("{p1}.{n}")
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

struct Render<'a> {
    tree: &'a Tree,
    nota: Vec<String>,
}

impl Render<'_> {
    fn walk(&mut self, idx: usize, out: &mut Lines) {
        for child in &self.tree.nodes[idx].children {
            match child {
                Child::Text(t) => out.push(t),
                Child::Node(i) => self.node(*i, out),
            }
        }
    }

    fn inline(&mut self, idx: usize) -> String {
        let mut lines = Lines::new(same);
        self.walk(idx, &mut lines);
        lines.finish().join(" ")
    }

    fn node(&mut self, i: usize, out: &mut Lines) {
        let tag = self.tree.nodes[i].tag.as_str();
        match tag {
            "HistoricalNote" => {
                let items: Vec<String> = self
                    .tree
                    .children(i, "HistoricalNoteSubItem")
                    .map(|s| self.inline(s))
                    .filter(|s| !s.is_empty())
                    .collect();
                if !items.is_empty() {
                    self.nota.push(items.join("; "));
                }
            }
            "Footnote" | "Note" => {
                let note = self.inline(i);
                if !note.is_empty() {
                    self.nota.push(note);
                }
            }
            "FootnoteRef" | "Label" => {}
            "Repealed" => {
                out.push(" ");
                self.walk(i, out);
            }
            "Sup" => {
                out.glue();
                self.walk(i, out);
            }
            "MarginalNote" | "row" => {
                out.line_break();
                self.walk(i, out);
                out.line_break();
            }
            "entry" => {
                out.push(" ");
                self.walk(i, out);
                out.push(" ");
            }
            t if PROVISIONS.contains(&t) => {
                out.line_break();
                for child in &self.tree.nodes[i].children {
                    match child {
                        Child::Node(c) if self.tree.nodes[*c].tag == "Label" => {
                            let label = self.inline(*c);
                            if !label.is_empty() {
                                out.mark(&label);
                            }
                        }
                        Child::Node(c) => self.node(*c, out),
                        Child::Text(t) => out.push(t),
                    }
                }
            }
            t if t.starts_with("Continued") => {
                out.line_break();
                self.walk(i, out);
            }
            _ => self.walk(i, out),
        }
    }

    /// The marginal note heads the text; the section's own number stays out of it.
    fn section(&mut self, idx: usize) -> (String, Option<String>) {
        self.nota.clear();
        let mut lines = Lines::new(same);
        for child in &self.tree.nodes[idx].children {
            match child {
                Child::Node(c) if self.tree.nodes[*c].tag == "Label" => {}
                Child::Node(c) => self.node(*c, &mut lines),
                Child::Text(t) => lines.push(t),
            }
        }
        let nota = std::mem::take(&mut self.nota);
        (
            lines.finish().join("\n"),
            (!nota.is_empty()).then(|| nota.join("\n")),
        )
    }
}

/// The sections of the body under their headings; a repealed range yields each section it covers
/// that does not stand alone, a number two sections bear holds both, and a section without a
/// number goes on the one before it. An instrument with no
/// section is one article: its repeal note when the whole of it is repealed, its order, else its
/// schedules.
pub fn justice_laws_articles(xml: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let tree = Tree::parse(xml.trim_start_matches('\u{feff}'))?;
    let root = tree.nodes[0]
        .children
        .iter()
        .find_map(|c| match c {
            Child::Node(i) => Some(*i),
            Child::Text(_) => None,
        })
        .ok_or("no root element")?;
    let mut render = Render {
        tree: &tree,
        nota: Vec::new(),
    };
    let body = tree
        .child(root, "Body")
        .filter(|&b| tree.child(b, "Section").is_some());
    let Some(body) = body else {
        let whole: Vec<usize> = match tree
            .child(root, "Repealed")
            .or_else(|| tree.child(root, "Order"))
        {
            Some(w) => vec![w],
            None => tree.children(root, "Schedule").collect(),
        };
        let (texts, notas): (Vec<String>, Vec<Option<String>>) =
            whole.into_iter().map(|w| render.section(w)).unzip();
        let texte = texts
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let nota = Some(notas.into_iter().flatten().collect::<Vec<_>>().join("\n"))
            .filter(|n| !n.is_empty());
        if texte.is_empty() {
            return Ok(Vec::new());
        }
        return Ok(vec![ConsolidatedArticle {
            num: WHOLE.to_string(),
            title_path: None,
            texte,
            nota,
        }]);
    };
    let labels: Vec<String> = tree
        .children(body, "Section")
        .map(|s| {
            tree.child(s, "Label")
                .map(|l| render.inline(l))
                .unwrap_or_default()
        })
        .collect();
    let solo: HashSet<&str> = labels
        .iter()
        .map(String::as_str)
        .filter(|l| !RANGE.is_match(l))
        .collect();

    let mut out: Vec<ConsolidatedArticle> = Vec::new();
    let mut seen = HashSet::new();
    let mut previous: Option<String> = None;
    let mut headings: Vec<(u32, String)> = Vec::new();
    for child in &tree.nodes[body].children {
        let Child::Node(idx) = child else { continue };
        let idx = *idx;
        match tree.nodes[idx].tag.as_str() {
            "Heading" => {
                let level: u32 = tree
                    .attr(idx, "level")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(1);
                let label = tree.child(idx, "Label").map(|l| render.inline(l));
                let title = tree.child(idx, "TitleText").map(|t| render.inline(t));
                let text = match (
                    label.filter(|l| !l.is_empty()),
                    title.filter(|t| !t.is_empty()),
                ) {
                    (Some(l), Some(t)) => format!("{l} - {t}"),
                    (Some(l), None) => l,
                    (None, Some(t)) => t,
                    (None, None) => continue,
                };
                headings.retain(|(l, _)| *l < level);
                headings.push((level, text.replace('>', "›")));
            }
            "Section" => {
                let label = match tree.child(idx, "Label").map(|l| render.inline(l)) {
                    Some(l) if l == "Preamble" => PREAMBLE.to_string(),
                    Some(l) if !l.is_empty() => l,
                    _ => previous.clone().unwrap_or_else(|| PREAMBLE.to_string()),
                };
                previous = Some(label.clone());
                let (texte, mut nota) = render.section(idx);
                if texte.is_empty() || NO_SECTION.iter().any(|n| texte.starts_with(n)) {
                    continue;
                }
                let nums = match RANGE.captures(&label) {
                    None => vec![label.clone()],
                    Some(r) => {
                        let nums: Vec<String> = covered(&r[1], &r[2], &r[3])
                            .into_iter()
                            .filter(|n| !solo.contains(n.as_str()))
                            .collect();
                        if nums.is_empty() {
                            nota = Some(match nota {
                                Some(n) => format!("{label}\n{n}"),
                                None => label.clone(),
                            });
                            vec![r[1].to_string()]
                        } else {
                            nums
                        }
                    }
                };
                let title_path = (!headings.is_empty()).then(|| {
                    headings
                        .iter()
                        .map(|(_, h)| h.as_str())
                        .collect::<Vec<_>>()
                        .join(" > ")
                });
                for num in nums {
                    if !seen.insert(num.clone()) {
                        let first = out
                            .iter_mut()
                            .find(|a| a.num == num)
                            .expect("a number seen is an article");
                        first.texte = format!("{}\n{texte}", first.texte);
                        first.nota = match (first.nota.take(), nota.clone()) {
                            (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
                            (a, b) => a.or(b),
                        };
                        continue;
                    }
                    out.push(ConsolidatedArticle {
                        num,
                        title_path: title_path.clone(),
                        texte: texte.clone(),
                        nota: nota.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Whether an instrument of the index is an Act or a regulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JusticeLawsKind {
    Act,
    Regulation,
}

/// An instrument in one language, as the index names it: `C.R.C., ch. 669`, `2019, ch. 10`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JusticeLawsEntry {
    pub unique_id: String,
    pub official_number: Option<String>,
    pub title: String,
}

/// An instrument of the index, its French entry and the English one it pairs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JusticeLawsInstrument {
    pub kind: JusticeLawsKind,
    pub fr: JusticeLawsEntry,
    pub en: Option<JusticeLawsEntry>,
}

/// The instruments of `Legis.xml`. An Act pairs its languages on its chapter, a regulation on
/// the identifier each language's entry gives the other.
pub fn justice_laws_index(xml: &str) -> Result<Vec<JusticeLawsInstrument>, String> {
    let tree = Tree::parse(xml.trim_start_matches('\u{feff}'))?;
    let field = |i: usize, tag: &str| {
        tree.child(i, tag)
            .map(|c| tree.text(c, &[]))
            .filter(|t| !t.is_empty())
    };
    let entry = |i: usize| -> Result<(String, JusticeLawsEntry), String> {
        let unique_id = field(i, "UniqueId").ok_or("an entry has no UniqueId")?;
        let title = field(i, "Title").ok_or_else(|| format!("{unique_id} has no Title"))?;
        let language =
            field(i, "Language").ok_or_else(|| format!("{unique_id} has no Language"))?;
        Ok((
            language,
            JusticeLawsEntry {
                official_number: field(i, "OfficialNumber"),
                unique_id,
                title,
            },
        ))
    };
    let list = (0..tree.nodes.len())
        .find(|&i| tree.nodes[i].tag == "ActsRegsList")
        .ok_or("no ActsRegsList")?;
    let mut out = Vec::new();
    for acts in tree.children(list, "Acts") {
        let mut french = Vec::new();
        let mut english = std::collections::HashMap::new();
        for act in tree.children(acts, "Act") {
            match entry(act)? {
                (l, e) if l == "fra" => french.push(e),
                (l, e) if l == "eng" => {
                    english.insert(e.unique_id.clone(), e);
                }
                (l, e) => return Err(format!("{} is in {l}", e.unique_id)),
            }
        }
        out.extend(french.into_iter().map(|fr| JusticeLawsInstrument {
            kind: JusticeLawsKind::Act,
            en: english.remove(&fr.unique_id),
            fr,
        }));
    }
    for regulations in tree.children(list, "Regulations") {
        let mut french = Vec::new();
        let mut english = std::collections::HashMap::new();
        for reg in tree.children(regulations, "Regulation") {
            let id = tree.attr(reg, "id").unwrap_or_default().to_string();
            let other = tree.attr(reg, "olid").map(str::to_string);
            match entry(reg)? {
                (l, e) if l == "fra" => french.push((other, e)),
                (l, e) if l == "eng" => {
                    english.insert(id, e);
                }
                (l, e) => return Err(format!("{} is in {l}", e.unique_id)),
            }
        }
        out.extend(french.into_iter().map(|(other, fr)| JusticeLawsInstrument {
            kind: JusticeLawsKind::Regulation,
            en: other.and_then(|o| english.remove(&o)),
            fr,
        }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statute(body: &str) -> String {
        format!(
            "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?><Statute xml:lang=\"fr\" xmlns:lims=\"http://justice.gc.ca/lims\"><Identification><LongTitle>Loi</LongTitle></Identification><Body>{body}</Body><Schedule><Section><Label>1</Label><Text>Annexe.</Text></Section></Schedule></Statute>"
        )
    }

    #[test]
    fn a_section_reads_its_marginal_note_then_one_line_per_provision() {
        let xml = statute(concat!(
            "<Section lims:inforce-start-date=\"2002-12-31\"><MarginalNote>Compétence</MarginalNote><Label>3</Label>",
            "<Subsection><Label>(1)</Label><Text>Le tribunal de la province<FootnoteRef idref=\"n1\">*</FootnoteRef> peut :</Text>",
            "<Paragraph><Label>a)</Label><Text>instruire l’action le 1<Sup>er</Sup> jour;</Text></Paragraph>",
            "<Paragraph><Label>b)</Label><Text>la renvoyer.</Text></Paragraph>",
            "<ContinuedSectionSubsection><Text>sauf exception.</Text></ContinuedSectionSubsection></Subsection>",
            "<Subsection><MarginalNote>Renvoi</MarginalNote><Label>(2)</Label><Text>Second.</Text></Subsection>",
            "<HistoricalNote><HistoricalNoteSubItem>L.R. (1985), ch. 3</HistoricalNoteSubItem><HistoricalNoteSubItem>2019, ch. 16, art. 2</HistoricalNoteSubItem></HistoricalNote>",
            "<Footnote id=\"n1\"><Text>[Note : en vigueur.]</Text></Footnote></Section>",
        ));
        let arts = justice_laws_articles(&xml).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].num, "3");
        assert_eq!(
            arts[0].texte,
            "Compétence\n(1) Le tribunal de la province peut :\na) instruire l’action le 1er jour;\nb) la renvoyer.\nsauf exception.\nRenvoi\n(2) Second."
        );
        assert_eq!(
            arts[0].nota.as_deref(),
            Some("L.R. (1985), ch. 3; 2019, ch. 16, art. 2\n[Note : en vigueur.]")
        );
    }

    #[test]
    fn headings_nest_by_level() {
        let xml = statute(concat!(
            "<Heading level=\"1\"><Label>PARTIE I</Label><TitleText>Divorce</TitleText></Heading>",
            "<Heading level=\"2\"><TitleText>Compétence</TitleText></Heading>",
            "<Section><Label>3</Label><Text>A.</Text></Section>",
            "<Heading level=\"2\"><TitleText>Ordonnances</TitleText></Heading>",
            "<Section><Label>15.1</Label><Text>B.</Text></Section>",
            "<Heading level=\"1\"><Label>PARTIE II</Label><TitleText>Mesures</TitleText></Heading>",
            "<Section><Label>16</Label><Text>C.</Text></Section>",
        ));
        let paths: Vec<(String, Option<String>)> = justice_laws_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| (a.num, a.title_path))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("3".into(), Some("PARTIE I - Divorce > Compétence".into())),
                (
                    "15.1".into(),
                    Some("PARTIE I - Divorce > Ordonnances".into())
                ),
                ("16".into(), Some("PARTIE II - Mesures".into())),
            ]
        );
    }

    #[test]
    fn a_repealed_range_is_served_as_each_section_it_covers() {
        let xml = statute(concat!(
            "<Section><Label>594 à 596</Label><Text><Repealed>[Abrogés, 1997, ch. 18, art. 69]</Repealed></Text></Section>",
            "<Section><Label>595</Label><Text>Rétabli.</Text></Section>",
            "<Section><Label>672.65 et 672.66</Label><Text><Repealed>[Abrogés, 2005, ch. 22, art. 24]</Repealed></Text></Section>",
            "<Section><Label>7.1 à 8.2</Label><Text><Repealed>[Abrogés]</Repealed></Text></Section>",
            "<Section><Label>9 et 10</Label><Text>[Pas d’articles 9 et 10]</Text></Section>",
        ));
        let got: Vec<(String, Option<String>)> = justice_laws_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| (a.num, a.nota))
            .collect();
        assert_eq!(
            got,
            vec![
                ("594".into(), None),
                ("596".into(), None),
                ("595".into(), None),
                ("672.65".into(), None),
                ("672.66".into(), None),
                ("7.1".into(), Some("7.1 à 8.2".into())),
            ]
        );
    }

    #[test]
    fn an_english_state_reads_its_ranges_and_unused_numbers() {
        let xml = statute(concat!(
            "<Section><Label>3 to 5</Label><Text><Repealed>[Repealed, 1997, c. 18, s. 69]</Repealed></Text></Section>",
            "<Section><Label>6 and 7</Label><Text><Repealed>[Repealed]</Repealed></Text></Section>",
            "<Section><Label>9 and 10</Label><Text>[No sections 9 and 10]</Text></Section>",
        ));
        let nums: Vec<String> = justice_laws_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| a.num)
            .collect();
        assert_eq!(nums, ["3", "4", "5", "6", "7"]);
    }

    #[test]
    fn an_instrument_without_sections_is_one_article() {
        let order = concat!(
            "<Regulation xml:lang=\"fr\"><Identification><InstrumentNumber>TR/90-11</InstrumentNumber></Identification>",
            "<Order><Provision><Text>Il plaît à Son Excellence d’ordonner la vente.</Text></Provision></Order>",
            "<Schedule><Provision><Text>Lot 11.</Text></Provision></Schedule></Regulation>",
        );
        let arts = justice_laws_articles(order).unwrap();
        assert_eq!(
            (arts[0].num.as_str(), arts[0].texte.as_str(), arts.len()),
            (
                "unique",
                "Il plaît à Son Excellence d’ordonner la vente.",
                1
            )
        );
        let repealed = concat!(
            "<Regulation xml:lang=\"fr\"><Identification><ShortTitle>Règlement</ShortTitle></Identification>",
            "<Repealed>[Abrogé, DORS/2022-105, art. 91]</Repealed></Regulation>",
        );
        let arts = justice_laws_articles(repealed).unwrap();
        assert_eq!(
            (arts[0].num.as_str(), arts[0].texte.as_str()),
            ("unique", "[Abrogé, DORS/2022-105, art. 91]")
        );
        let schedule = concat!(
            "<Regulation xml:lang=\"fr\"><Identification><InstrumentNumber>DORS/78-68</InstrumentNumber></Identification>",
            "<Schedule><Provision><Text>Tarif des droits.</Text></Provision></Schedule></Regulation>",
        );
        let arts = justice_laws_articles(schedule).unwrap();
        assert_eq!(
            (arts[0].num.as_str(), arts[0].texte.as_str()),
            ("unique", "Tarif des droits.")
        );
    }

    #[test]
    fn recitals_a_number_borne_twice_and_a_section_without_one_are_kept() {
        let xml = concat!(
            "<Statute xml:lang=\"fr\"><Body>",
            "<Section><Label>Préambule</Label><Text/></Section>",
            "<Section><Label/><Text>Attendu qu’il est dans la tradition canadienne :</Text></Section>",
            "<Section><Label>15.1</Label><Text>Le Comité des parlementaires.</Text></Section>",
            "<Section><Label>15.1</Label><Text>Le Commissaire à la protection de la vie privée.</Text></Section>",
            "</Body></Statute>",
        );
        let arts = justice_laws_articles(xml).unwrap();
        let got: Vec<(&str, &str)> = arts
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Préambule", "Attendu qu’il est dans la tradition canadienne :"),
                (
                    "15.1",
                    "Le Comité des parlementaires.\nLe Commissaire à la protection de la vie privée."
                ),
            ]
        );
        let english = xml.replace("<Label>Préambule</Label>", "<Label>Preamble</Label>");
        assert_eq!(justice_laws_articles(&english).unwrap()[0].num, "Préambule");
        let unlabelled = xml.replace(
            "<Label>15.1</Label><Text>Le Commissaire",
            "<Label/><Text>Le Commissaire",
        );
        assert_eq!(justice_laws_articles(&unlabelled).unwrap(), arts);
    }

    #[test]
    fn the_index_pairs_each_french_instrument_with_its_english_one() {
        let xml = concat!(
            "<ActsRegsList><Acts>",
            "<Act><UniqueId>D-3.4</UniqueId><OfficialNumber>3 (2nd Supp.)</OfficialNumber><Language>eng</Language><Title>Divorce Act</Title></Act>",
            "<Act><UniqueId>D-3.4</UniqueId><OfficialNumber>1985, ch. 3 (2e suppl.)</OfficialNumber><Language>fra</Language><Title>Loi sur le divorce</Title></Act>",
            "</Acts><Regulations>",
            "<Regulation id=\"734629e\" olid=\"723140f\"><UniqueId>SOR-2007-151</UniqueId><Language>eng</Language><Title>Remission Order</Title></Regulation>",
            "<Regulation id=\"723140f\" olid=\"734629e\"><UniqueId>DORS-2007-151</UniqueId><Language>fra</Language><Title>Décret de remise</Title></Regulation>",
            "<Regulation id=\"1f\"><UniqueId>C.R.C., ch. 669</UniqueId><Language>fra</Language><Title>Règlement seul</Title></Regulation>",
            "</Regulations></ActsRegsList>",
        );
        let index = justice_laws_index(xml).unwrap();
        let got: Vec<(JusticeLawsKind, &str, Option<&str>, Option<&str>)> = index
            .iter()
            .map(|i| {
                (
                    i.kind,
                    i.fr.unique_id.as_str(),
                    i.fr.official_number.as_deref(),
                    i.en.as_ref().map(|e| e.unique_id.as_str()),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    JusticeLawsKind::Act,
                    "D-3.4",
                    Some("1985, ch. 3 (2e suppl.)"),
                    Some("D-3.4")
                ),
                (
                    JusticeLawsKind::Regulation,
                    "DORS-2007-151",
                    None,
                    Some("SOR-2007-151")
                ),
                (JusticeLawsKind::Regulation, "C.R.C., ch. 669", None, None),
            ]
        );
    }
}
