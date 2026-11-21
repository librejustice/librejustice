//! A consolidation's XML held as a tree, its text laid out one block per line, and the article
//! every consolidation reader returns.

use std::sync::LazyLock;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::Regex;

/// One article of a consolidated state, its text one block per line, the editor's notes apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidatedArticle {
    pub num: String,
    pub title_path: Option<String>,
    pub texte: String,
    pub nota: Option<String>,
}

pub(crate) enum Child {
    Node(usize),
    Text(String),
}

pub(crate) struct Node {
    /// Local name: `scl:jolux` is `jolux`, with `prefixed` set.
    pub(crate) tag: String,
    pub(crate) prefixed: bool,
    attrs: Vec<(String, String)>,
    pub(crate) parent: usize,
    pub(crate) children: Vec<Child>,
}

pub(crate) struct Tree {
    pub(crate) nodes: Vec<Node>,
}

fn node(e: &BytesStart, parent: usize) -> Result<Node, String> {
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| e.to_string())?;
        let value = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| e.to_string())?
            .into_owned();
        attrs.push((
            String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(),
            value,
        ));
    }
    Ok(Node {
        tag: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(),
        prefixed: e.name().prefix().is_some(),
        attrs,
        parent,
        children: Vec::new(),
    })
}

impl Tree {
    pub(crate) fn parse(xml: &str) -> Result<Self, String> {
        let mut nodes = vec![Node {
            tag: String::new(),
            prefixed: false,
            attrs: Vec::new(),
            parent: 0,
            children: Vec::new(),
        }];
        let mut stack = vec![0usize];
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        loop {
            let top = *stack.last().ok_or("closing tag without an opening one")?;
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let idx = nodes.len();
                    nodes.push(node(&e, top)?);
                    nodes[top].children.push(Child::Node(idx));
                    stack.push(idx);
                }
                Ok(Event::Empty(e)) => {
                    let idx = nodes.len();
                    nodes.push(node(&e, top)?);
                    nodes[top].children.push(Child::Node(idx));
                }
                Ok(Event::End(_)) => {
                    stack.pop();
                }
                Ok(Event::Text(e)) => {
                    let raw = e.decode().map_err(|e| e.to_string())?;
                    nodes[top].children.push(Child::Text(raw.into_owned()));
                }
                Ok(Event::CData(e)) => {
                    let raw = String::from_utf8_lossy(e.as_ref()).into_owned();
                    nodes[top].children.push(Child::Text(raw));
                }
                Ok(Event::GeneralRef(e)) => {
                    let name = e.decode().map_err(|e| e.to_string())?;
                    let text = quick_xml::escape::unescape(&format!("&{name};"))
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    nodes[top].children.push(Child::Text(text));
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(e) => return Err(format!("at byte {}: {e}", reader.error_position())),
            }
        }
        Ok(Self { nodes })
    }

    pub(crate) fn attr(&self, idx: usize, name: &str) -> Option<&str> {
        self.nodes[idx]
            .attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn child(&self, idx: usize, tag: &str) -> Option<usize> {
        self.children(idx, tag).next()
    }

    pub(crate) fn children<'a>(
        &'a self,
        idx: usize,
        tag: &'a str,
    ) -> impl Iterator<Item = usize> + 'a {
        self.nodes[idx]
            .children
            .iter()
            .filter_map(move |c| match c {
                Child::Node(i) if self.nodes[*i].tag == tag && !self.nodes[*i].prefixed => Some(*i),
                _ => None,
            })
    }

    /// The text under `idx`, whitespace folded, without `skip` nor any foreign-namespace element.
    pub(crate) fn text(&self, idx: usize, skip: &[&str]) -> String {
        let mut out = String::new();
        self.collect(idx, skip, &mut out);
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn collect(&self, idx: usize, skip: &[&str], out: &mut String) {
        for c in &self.nodes[idx].children {
            match c {
                Child::Text(t) => out.push_str(t),
                Child::Node(i) => {
                    let n = &self.nodes[*i];
                    if !n.prefixed && !skip.contains(&n.tag.as_str()) {
                        self.collect(*i, skip, out);
                    }
                }
            }
        }
    }

    pub(crate) fn ancestors(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(self.nodes[idx].parent), |&p| {
            (p != 0).then_some(self.nodes[p].parent)
        })
        .take_while(|&p| p != 0)
    }
}

/// Glues a superscript to the token before it: `1<sup>er</sup>` reads `1er`.
const GLUE: char = '\u{1}';
static GLUED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\x01\s*").unwrap());

pub(crate) struct Lines {
    done: Vec<String>,
    current: String,
    /// Set after a number or a list mark, until text follows it on its line.
    hold: bool,
    normalize: fn(String) -> String,
}

impl Lines {
    pub(crate) fn new(normalize: fn(String) -> String) -> Self {
        Self {
            done: Vec::new(),
            current: String::new(),
            hold: false,
            normalize,
        }
    }

    pub(crate) fn push(&mut self, text: &str) {
        self.current.push_str(text);
        if !text.trim().is_empty() {
            self.hold = false;
        }
    }

    pub(crate) fn glue(&mut self) {
        self.current.push(GLUE);
    }

    pub(crate) fn mark(&mut self, mark: &str) {
        self.push(mark);
        self.current.push(' ');
        self.hold = true;
    }

    pub(crate) fn line_break(&mut self) {
        if self.hold {
            return;
        }
        let line = GLUED.replace_all(&self.current, "");
        let line = (self.normalize)(line.split_whitespace().collect::<Vec<_>>().join(" "));
        if !line.is_empty() {
            self.done.push(line);
        }
        self.current.clear();
    }

    pub(crate) fn finish(mut self) -> Vec<String> {
        self.hold = false;
        self.line_break();
        self.done
    }
}
