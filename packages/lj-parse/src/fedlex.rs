//! Fedlex's Akoma Ntoso: the articles of one consolidated state of a Swiss federal act.

use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{ConsolidatedArticle, Tree};

const STRUCT_TAGS: &[&str] = &[
    "book",
    "part",
    "title",
    "chapter",
    "subchapter",
    "level",
    "proviso",
];

/// Above this number, an `Art. 1` or `Art. 2` opens the final provisions of a revision, which
/// restart the numbering outside any `<transitional>`.
const RESTART_AFTER: u32 = 50;

static ART_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Art\.\s*(.+)").unwrap());
static LEADING_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+").unwrap());

const NOTES: &[&str] = &["authorialNote"];

fn text(tree: &Tree, idx: usize) -> String {
    tree.text(idx, NOTES)
}

fn article_num(tree: &Tree, idx: usize) -> Option<String> {
    let num = text(tree, tree.child(idx, "num")?);
    ART_NUM
        .captures(&num)
        .map(|c| c[1].trim().to_string())
        .filter(|n| !n.is_empty())
}

fn article_texte(tree: &Tree, idx: usize) -> String {
    let mut parts = Vec::new();
    if let Some(h) = tree.child(idx, "heading") {
        parts.push(text(tree, h));
    }
    let mut paragraphs = tree.children(idx, "paragraph").peekable();
    if paragraphs.peek().is_some() {
        for p in paragraphs {
            let pnum = tree
                .child(p, "num")
                .map(|n| text(tree, n))
                .unwrap_or_default();
            let body = text(tree, tree.child(p, "content").unwrap_or(p));
            parts.push(if pnum.is_empty() {
                body
            } else {
                format!("{pnum} {body}")
            });
        }
    } else if let Some(c) = tree.child(idx, "content") {
        parts.push(text(tree, c));
    }
    parts.retain(|p| !p.is_empty());
    parts.join("\n")
}

fn title_path(tree: &Tree, idx: usize) -> Option<String> {
    let mut labels: Vec<String> = tree
        .ancestors(idx)
        .filter(|&a| STRUCT_TAGS.contains(&tree.nodes[a].tag.as_str()))
        .filter_map(|a| {
            let num = tree
                .child(a, "num")
                .map(|n| text(tree, n))
                .unwrap_or_default();
            let head = tree
                .child(a, "heading")
                .map(|h| text(tree, h))
                .unwrap_or_default();
            let label = format!("{num} {head}");
            let label = label.trim().trim_end_matches(':').trim().replace('>', "›");
            (!label.is_empty()).then_some(label)
        })
        .collect();
    labels.reverse();
    (!labels.is_empty()).then(|| labels.join(" > "))
}

/// The articles of the act's body, in document order: the transitional provisions and the final
/// provisions of later revisions are left out.
pub fn fedlex_articles(xml: &str) -> Result<Vec<ConsolidatedArticle>, String> {
    let tree = Tree::parse(xml)?;
    let mut out = Vec::new();
    let mut highest = 0u32;
    for idx in 0..tree.nodes.len() {
        if tree.nodes[idx].tag != "article" {
            continue;
        }
        if tree
            .ancestors(idx)
            .any(|a| tree.nodes[a].tag == "transitional")
        {
            continue;
        }
        let Some(num) = article_num(&tree, idx) else {
            continue;
        };
        let leading = LEADING_INT
            .find(&num)
            .and_then(|m| m.as_str().parse::<u32>().ok());
        if let Some(n) = leading {
            if n <= 2 && highest >= RESTART_AFTER {
                break;
            }
            highest = highest.max(n);
        }
        let texte = article_texte(&tree, idx);
        if texte.is_empty() {
            continue;
        }
        out.push(ConsolidatedArticle {
            num,
            title_path: title_path(&tree, idx),
            texte,
            nota: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><akomaNtoso xmlns="http://docs.oasis-open.org/legaldocml/ns/akn/3.0"><act><body>{body}</body></act></akomaNtoso>"#
        )
    }

    fn filler(from: u32, to: u32) -> String {
        (from..=to)
            .map(|n| {
                format!(
                    "<article><num>Art. {n}</num><content><p>Corps {n}.</p></content></article>"
                )
            })
            .collect()
    }

    #[test]
    fn an_article_keeps_its_heading_and_numbered_paragraphs_without_its_notes() {
        let xml = act(concat!(
            "<article><num><b>Art. 1</b></num><heading>A. Champ</heading>",
            "<paragraph><num>1</num><content><p>Premier alinéa &amp; suite",
            "<authorialNote><p>RO 2000 1</p></authorialNote> du texte.</p></content></paragraph>",
            "<paragraph><num>2</num><content><p>Second alinéa.</p></content></paragraph>",
            "</article>",
        ));
        let arts = fedlex_articles(&xml).unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].num, "1");
        assert_eq!(
            arts[0].texte,
            "A. Champ\n1 Premier alinéa & suite du texte.\n2 Second alinéa."
        );
    }

    #[test]
    fn a_lettered_article_keeps_its_letter() {
        let xml =
            act("<article><num>Art. 12<i>a</i></num><content><p>Corps.</p></content></article>");
        assert_eq!(fedlex_articles(&xml).unwrap()[0].num, "12a");
    }

    #[test]
    fn the_divisions_make_the_title_path() {
        let xml = act(concat!(
            "<part><num>Première partie:</num><heading>Des personnes</heading>",
            "<title><num>Titre premier</num><heading>Des personnes physiques</heading>",
            "<level><heading>A. La personnalité</heading>",
            "<article><num>Art. 11</num><content><p>Corps.</p></content></article>",
            "</level></title></part>",
        ));
        assert_eq!(
            fedlex_articles(&xml).unwrap()[0].title_path.as_deref(),
            Some("Première partie: Des personnes > Titre premier Des personnes physiques > A. La personnalité")
        );
    }

    #[test]
    fn transitional_and_restarted_final_provisions_are_left_out() {
        let xml = act(&format!(
            "{}<transitional><article><num>Art. 3</num><content><p>Transitoire.</p></content></article></transitional>\
             <level><heading>Dispositions finales</heading><article><num>Art. 1</num><content><p>Final.</p></content></article></level>",
            filler(1, 55)
        ));
        let arts = fedlex_articles(&xml).unwrap();
        assert_eq!(arts.len(), 55);
        assert_eq!(arts.last().unwrap().num, "55");
    }
}
