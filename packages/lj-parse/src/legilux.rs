//! Legilux's Akoma Ntoso: the articles of a Luxembourg act, consolidated or as published.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{Child, ConsolidatedArticle, Lines, Tree};

const STRUCT_TAGS: &[&str] = &[
    "part",
    "book",
    "title",
    "subtitle",
    "chapter",
    "subchapter",
    "section",
    "subsection",
];

static ID_ORDINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d|er)(quinquies|quater|septies|sexies|octies|nonies|undecies|duodecies|decies|novies|bis|ter)$")
        .unwrap()
});
/// A placeholder for repealed articles (`Art. 41 à 43.`, `Art. 367-2. et Art. 367-3.`): the id
/// writes the connector `à` or `a` depending on the state, so the label is the only sign.
static RANGE_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Art\.\s*\S+\s*(?:à|et)\s").unwrap());
/// The states lay a `<ref>` out on its own lines or inline, so the space it leaves beside a
/// parenthesis, a comma, a period or an elided article is dropped.
static REF_SPACING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\(|\b(?i:l|d|n|s|j|m|t|c|qu|jusqu|lorsqu|puisqu|quoiqu)[’'])\s+|\s+([),.;])")
        .unwrap()
});
/// The states write `2bis` and `2 bis` in turn: the ordinal is detached, as in a number.
static TEXT_ORDINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\d)(bis|ter|quater|quinquies|sexies|septies|octies|nonies|decies|undecies|duodecies)\b",
    )
    .unwrap()
});

/// The day a state stamps on the id of an article it rewrote: `art_2_20180921`.
static ID_DAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_(?:19|20)\d{6}$").unwrap());

/// `art_401bis` is `401 bis`: the cited form detaches the ordinal. `art_2_4_1` is `2.4.1`.
fn num_from_id(id: &str) -> Result<String, String> {
    let core = ["art__art_", "article_", "Article_", "art_", "art"]
        .iter()
        .find_map(|p| id.strip_prefix(p))
        .map(|c| {
            ID_DAY
                .replace(c, "")
                .trim_end_matches('-')
                .replace('_', ".")
        })
        .filter(|c| c.starts_with(|ch: char| ch.is_ascii_digit()))
        .ok_or_else(|| format!("article id {id:?} holds no number"))?;
    Ok(ID_ORDINAL.replace(&core, "$1 $2").into_owned())
}

fn letter(n: u32, upper: bool) -> String {
    let base = if upper { b'A' } else { b'a' };
    char::from(base + ((n - 1) % 26) as u8).to_string()
}

/// The printed mark of the `n`-th item of a list whose symbol is `1°`, `1)`, `1.`, `a)`, `dash`…
fn list_mark(symbol: &str, n: u32) -> Option<String> {
    match symbol {
        "dash" => Some("-".to_string()),
        "" | "none" | "custom" => None,
        s if s.contains('1') => Some(s.replacen('1', &n.to_string(), 1)),
        s if s.contains('a') => Some(s.replacen('a', &letter(n, false), 1)),
        s if s.contains('A') => Some(s.replacen('A', &letter(n, true), 1)),
        _ => None,
    }
}

fn normalize(line: String) -> String {
    let line = REF_SPACING.replace_all(&line, "$1$2");
    TEXT_ORDINAL.replace_all(&line, "$1 $2").into_owned()
}

struct Render<'a> {
    tree: &'a Tree,
    notes: Vec<String>,
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

    fn node(&mut self, i: usize, out: &mut Lines) {
        let n = &self.tree.nodes[i];
        if n.prefixed {
            return;
        }
        match n.tag.as_str() {
            "noteRef" => {
                if let Some(href) = self.tree.attr(i, "href") {
                    self.notes.push(href.trim_start_matches('#').to_string());
                }
            }
            "sup" => {
                out.glue();
                self.walk(i, out);
            }
            "br" => out.line_break(),
            "paragraph" => {
                out.line_break();
                if let Some(num) = self.tree.child(i, "num") {
                    let num = self.inline(num);
                    if !num.is_empty() {
                        out.mark(&num);
                    }
                }
                for child in &self.tree.nodes[i].children {
                    match child {
                        Child::Node(c) if self.tree.nodes[*c].tag == "num" => {}
                        Child::Node(c) => self.node(*c, out),
                        Child::Text(t) => out.push(t),
                    }
                }
            }
            "li" => {
                out.line_break();
                let list = n.parent;
                let symbol = self.tree.attr(list, "symbol").unwrap_or_default();
                let start: u32 = self
                    .tree
                    .attr(list, "start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                let rank = self
                    .tree
                    .children(list, "li")
                    .position(|c| c == i)
                    .unwrap_or(0) as u32;
                let mark = match (self.tree.nodes[list].tag.as_str(), symbol) {
                    ("ol", _) => list_mark(symbol, start + rank),
                    (_, "") => Some("•".to_string()),
                    _ => list_mark(symbol, 1),
                };
                if let Some(mark) = mark {
                    out.mark(&mark);
                }
                self.walk(i, out);
            }
            "alinea" | "p" | "tr" => {
                out.line_break();
                self.walk(i, out);
            }
            _ => self.walk(i, out),
        }
    }

    fn inline(&mut self, idx: usize) -> String {
        let mut lines = Lines::new(normalize);
        self.walk(idx, &mut lines);
        lines.finish().join(" ")
    }
}

fn title_path(tree: &Tree, idx: usize) -> Option<String> {
    let mut render = Render {
        tree,
        notes: Vec::new(),
    };
    let mut labels: Vec<String> = tree
        .ancestors(idx)
        .filter(|&a| STRUCT_TAGS.contains(&tree.nodes[a].tag.as_str()) && !tree.nodes[a].prefixed)
        .filter_map(|a| {
            let num = tree.child(a, "num").map(|n| render.inline(n));
            let head = tree.child(a, "heading").map(|h| render.inline(h));
            let label = match (
                num.filter(|n| !n.is_empty()),
                head.filter(|h| !h.is_empty()),
            ) {
                (Some(n), Some(h)) => format!("{n} - {h}"),
                (Some(n), None) => n,
                (None, Some(h)) => h,
                (None, None) => return None,
            };
            Some(label.replace('>', "›"))
        })
        .collect();
    labels.reverse();
    (!labels.is_empty()).then(|| labels.join(" > "))
}

/// The articles of the code's body, in document order. A repealed range is served as each article
/// it covers that does not stand alone, or once under its first number when it lists none.
pub fn legilux_articles(xml: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let tree = Tree::parse(xml)?;
    let notes: HashMap<&str, usize> = (0..tree.nodes.len())
        .filter(|&i| tree.nodes[i].tag == "note" && !tree.nodes[i].prefixed)
        .filter_map(|i| tree.attr(i, "id").map(|id| (id, i)))
        .collect();
    let articles: Vec<usize> = (0..tree.nodes.len())
        .filter(|&i| {
            tree.nodes[i].tag == "article"
                && !tree.nodes[i].prefixed
                && tree.attr(i, "style") != Some("del")
                && tree.ancestors(i).any(|a| tree.nodes[a].tag == "body")
                && !tree.ancestors(i).any(|a| {
                    matches!(
                        tree.nodes[a].tag.as_str(),
                        "embeddedStructure" | "quotedStructure" | "mod"
                    )
                })
        })
        .collect();

    let mut render = Render {
        tree: &tree,
        notes: Vec::new(),
    };
    let mut labelled = Vec::with_capacity(articles.len());
    for &idx in &articles {
        let id = tree
            .attr(idx, "id")
            .ok_or("an article has no id")?
            .to_string();
        let label = tree
            .child(idx, "num")
            .map(|n| render.inline(n))
            .unwrap_or_default();
        labelled.push((idx, id, RANGE_LABEL.is_match(&label).then_some(label)));
    }
    let solo: HashSet<&str> = labelled
        .iter()
        .filter(|(_, _, range)| range.is_none())
        .map(|(_, id, _)| id.as_str())
        .collect();

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (idx, id, range) in &labelled {
        render.notes.clear();
        let mut lines = Lines::new(normalize);
        if let Some(h) = tree.child(*idx, "heading") {
            let heading = render.inline(h);
            lines.push(&heading);
            lines.line_break();
        }
        for child in &tree.nodes[*idx].children {
            match child {
                Child::Node(c) if matches!(tree.nodes[*c].tag.as_str(), "num" | "heading") => {}
                Child::Node(c) => render.node(*c, &mut lines),
                Child::Text(t) => lines.push(t),
            }
        }
        let texte = lines.finish().join("\n");
        if texte.is_empty() {
            continue;
        }
        let refs = std::mem::take(&mut render.notes);
        let mut nota: Vec<String> = refs
            .iter()
            .filter_map(|n| notes.get(n.as_str()))
            .map(|&n| render.inline(n))
            .filter(|n| !n.is_empty())
            .collect();
        let title_path = title_path(&tree, *idx);

        let covered: Vec<&str> = tree
            .attr(*idx, "ids")
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        let nums = match range {
            None => vec![num_from_id(id)?],
            Some(_) if !covered.is_empty() => covered
                .iter()
                .filter(|c| !solo.contains(*c))
                .map(|c| num_from_id(c))
                .collect::<Result<_, _>>()?,
            Some(label) => {
                nota.insert(0, label.clone());
                let first = id.split('_').nth(1).unwrap_or_default();
                vec![num_from_id(&format!("art_{first}"))?]
            }
        };
        let nota = (!nota.is_empty()).then(|| nota.join("\n"));
        for num in nums {
            if !seen.insert(num.clone()) {
                return Err(format!("article {num} appears twice"));
            }
            out.push(ConsolidatedArticle {
                num,
                title_path: title_path.clone(),
                texte: texte.clone(),
                nota: nota.clone(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><akomaNtoso xmlns="http://docs.oasis-open.org/legaldocml/ns/akn/3.0/CSD13" xmlns:scl="http://www.scl.lu"><act><meta><notes><note id="note_0_0"><p>Note de l'éditeur.</p></note></notes></meta><body>{body}</body></act></akomaNtoso>"#
        )
    }

    fn article(id: &str, label: &str, body: &str) -> String {
        format!(
            r#"<article id="{id}"><scl:JOLUXWork><scl:jolux scl:name="title">{label}</scl:jolux></scl:JOLUXWork><num>{label}</num>{body}</article>"#
        )
    }

    fn alinea(text: &str) -> String {
        format!("<alinea><content><p>{text}</p></content></alinea>")
    }

    #[test]
    fn the_number_comes_from_the_id_with_its_ordinal_detached() {
        let xml = code(&format!(
            "{}{}{}{}{}{}{}",
            article("art_1er", "Art. 1er.", &alinea("Premier.")),
            article("art_1erbis", "Art. 1erbis.", &alinea("Premier bis.")),
            article("art_401bis", "Art. 401bis.", &alinea("Bis.")),
            article("art_2_4_1", "Art. 2.4.1.", &alinea("Sous-section.")),
            article("article_3", "Art. 3.", &alinea("Trois.")),
            article("art30undecies", "Art.30undecies.", &alinea("Onze.")),
            article("art_5_20200401", "Art. 5.", &alinea("Réécrit.")),
        ));
        let nums: Vec<String> = legilux_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| a.num)
            .collect();
        assert_eq!(
            nums,
            vec![
                "1er",
                "1er bis",
                "401 bis",
                "2.4.1",
                "3",
                "30 undecies",
                "5"
            ]
        );
    }

    #[test]
    fn an_act_keeps_its_own_articles_and_not_those_it_quotes_nor_a_deleted_form() {
        let xml = code(&format!(
            "{}{}{}",
            article(
                "art_1er",
                "Art. 1er.",
                "<alinea><content><p>Il est inséré un article 7bis:<embeddedStructure><article><num>Art. 7bis.</num><alinea><content><p>Texte inséré.</p></content></alinea></article></embeddedStructure></p></content></alinea>",
            ),
            r#"<article id="art_2_20180921" style="del"><num>Art. 2.</num><alinea><content><p>Ancien.</p></content></alinea></article>"#,
            article("art_2", "Art. 2.", &alinea("Nouveau.")),
        ));
        let read: Vec<(String, String)> = legilux_articles(&xml)
            .unwrap()
            .into_iter()
            .map(|a| (a.num, a.texte))
            .collect();
        assert_eq!(
            read,
            [
                (
                    "1er".to_string(),
                    "Il est inséré un article 7 bis:Art. 7 bis.\nTexte inséré.".to_string()
                ),
                ("2".to_string(), "Nouveau.".to_string()),
            ]
        );
    }

    #[test]
    fn the_amending_act_heads_the_text_and_the_metadata_stays_out() {
        let xml = code(&article(
            "art_30",
            "Art. 30.",
            concat!(
                "<heading>( <ref href=\"/eli/x\">L. 13 juin 1994</ref> )</heading>",
                "<scl:indexterms><scl:label>Contrainte</scl:label></scl:indexterms>",
                "<paragraph><num>(1)</num><alinea><content><p>( <ref>L. 1\n<sup>er</sup>\naoût 2001</ref> ) La durée.</p></content></alinea></paragraph>",
                "<paragraph><num>(2bis)</num><alinea><content><p>Selon l’\n<ref>article 33\n</ref>\n, dix ans.</p></content></alinea></paragraph>",
            ),
        ));
        assert_eq!(
            legilux_articles(&xml).unwrap()[0].texte,
            "(L. 13 juin 1994)\n(1) (L. 1er août 2001) La durée.\n(2 bis) Selon l’article 33, dix ans."
        );
    }

    #[test]
    fn list_items_carry_the_mark_their_list_declares() {
        let xml = code(&article(
            "art_7",
            "Art. 7.",
            concat!(
                "<alinea><content><p>Les peines sont:</p>",
                "<ol scl:symbol=\"1)\"><li>la réclusion;</li><li>l'amende;</li></ol>",
                "<ol xmlns:ns1=\"http://www.scl.lu\" ns1:start=\"3\" ns1:symbol=\"1°\"><li>la confiscation.</li></ol>",
                "<ul scl:symbol=\"dash\"><li>un tiret</li></ul><ul><li>une puce</li></ul></content></alinea>",
            ),
        ));
        assert_eq!(
            legilux_articles(&xml).unwrap()[0].texte,
            "Les peines sont:\n1) la réclusion;\n2) l'amende;\n3° la confiscation.\n- un tiret\n• une puce"
        );
    }

    #[test]
    fn a_range_is_served_as_each_article_it_covers_unless_one_stands_alone() {
        let xml = code(&format!(
            "{}{}{}",
            article(
                "art_80_à_82",
                "Art. 80 à 82.",
                &alinea("Abrogés ( L. 13 juin 1994 )")
            ),
            article("art_81", "Art. 81.", &alinea("Rétabli.")),
            article(
                "art_181_à_art_192-2",
                "Art. 181. à Art. 192-2.",
                &alinea("Abrogés.")
            ),
        ))
        .replace(
            "id=\"art_80_à_82\"",
            "id=\"art_80_à_82\" scl:ids=\"art_80;art_81;art_82;\"",
        );
        let arts = legilux_articles(&xml).unwrap();
        let got: Vec<(&str, &str, Option<&str>)> = arts
            .iter()
            .map(|a| (a.num.as_str(), a.texte.as_str(), a.nota.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("80", "Abrogés (L. 13 juin 1994)", None),
                ("82", "Abrogés (L. 13 juin 1994)", None),
                ("81", "Rétabli.", None),
                ("181", "Abrogés.", Some("Art. 181. à Art. 192-2.")),
            ]
        );
    }

    #[test]
    fn divisions_make_the_title_path_and_notes_go_to_the_nota() {
        let xml = code(&format!(
            "<book id=\"livre_ier\"><num><b>LIVRE I<sup>er</sup>.</b></num><heading><b>Des infractions</b></heading>\
             <chapter><num>Chapitre II.</num><heading>Des peines</heading>{}</chapter></book>",
            article(
                "art_563",
                "Art. 563.",
                &alinea("<noteRef href=\"#note_0_0\"/>Les vagabonds.")
            )
        ));
        let a = &legilux_articles(&xml).unwrap()[0];
        assert_eq!(
            a.title_path.as_deref(),
            Some("LIVRE Ier. - Des infractions > Chapitre II. - Des peines")
        );
        assert_eq!(a.texte, "Les vagabonds.");
        assert_eq!(a.nota.as_deref(), Some("Note de l'éditeur."));
    }
}
