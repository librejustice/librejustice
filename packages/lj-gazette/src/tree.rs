//! The element tree a stored notice gives back: whitespace between elements, comments and processing instructions are not part of it.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Empty,
    Text(String),
    Children(Vec<Node>),
}

impl Node {
    pub fn children(&self) -> &[Node] {
        match &self.body {
            Body::Children(c) => c,
            _ => &[],
        }
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children().iter().find(|c| c.name == name)
    }

    pub fn text(&self) -> Option<&str> {
        match &self.body {
            Body::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(Node::text)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Latin-1 byte for byte when the declaration says ISO-8859-1, strict UTF-8 otherwise.
pub fn decode(raw: &[u8]) -> Result<String, Error> {
    let head = &raw[..raw.len().min(200)];
    let declared_latin1 = head.starts_with(b"<?xml")
        && String::from_utf8_lossy(head)
            .to_ascii_lowercase()
            .split("?>")
            .next()
            .is_some_and(|decl| {
                decl.contains("iso-8859-1") || decl.contains("iso8859-1") || decl.contains("latin1")
            });
    if declared_latin1 {
        return Ok(raw.iter().map(|&b| b as char).collect());
    }
    let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(raw);
    String::from_utf8(raw.to_vec()).map_err(|e| {
        Error::new(format!(
            "invalid UTF-8 at byte {}",
            e.utf8_error().valid_up_to()
        ))
    })
}

struct Open {
    node: Node,
    text: String,
    children: Vec<Node>,
}

/// Text standing between elements, which a few published files carry by mistake, is kept as a child of this name.
pub const STRAY_TEXT: &str = "#text";

impl Open {
    fn push_child(&mut self, node: Node) {
        self.flush_stray();
        self.children.push(node);
    }

    fn flush_stray(&mut self) {
        if !self.text.chars().all(char::is_whitespace) {
            self.children.push(Node {
                name: STRAY_TEXT.to_string(),
                attrs: Vec::new(),
                body: Body::Text(std::mem::take(&mut self.text)),
            });
        }
        self.text.clear();
    }

    fn close(mut self) -> Node {
        self.node.body = if self.children.is_empty() {
            if self.text.is_empty() {
                Body::Empty
            } else {
                Body::Text(std::mem::take(&mut self.text))
            }
        } else {
            self.flush_stray();
            Body::Children(self.children)
        };
        self.node
    }
}

fn start(e: &quick_xml::events::BytesStart<'_>) -> Result<Node, Error> {
    let name = std::str::from_utf8(e.name().as_ref())
        .map_err(|_| Error::new("element name is not UTF-8"))?
        .to_string();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|err| Error::new(format!("attribute of <{name}>: {err}")))?;
        let key = std::str::from_utf8(a.key.as_ref())
            .map_err(|_| Error::new("attribute name is not UTF-8"))?
            .to_string();
        let value = match a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
            Ok(v) => v.into_owned(),
            Err(_) => lenient_value(
                std::str::from_utf8(&a.value)
                    .map_err(|_| Error::new(format!("attribute {key} of <{name}> is not UTF-8")))?,
            ),
        };
        attrs.push((key, value));
    }
    Ok(Node {
        name,
        attrs,
        body: Body::Empty,
    })
}

/// A bare `&` or an undeclared entity, which a few published files carry, is kept as written.
fn lenient_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let resolved = rest.find(';').and_then(|end| {
            let name = &rest[1..end];
            let c = match name.strip_prefix('#') {
                Some(n) => match n.strip_prefix('x') {
                    Some(h) => u32::from_str_radix(h, 16).ok().and_then(char::from_u32),
                    None => n.parse().ok().and_then(char::from_u32),
                },
                None => entity(name),
            }?;
            Some((c, end + 1))
        });
        match resolved {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => return None,
    })
}

pub fn parse(xml: &str) -> Result<Node, Error> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Open> = Vec::new();
    let mut root: Option<Node> = None;
    let attach =
        |node: Node, stack: &mut Vec<Open>, root: &mut Option<Node>| -> Result<(), Error> {
            match stack.last_mut() {
                Some(parent) => parent.push_child(node),
                None if root.is_none() => *root = Some(node),
                None => return Err(Error::new("more than one root element")),
            }
            Ok(())
        };
    loop {
        let event = reader
            .read_event()
            .map_err(|e| Error::new(format!("XML at byte {}: {e}", reader.buffer_position())))?;
        match event {
            Event::Start(e) => stack.push(Open {
                node: start(&e)?,
                text: String::new(),
                children: Vec::new(),
            }),
            Event::Empty(e) => attach(start(&e)?, &mut stack, &mut root)?,
            Event::End(_) => {
                let open = stack
                    .pop()
                    .ok_or_else(|| Error::new("closing tag without opening"))?;
                attach(open.close(), &mut stack, &mut root)?;
            }
            Event::Text(t) => {
                let s = t
                    .xml10_content()
                    .map_err(|e| Error::new(format!("text: {e}")))?;
                match stack.last_mut() {
                    Some(open) => open.text.push_str(&s),
                    None if s.chars().all(char::is_whitespace) => {}
                    None => return Err(Error::new("text outside the root element")),
                }
            }
            Event::CData(t) => {
                let open = stack
                    .last_mut()
                    .ok_or_else(|| Error::new("CDATA outside the root element"))?;
                let s = t.decode().map_err(|e| Error::new(format!("CDATA: {e}")))?;
                open.text.push_str(&s);
            }
            Event::GeneralRef(r) => {
                let open = stack
                    .last_mut()
                    .ok_or_else(|| Error::new("reference outside the root element"))?;
                let c = match r
                    .resolve_char_ref()
                    .map_err(|e| Error::new(format!("character reference: {e}")))?
                {
                    Some(c) => c,
                    None => {
                        let name = r
                            .decode()
                            .map_err(|e| Error::new(format!("reference: {e}")))?;
                        match entity(&name) {
                            Some(c) => c,
                            None => {
                                open.text.push('&');
                                open.text.push_str(&name);
                                open.text.push(';');
                                continue;
                            }
                        }
                    }
                };
                open.text.push(c);
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
        }
    }
    if let Some(open) = stack.last() {
        return Err(Error::new(format!(
            "truncated XML, <{}> left open",
            open.node.name
        )));
    }
    root.ok_or_else(|| Error::new("no root element"))
}
