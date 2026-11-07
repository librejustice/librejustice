//! An OOXML package: every text node of every part, and a splice back into the
//! raw bytes so everything no edit names survives byte for byte.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Cursor, Read, Write};
use std::ops::Range;

use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::docx::{RunPatch, StyleSheet, Theme};
use crate::{record, views_showing, Anchor, NodeEdit, Replacement, Views, EVERY_VIEW};

/// How deep a package stored as an entry of another is opened: a workbook
/// embedded in a document embedded in a document.
const NESTING: usize = 4;

const BOM: &[u8] = b"\xEF\xBB\xBF";
const PRESERVE: &[u8] = b" xml:space=\"preserve\"";

/// One run of character data or one attribute value: its escaped bytes in the
/// part, and what it says.
struct TextNode {
    span: Range<usize>,
    text: String,
    /// The views that show it.
    views: Views,
    /// The views in which a line break follows it, none when zero.
    breaks_after: Views,
    /// Where ` xml:space="preserve"` goes in the enclosing start tag, for an
    /// element that drops the blanks at its edges without it.
    preserve_at: Option<usize>,
    /// Author metadata, which [`OoxmlPackage::clear_authors`] empties.
    scrub: bool,
    /// A path whose folders name the author's machine, cut to its file name.
    path: bool,
}

/// A part walked for text, for the attribute values that carry prose, or both.
struct Part {
    entry: usize,
    /// Text nodes, numbered as [`walk`] numbers them; empty for a part that
    /// carries no prose.
    texts: Vec<TextNode>,
    /// Attribute values that carry prose or name someone, numbered after the
    /// text nodes.
    values: Vec<TextNode>,
    /// The keyword and switches of each field instruction, and every character
    /// of a bookmark or a drawing name, as text node and offset in it.
    markup: Vec<(u32, u32)>,
}

impl Part {
    fn nodes(&self) -> impl Iterator<Item = &TextNode> {
        self.texts.iter().chain(&self.values)
    }
}

/// A ZIP-XML package held in memory, entries in their original order.
pub struct OoxmlPackage {
    entries: Vec<(String, Vec<u8>)>,
    parts: Vec<Part>,
    /// Packages stored as one of the entries, an embedded document or workbook,
    /// whose parts are numbered after this package's own.
    nested: Vec<(usize, OoxmlPackage)>,
    /// Entries that are neither a part nor a package: media, fonts, OLE objects.
    binaries: Vec<usize>,
    /// The replacements recorded, by part as anchors number them, nested
    /// packages included; empty in a nested package.
    edits: Vec<BTreeMap<u32, NodeEdit>>,
    clear_authors: bool,
    drop_thumbnails: bool,
}

/// A part is scanned when it can carry prose. `_rels` and the content-type map
/// hold identifiers, never text a reader sees.
fn carries_text(name: &str) -> bool {
    name.ends_with(".xml")
        && name != "[Content_Types].xml"
        && !name.contains("_rels/")
        && !name.ends_with(".rels")
}

/// Entries [`OoxmlPackage::drop_thumbnails`] drops: the first-page thumbnail
/// shows the document as it was, and a signature no longer matches once a part
/// changes and names its signer.
fn dropped(name: &str) -> bool {
    name.starts_with("docProps/thumbnail") || name.starts_with("_xmlsignatures/")
}

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn prefix(name: &[u8]) -> &[u8] {
    match name.iter().position(|b| *b == b':') {
        Some(at) => &name[..at],
        None => b"",
    }
}

/// Element whose end means the reader sees a line break, so a name may not be
/// matched across it. Two `<w:t>` of one paragraph, on the other hand, are
/// contiguous text, and that is where a split name hides.
fn ends_a_paragraph(name: &[u8]) -> bool {
    matches!(
        local(name),
        b"p" | b"tr" | b"br" | b"cr" | b"tab" | b"si" | b"c"
    )
}

/// Element whose blanks at the edges Word and LibreOffice drop unless it
/// carries `xml:space="preserve"`. DrawingML's `a:t` keeps them and takes no
/// such attribute.
fn strips_blanks(name: &[u8]) -> bool {
    prefix(name) != b"a"
        && matches!(
            local(name),
            b"t" | b"delText" | b"instrText" | b"delInstrText"
        )
}

/// Element whose text is who wrote or last saved the document.
fn names_an_author(part: &str, name: &[u8]) -> bool {
    match name {
        b"dc:creator" | b"cp:lastModifiedBy" => true,
        b"Company" | b"Manager" => part == "docProps/app.xml",
        b"author" => part.contains("comments"),
        _ => false,
    }
}

/// Attribute naming who wrote a comment or a revision, who may edit a range,
/// or a task's assignee.
fn is_author_attribute(element: &[u8], attribute: &[u8]) -> bool {
    match attribute {
        b"w:author" | b"w:initials" | b"w:ed" | b"w15:author" | b"w15:userId"
        | b"w15:providerId" | b"userName" | b"userId" => true,
        b"displayName" => local(element) == b"person",
        b"name" | b"initials" => local(element) == b"cmAuthor",
        _ => false,
    }
}

/// A custom property naming who wrote, sent, reviewed or owns the document:
/// `_AuthorEmail`, `Document Author`, `_reviewerList`, a label's `_Owner`.
pub fn is_author_property(name: &str) -> bool {
    let key: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    key.ends_with("owner")
        || matches!(
            key.as_str(),
            "author"
                | "authors"
                | "documentauthor"
                | "lastauthor"
                | "authoremail"
                | "authoremaildisplayname"
                | "emailsubject"
                | "creator"
                | "lastmodifiedby"
                | "company"
                | "manager"
                | "reviewerlist"
                | "approverlist"
        )
}

/// A reference to a file on a disk or a share, whose folders name the author's
/// machine and account: `file:///C:\Users\jdupont\...`, `D:\...`, `\\srv\...`.
pub fn is_local_file(target: &str) -> bool {
    let bytes = target.trim_start().as_bytes();
    bytes
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case(b"file:"))
        || bytes.starts_with(b"\\\\")
        || matches!(bytes, [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic())
}

/// How many characters of `path` come before its last segment.
pub fn directory_len(path: &str) -> usize {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rfind(['/', '\\'])
        .map_or(0, |at| trimmed[..=at].chars().count())
}

/// Attribute whose value a user wrote or that names something the document
/// is about: alternative text, a link, a field code, a bookmark, a form
/// field, a document variable. Style, font and part names stay: they are the
/// template's, and rewriting one breaks what refers to it.
fn carries_prose(stack: &[Open], element: &[u8], attribute: &[u8]) -> bool {
    let parent = stack.last().map_or(&b""[..], |o| local(&o.name));
    let element_local = local(element);
    match attribute {
        b"descr" | b"title" | b"name" if matches!(element_local, b"docPr" | b"cNvPr") => true,
        b"tooltip" => matches!(element_local, b"hlinkClick" | b"hlinkHover"),
        b"w:tooltip" | b"w:anchor" | b"w:docLocation" | b"w:instr" => true,
        b"title" if element_local == b"SetTitle" => true,
        b"w:name" => element_local == b"bookmarkStart",
        b"w:val" => match element_local {
            b"docVar" | b"alias" | b"tag" | b"helpText" | b"statusText" | b"listEntry"
            | b"lvlText" | b"connectString" | b"query" | b"udl" | b"attr" => true,
            b"name" => parent == b"ffData",
            b"default" => parent == b"textInput",
            _ => false,
        },
        b"w:displayText" | b"w:value" => element_local == b"listItem",
        b"o:title" | b"o:href" => true,
        b"alt" | b"href" | b"title" | b"string" => prefix(element) == b"v",
        _ => false,
    }
}

/// Positions in an attribute value [`carries_prose`] keeps that are markup:
/// the keyword and switches of a simple field, the whole of a bookmark name,
/// whether a bookmark, a form field or a link landing on one names it, and the
/// whole of a drawing's name, which Word numbers `Image 281969989`.
fn value_markup(stack: &[Open], element: &[u8], attribute: &[u8], text: &str) -> Vec<usize> {
    let parent = stack.last().map_or(&b""[..], |o| local(&o.name));
    let identifier = match attribute {
        b"w:instr" => return crate::field::markup(&text.chars().collect::<Vec<_>>()),
        b"name" => matches!(local(element), b"docPr" | b"cNvPr"),
        b"w:name" => local(element) == b"bookmarkStart",
        b"w:anchor" => true,
        b"w:val" => local(element) == b"name" && parent == b"ffData",
        _ => false,
    };
    match identifier {
        true => (0..text.chars().count()).collect(),
        false => Vec::new(),
    }
}

/// Resolve one of the five entities XML predefines, which are the only named
/// ones an OOXML part uses.
fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Where `inner` starts in `whole`, which it borrows from.
fn offset_in(whole: &[u8], inner: &[u8]) -> Result<usize> {
    let at = (inner.as_ptr() as usize).wrapping_sub(whole.as_ptr() as usize);
    match at + inner.len() <= whole.len() {
        true => Ok(at),
        false => Err(anyhow!("an XML event does not borrow from its part")),
    }
}

/// One step of [`walk`] over a package part.
pub enum Step<'e, 'a> {
    /// An element opens; `empty` when it closes at once, as `<w:tab/>`.
    Open {
        element: &'e BytesStart<'a>,
        empty: bool,
    },
    Close(&'e BytesEnd<'a>),
    /// One text node: `node` is the number an [`Anchor::XmlText`] carries, and
    /// `span` its escaped bytes in the part.
    Text {
        node: u32,
        text: &'e str,
        span: std::ops::Range<usize>,
    },
}

/// Walk a part in document order, numbering its text nodes the one way the
/// splice numbers them, so a reader that builds on this walk hands out anchors
/// [`OoxmlPackage::replace`] consumes. A node is a run of character data or one
/// entity reference, and an empty one takes no number.
pub fn walk(part: &[u8], mut visit: impl FnMut(Step<'_, '_>)) -> Result<()> {
    // The reader skips a byte-order mark without counting it in its positions.
    let bom = if part.starts_with(BOM) { BOM.len() } else { 0 };
    let mut reader = Reader::from_reader(&part[bom..]);
    reader.config_mut().trim_text(false);
    let mut node: u32 = 0;
    let mut text =
        |text: &str, span: std::ops::Range<usize>, visit: &mut dyn FnMut(Step<'_, '_>)| {
            if !text.is_empty() {
                visit(Step::Text { node, text, span });
                node += 1;
            }
        };
    loop {
        let start = bom + reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Text(e)) => {
                let end = bom + reader.buffer_position() as usize;
                let raw = e
                    .decode()
                    .map_err(|e| anyhow!("decoding a text node: {e}"))?;
                let unescaped = quick_xml::escape::unescape(&raw)
                    .map_err(|e| anyhow!("unescaping a text node: {e}"))?;
                text(&unescaped, start..end, &mut visit);
            }
            Ok(Event::GeneralRef(e)) => {
                let end = bom + reader.buffer_position() as usize;
                let name = e.decode().map_err(|e| anyhow!("decoding an entity: {e}"))?;
                let resolved = match e.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    _ => resolve_entity(&name),
                };
                text(
                    &resolved.map(String::from).unwrap_or_default(),
                    start..end,
                    &mut visit,
                );
            }
            Ok(Event::Start(e)) => visit(Step::Open {
                element: &e,
                empty: false,
            }),
            Ok(Event::Empty(e)) => visit(Step::Open {
                element: &e,
                empty: true,
            }),
            Ok(Event::End(e)) => visit(Step::Close(&e)),
            Ok(_) => {}
            Err(e) => return Err(anyhow!("reading the XML part: {e}")),
        }
    }
    Ok(())
}

/// A field whose instruction [`scan`] is reading, until its separator or its
/// end: each character with its text node and offset.
#[derive(Default)]
struct Field {
    separated: bool,
    instruction: Vec<(char, u32, u32)>,
}

impl Field {
    fn markup(&self, out: &mut Vec<(u32, u32)>) {
        let chars: Vec<char> = self.instruction.iter().map(|(c, ..)| *c).collect();
        for at in crate::field::markup(&chars) {
            let (_, node, offset) = self.instruction[at];
            out.push((node, offset));
        }
    }
}

/// An element still open during [`scan`].
struct Open {
    name: Vec<u8>,
    preserve_at: Option<usize>,
    scrub: bool,
    /// In the WordprocessingML namespace, whatever its prefix.
    word: bool,
    /// The namespace prefixes it declares, each with whether it binds
    /// WordprocessingML.
    declares: Vec<(Vec<u8>, bool)>,
    /// Inside a tracked deletion or insertion.
    deleted: bool,
    inserted: bool,
    /// For a Word run, its character formatting; for a paragraph, its mark's.
    format: RunPatch,
    /// For a Word paragraph: its style, and whether its mark is a tracked
    /// deletion or insertion.
    paragraph_style: Option<String>,
    mark_deleted: bool,
    mark_inserted: bool,
}

/// What resolves whether Word text is hidden: the package's styles, and
/// whether any of them hides text at all.
struct Hiding {
    sheet: StyleSheet,
    theme: Theme,
    styled: bool,
}

impl Hiding {
    /// Whether the run formatted `format` in a paragraph of style
    /// `paragraph_style` is hidden text. Its own `w:vanish` comes last and
    /// decides alone.
    fn hidden(&self, paragraph_style: Option<&str>, format: &RunPatch) -> bool {
        match format.hidden {
            Some(hidden) => hidden,
            None if !self.styled => false,
            None => {
                self.sheet
                    .run(&self.theme, None, paragraph_style, format)
                    .hidden
            }
        }
    }
}

const WORDPROCESSING: [&[u8]; 2] = [
    b"http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    b"http://purl.oclc.org/ooxml/wordprocessingml/main",
];

/// The namespace prefixes `element` declares, each with whether it binds
/// WordprocessingML; the default namespace has the empty prefix.
fn declared(element: &BytesStart<'_>) -> Vec<(Vec<u8>, bool)> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .filter_map(|a| {
            let key = a.key.as_ref();
            let prefix = match key.strip_prefix(b"xmlns") {
                Some(b"") => Vec::new(),
                Some(rest) => rest.strip_prefix(b":")?.to_vec(),
                None => return None,
            };
            Some((prefix, WORDPROCESSING.contains(&a.value.as_ref())))
        })
        .collect()
}

/// Whether `element`, opening in `stack`, is in the WordprocessingML
/// namespace, which a file may bind to any prefix.
fn in_word(stack: &[Open], element: &BytesStart<'_>, declares: &[(Vec<u8>, bool)]) -> bool {
    let name = element.name();
    let prefix = prefix(name.as_ref());
    declares
        .iter()
        .chain(stack.iter().rev().flat_map(|o| o.declares.iter()))
        .find(|(p, _)| p == prefix)
        .is_some_and(|(_, word)| *word)
}

fn is_word(frame: &Open, local_name: &[u8]) -> bool {
    frame.word && local(&frame.name) == local_name
}

/// `ST_OnOff`, where an element without a value means on.
fn on(element: &BytesStart<'_>) -> bool {
    !element
        .attributes()
        .with_checks(false)
        .flatten()
        .any(|a| local(a.key.as_ref()) == b"val" && matches!(&*a.value, b"0" | b"false" | b"off"))
}

fn value(element: &BytesStart<'_>) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|a| local(a.key.as_ref()) == b"val")
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// Record on the run or paragraph it formats what an element opening in
/// `stack` says of hidden text and tracked changes: a run's or a mark's
/// `w:rStyle` and `w:vanish`, a mark's `w:del` or `w:ins`, a paragraph's
/// `w:pStyle`. A former formatting, under `w:rPrChange`, says nothing.
fn format(stack: &mut [Open], element: &BytesStart<'_>, word: bool) {
    if !word {
        return;
    }
    let name = element.name();
    let name = name.as_ref();
    let n = stack.len();
    let at = |k: usize, local_name: &[u8]| n >= k && is_word(&stack[n - k], local_name);
    if at(1, b"pPr") && at(2, b"p") && local(name) == b"pStyle" {
        stack[n - 2].paragraph_style = value(element);
        return;
    }
    if !at(1, b"rPr") {
        return;
    }
    let target = match (at(2, b"r"), at(2, b"pPr") && at(3, b"p")) {
        (true, _) => n - 2,
        (_, true) => n - 3,
        _ => return,
    };
    let frame = &mut stack[target];
    match local(name) {
        b"rStyle" => frame.format.style = value(element),
        b"vanish" | b"specVanish" => frame.format.hidden = Some(on(element)),
        b"del" | b"moveFrom" if target == n - 3 => frame.mark_deleted = true,
        b"ins" | b"moveTo" if target == n - 3 => frame.mark_inserted = true,
        _ => {}
    }
}

/// The views that show what is read now in `stack`: the text of a run, or a
/// tab or a break it holds.
fn views_in(stack: &[Open], hiding: &Hiding) -> Views {
    let (deleted, inserted) = stack
        .last()
        .map_or((false, false), |o| (o.deleted, o.inserted));
    let run = stack.iter().rposition(|o| is_word(o, b"r"));
    let hidden = run.is_some_and(|run| {
        let paragraph = stack[..run]
            .iter()
            .rev()
            .find(|o| is_word(o, b"p"))
            .and_then(|p| p.paragraph_style.as_deref());
        hiding.hidden(paragraph, &stack[run].format)
    });
    views_showing(deleted, inserted, hidden)
}

/// A tab stop, which breaks a line only where nothing else does: a
/// paragraph mark it follows keeps the views it shows in.
const TAB_STOP: Views = 1 << 7;

/// The views in which an element that ends a line, closing in `stack` or
/// standing empty there, breaks it: every view for a row, a paragraph mark in
/// the views it shows in, a tab or a break in those of its run. `frame` is
/// the element's own, once it has opened.
fn break_views(
    stack: &[Open],
    name: &[u8],
    word: bool,
    frame: Option<&Open>,
    hiding: &Hiding,
) -> Views {
    let parent = stack.last();
    match local(name) {
        b"p" if word => {
            let (deleted, inserted) = parent.map_or((false, false), |o| (o.deleted, o.inserted));
            let format = frame.map(|p| &p.format).cloned().unwrap_or_default();
            let hidden = hiding.hidden(frame.and_then(|p| p.paragraph_style.as_deref()), &format);
            views_showing(
                deleted || frame.is_some_and(|p| p.mark_deleted),
                inserted || frame.is_some_and(|p| p.mark_inserted),
                hidden,
            )
        }
        b"tab" | b"br" | b"cr" if word && parent.is_some_and(|o| is_word(o, b"r")) => {
            views_in(stack, hiding)
        }
        b"tab" if word => TAB_STOP,
        _ => EVERY_VIEW,
    }
}

/// The text nodes of a part that carries prose, and in any part the attribute
/// values [`carries_prose`] or [`is_author_attribute`] names.
fn scan(entry: usize, name: &str, part: &[u8], prose: bool, hiding: &Hiding) -> Result<Part> {
    let mut texts: Vec<TextNode> = Vec::new();
    let mut values: Vec<TextNode> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let mut markup: Vec<(u32, u32)> = Vec::new();
    let mut value_markup: Vec<(u32, u32)> = Vec::new();
    let mut error = None;
    walk(part, |step| {
        if error.is_some() {
            return;
        }
        let breaks: Views = match step {
            Step::Text { node, text, span } => {
                if prose {
                    let enclosing = stack.last();
                    if enclosing
                        .is_some_and(|o| matches!(local(&o.name), b"instrText" | b"delInstrText"))
                    {
                        let chars = text.chars().zip(0..).map(|(c, offset)| (c, node, offset));
                        match fields.last_mut() {
                            Some(field) if !field.separated => field.instruction.extend(chars),
                            _ => Field {
                                separated: false,
                                instruction: chars.collect(),
                            }
                            .markup(&mut markup),
                        }
                    }
                    texts.push(TextNode {
                        span,
                        text: text.to_string(),
                        views: views_in(&stack, hiding),
                        breaks_after: 0,
                        preserve_at: enclosing.and_then(|o| o.preserve_at),
                        scrub: enclosing.is_some_and(|o| o.scrub),
                        path: false,
                    });
                }
                0
            }
            Step::Open { element, empty } => {
                if prose && local(element.name().as_ref()) == b"fldChar" {
                    let kind = element
                        .attributes()
                        .with_checks(false)
                        .flatten()
                        .find(|a| local(a.key.as_ref()) == b"fldCharType")
                        .map(|a| a.value.into_owned());
                    match kind.as_deref() {
                        Some(b"begin") => fields.push(Field::default()),
                        Some(b"separate") => {
                            if let Some(field) = fields.last_mut().filter(|f| !f.separated) {
                                field.separated = true;
                                field.markup(&mut markup);
                            }
                        }
                        Some(b"end") => {
                            if let Some(field) = fields.pop().filter(|f| !f.separated) {
                                field.markup(&mut markup);
                            }
                        }
                        _ => {}
                    }
                }
                let declares = declared(element);
                let word = in_word(&stack, element, &declares);
                if prose {
                    format(&mut stack, element, word);
                }
                let breaks = match empty && ends_a_paragraph(element.name().as_ref()) {
                    true => break_views(&stack, element.name().as_ref(), word, None, hiding),
                    false => 0,
                };
                let namespace = (word, declares);
                match open(
                    name,
                    part,
                    &stack,
                    element,
                    namespace,
                    &mut values,
                    &mut value_markup,
                ) {
                    Ok(frame) if !empty => stack.push(frame),
                    Ok(_) => {}
                    Err(e) => error = Some(e),
                }
                breaks
            }
            Step::Close(element) => {
                let name = element.name();
                let breaks = match (ends_a_paragraph(name.as_ref()), stack.split_last()) {
                    (true, Some((frame, outer))) => {
                        break_views(outer, name.as_ref(), frame.word, Some(frame), hiding)
                    }
                    (true, None) => EVERY_VIEW,
                    _ => 0,
                };
                stack.pop();
                breaks
            }
        };
        if let Some(last) = texts.last_mut() {
            last.breaks_after |= match (breaks, last.breaks_after) {
                (TAB_STOP, 0) => EVERY_VIEW,
                (TAB_STOP, _) => 0,
                (breaks, _) => breaks,
            };
        }
    })?;
    if let Some(e) = error {
        return Err(e);
    }
    for field in fields.iter().filter(|f| !f.separated) {
        field.markup(&mut markup);
    }
    let first = texts.len() as u32;
    markup.extend(value_markup.into_iter().map(|(v, o)| (first + v, o)));
    Ok(Part {
        entry,
        texts,
        values,
        markup,
    })
}

fn open(
    part_name: &str,
    part: &[u8],
    stack: &[Open],
    element: &BytesStart<'_>,
    namespace: (bool, Vec<(Vec<u8>, bool)>),
    values: &mut Vec<TextNode>,
    markup: &mut Vec<(u32, u32)>,
) -> Result<Open> {
    let name = element.name().as_ref().to_vec();
    let attributes: Vec<_> = element
        .attributes()
        .with_checks(false)
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("reading an attribute: {e}"))?;
    let external = attributes
        .iter()
        .any(|a| a.key.as_ref() == b"TargetMode" && a.value.as_ref() == b"External");
    let template = attributes
        .iter()
        .any(|a| a.key.as_ref() == b"Type" && a.value.ends_with(b"/attachedTemplate"));
    let mut preserves = false;
    for attribute in &attributes {
        let key = attribute.key.as_ref();
        preserves |= key == b"xml:space";
        let scrub = is_author_attribute(&name, key);
        let target = key == b"Target" && (external || template);
        if !scrub && !target && !carries_prose(stack, &name, key) {
            continue;
        }
        let raw = attribute.value.as_ref();
        let start = offset_in(part, raw)?;
        let text = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| anyhow!("reading an attribute value: {e}"))?;
        if text.is_empty() {
            continue;
        }
        let node = values.len() as u32;
        markup.extend(
            value_markup(stack, &name, key, &text)
                .into_iter()
                .map(|at| (node, at as u32)),
        );
        values.push(TextNode {
            span: start..start + raw.len(),
            path: target && (template || is_local_file(&text)),
            text: text.into_owned(),
            views: EVERY_VIEW,
            breaks_after: EVERY_VIEW,
            preserve_at: None,
            scrub,
        });
    }
    let preserve_at = match strips_blanks(&name) && !preserves {
        true => Some(offset_in(part, element.as_ref())? + name.len()),
        false => None,
    };
    let author_property = part_name == "docProps/custom.xml"
        && local(&name) == b"property"
        && attributes.iter().any(|a| {
            a.key.as_ref() == b"name" && is_author_property(&String::from_utf8_lossy(&a.value))
        });
    let scrub = names_an_author(part_name, &name)
        || author_property
        || stack.last().is_some_and(|o| o.scrub);
    let revision = local(&name);
    let (word, declares) = namespace;
    Ok(Open {
        word,
        declares,
        preserve_at,
        scrub,
        deleted: stack.last().is_some_and(|o| o.deleted)
            || word && matches!(revision, b"del" | b"moveFrom"),
        inserted: stack.last().is_some_and(|o| o.inserted)
            || word && matches!(revision, b"ins" | b"moveTo"),
        format: RunPatch::default(),
        paragraph_style: None,
        mark_deleted: false,
        mark_inserted: false,
        name,
    })
}

/// The target of the first relationship in the `.rels` entry `rels` whose type
/// ends in `kind`, as an entry name.
fn related(entries: &[(String, Vec<u8>)], rels: &str, kind: &[u8]) -> Option<String> {
    let (_, data) = entries.iter().find(|(name, _)| name == rels)?;
    let mut reader = Reader::from_reader(data.strip_prefix(BOM).unwrap_or(data));
    loop {
        let element = match reader.read_event().ok()? {
            Event::Eof => return None,
            Event::Empty(e) | Event::Start(e) => e,
            _ => continue,
        };
        let attribute = |key: &[u8]| {
            element
                .attributes()
                .with_checks(false)
                .flatten()
                .find(|a| a.key.as_ref() == key)
                .map(|a| a.value.into_owned())
        };
        if attribute(b"Type").is_some_and(|t| t.ends_with(kind)) {
            let target = attribute(b"Target")?;
            return Some(resolve(rels, &String::from_utf8_lossy(&target)));
        }
    }
}

/// What the styles of the main document of a word-processing package say of
/// hidden text; none in any other package. Styles that do not parse hide
/// nothing, as a part that does not parse is read as bytes.
fn hiding(entries: &[(String, Vec<u8>)]) -> Hiding {
    let main = related(entries, "_rels/.rels", b"/officeDocument")
        .unwrap_or_else(|| "word/document.xml".to_string());
    let (dir, file) = main.rsplit_once('/').unwrap_or(("", &main));
    let rels = match dir {
        "" => format!("_rels/{file}.rels"),
        dir => format!("{dir}/_rels/{file}.rels"),
    };
    let styles = related(entries, &rels, b"/styles")
        .and_then(|name| entries.iter().find(|(n, _)| *n == name))
        .map(|(_, data)| data.as_slice());
    Hiding {
        sheet: styles
            .and_then(|data| StyleSheet::read(data).ok())
            .unwrap_or_default(),
        theme: Theme::default(),
        styled: styles.is_some_and(|data| {
            [b"vanish", b"Vanish"]
                .iter()
                .any(|word| memchr::memmem::find(data, *word).is_some())
        }),
    }
}

/// A relationship target as a package entry name, from the part its `.rels`
/// describes.
fn resolve(rels: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let dir = rels.split("_rels/").next().unwrap_or_default();
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Cut from a content-type map or a `.rels` part every element that points
/// at an entry `gone` drops. `None` when nothing points at one.
fn cut_references(name: &str, part: &[u8], gone: &dyn Fn(&str) -> bool) -> Result<Option<Vec<u8>>> {
    let bom = if part.starts_with(BOM) { BOM.len() } else { 0 };
    let mut reader = Reader::from_reader(&part[bom..]);
    let mut cuts: Vec<Range<usize>> = Vec::new();
    loop {
        let start = bom + reader.buffer_position() as usize;
        let element = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Empty(e)) => e,
            Ok(_) => continue,
            Err(e) => return Err(anyhow!("reading {name}: {e}")),
        };
        let end = bom + reader.buffer_position() as usize;
        let target = element
            .attributes()
            .with_checks(false)
            .flatten()
            .find_map(|a| match (local(element.name().as_ref()), a.key.as_ref()) {
                (b"Override", b"PartName") => Some(
                    String::from_utf8_lossy(&a.value)
                        .trim_start_matches('/')
                        .to_string(),
                ),
                (b"Relationship", b"Target") => {
                    Some(resolve(name, &String::from_utf8_lossy(&a.value)))
                }
                _ => None,
            });
        if target.is_some_and(|t| gone(&t)) {
            cuts.push(start..end);
        }
    }
    if cuts.is_empty() {
        return Ok(None);
    }
    let mut out = part.to_vec();
    for cut in cuts.into_iter().rev() {
        out.drain(cut);
    }
    Ok(Some(out))
}

/// `text` escaped for a text node, or for an attribute value, where a raw
/// line break or tab would read back as a space.
fn escaped(text: &str, attribute: bool) -> Vec<u8> {
    let escaped = quick_xml::escape::escape(text);
    match attribute {
        false => escaped.into_owned().into_bytes(),
        true => escaped
            .replace('\n', "&#10;")
            .replace('\r', "&#13;")
            .replace('\t', "&#9;")
            .into_bytes(),
    }
}

/// Apply byte edits to a part, each a range and what replaces it, in one pass
/// over the part.
fn splice(bytes: &mut Vec<u8>, mut edits: Vec<(Range<usize>, Vec<u8>)>) {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    edits.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let mut out = Vec::with_capacity(bytes.len());
    let mut copied = 0;
    for (range, with) in edits {
        out.extend_from_slice(&bytes[copied..range.start]);
        out.extend_from_slice(&with);
        copied = range.end;
    }
    out.extend_from_slice(&bytes[copied..]);
    *bytes = out;
}

/// What a package may unpack to, every entry together: well above a real
/// document, where a zip bomb unpacks to whatever it claims.
pub const MAX_UNPACKED: u64 = 512 << 20;

/// Entries a package may hold, which keeps a part number within the `u16` an
/// anchor carries.
pub const MAX_ENTRIES: usize = 20_000;

/// Characters a package may carry, text and rewritten attributes together,
/// which bounds the memory from opening to redaction.
pub const MAX_CHARACTERS: usize = 4 << 20;

/// Every entry of a ZIP package, in order, refused past `left` bytes unpacked.
fn unpack(bytes: &[u8], left: &mut u64) -> Result<Vec<(String, Vec<u8>)>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("reading the package")?;
    if archive.len() > MAX_ENTRIES {
        return Err(anyhow!(
            "the package holds {} entries, more than {MAX_ENTRIES}",
            archive.len()
        ));
    }
    let mut entries = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let file = archive.by_index(i).context("reading a package entry")?;
        let name = file.name().to_string();
        let mut data = Vec::with_capacity(file.size().min(*left) as usize);
        let read = file
            .take(left.saturating_add(1))
            .read_to_end(&mut data)
            .context("reading a package entry")? as u64;
        if read > *left {
            return Err(anyhow!(
                "the package unpacks to more than {} MiB",
                MAX_UNPACKED >> 20
            ));
        }
        *left -= read;
        entries.push((name, data));
    }
    Ok(entries)
}

impl OoxmlPackage {
    /// Read the package and walk every part that can carry prose.
    pub fn open(bytes: &[u8]) -> Result<Self> {
        let mut package = Self::read(bytes, 0, &mut MAX_UNPACKED.clone())?;
        if package.part_count() > u16::MAX as usize {
            bail!("the package has more parts than an anchor can number");
        }
        let characters = package.character_count();
        if characters > MAX_CHARACTERS {
            bail!("the package carries {characters} characters, more than {MAX_CHARACTERS}");
        }
        package.edits = vec![BTreeMap::new(); package.part_count()];
        Ok(package)
    }

    /// `left` is what the package may still unpack to, nested packages
    /// included.
    fn read(bytes: &[u8], depth: usize, left: &mut u64) -> Result<Self> {
        let entries = unpack(bytes, left)?;
        let hiding = hiding(&entries);

        let mut parts = Vec::new();
        let mut nested = Vec::new();
        let mut binaries = Vec::new();
        for (i, (name, data)) in entries.iter().enumerate() {
            if dropped(name) {
                binaries.push(i);
                continue;
            }
            let prose = carries_text(name);
            if prose || name.ends_with(".rels") {
                // A part other than the body that does not parse is erased as bytes.
                match scan(i, name, data, prose, &hiding) {
                    Ok(part) => parts.push(part),
                    Err(e) if name == "word/document.xml" => {
                        return Err(e.context(format!("reading {name}")))
                    }
                    Err(_) => binaries.push(i),
                }
                continue;
            }
            if name.ends_with('/') || name == "[Content_Types].xml" {
                continue;
            }
            if depth < NESTING && data.starts_with(b"PK\x03\x04") {
                if let Ok(package) = Self::read(data, depth + 1, left) {
                    nested.push((i, package));
                    continue;
                }
            }
            binaries.push(i);
        }
        Ok(Self {
            entries,
            parts,
            nested,
            binaries,
            edits: Vec::new(),
            clear_authors: false,
            drop_thumbnails: false,
        })
    }

    fn character_count(&self) -> usize {
        let own: usize = self
            .parts
            .iter()
            .flat_map(Part::nodes)
            .map(|node| node.text.chars().count())
            .sum();
        own + self
            .nested
            .iter()
            .map(|(_, p)| p.character_count())
            .sum::<usize>()
    }

    /// Parts of this package and of every package nested in it, which is the
    /// range the part number of an anchor spans.
    fn part_count(&self) -> usize {
        self.parts.len()
            + self
                .nested
                .iter()
                .map(|(_, p)| p.part_count())
                .sum::<usize>()
    }

    /// A part that was walked for text: the number its anchors carry, and its
    /// bytes, for a reader that walks it again with [`walk`].
    pub fn part(&self, name: &str) -> Option<(u16, &[u8])> {
        self.parts
            .iter()
            .position(|part| self.entries[part.entry].0 == name)
            .map(|part| {
                (
                    part as u16,
                    self.entries[self.parts[part].entry].1.as_slice(),
                )
            })
    }

    /// Any entry of the package by name, relationships and media included.
    pub fn entry(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, data)| data.as_slice())
    }

    /// Every character of every scanned part, in package order, each anchored.
    /// A newline separates parts and paragraphs, and nothing separates the runs
    /// inside a paragraph, which is what makes a split name matchable. After
    /// the text of a part come its attribute values that carry prose or name
    /// someone, one per line: an alternative text, a tooltip, a link target.
    pub fn characters(&self) -> Vec<(char, Anchor)> {
        let mut out: Vec<(char, Anchor)> = Vec::new();
        self.feed(0, &mut out, &mut Vec::new());
        out
    }

    /// For each character of [`Self::characters`], the views that show it.
    pub fn views(&self) -> Vec<Views> {
        let mut views = Vec::new();
        self.feed(0, &mut Vec::new(), &mut views);
        views
    }

    /// The characters of [`Self::characters`] that are the keyword or a switch
    /// of a field instruction, or a bookmark or a drawing name.
    pub fn markup(&self) -> HashSet<Anchor> {
        let mut out = HashSet::new();
        self.collect_markup(0, &mut out);
        out
    }

    fn collect_markup(&self, base: usize, out: &mut HashSet<Anchor>) {
        for (i, part) in self.parts.iter().enumerate() {
            out.extend(part.markup.iter().map(|&(node, offset)| Anchor::XmlText {
                part: (base + i) as u16,
                node,
                offset,
            }));
        }
        let mut base = base + self.parts.len();
        for (_, package) in &self.nested {
            package.collect_markup(base, out);
            base += package.part_count();
        }
    }

    /// The text nodes then the attribute values of every part, the parts
    /// numbered from `base` on, and the views that show each character.
    fn feed(&self, base: usize, out: &mut Vec<(char, Anchor)>, views: &mut Vec<Views>) {
        for (i, part) in self.parts.iter().enumerate() {
            if part.texts.is_empty() && part.values.is_empty() {
                continue;
            }
            if let Some(last) = out.last().map(|(_, a)| *a) {
                out.push(('\n', last));
                views.push(EVERY_VIEW);
            }
            for (node, entry) in part.nodes().enumerate() {
                if node == part.texts.len() {
                    if let Some(&(_, last)) = out.last().filter(|(c, _)| *c != '\n') {
                        out.push(('\n', last));
                        views.push(EVERY_VIEW);
                    }
                }
                for (offset, c) in entry.text.chars().enumerate() {
                    let anchor = Anchor::XmlText {
                        part: (base + i) as u16,
                        node: node as u32,
                        offset: offset as u32,
                    };
                    out.push((c, anchor));
                    views.push(entry.views);
                }
                if entry.breaks_after != 0 {
                    if let Some(last) = out.last().map(|(_, a)| *a) {
                        out.push(('\n', last));
                        views.push(entry.breaks_after);
                    }
                }
            }
        }
        let mut base = base + self.parts.len();
        for (_, package) in &self.nested {
            package.feed(base, out, views);
            base += package.part_count();
        }
    }

    /// Every entry no XML reader walks, a picture's metadata or an OLE
    /// object's stream, nested packages included, by name.
    pub fn binaries(&self) -> Vec<(&str, &[u8])> {
        let mut out: Vec<(&str, &[u8])> = self
            .binaries
            .iter()
            .map(|&entry| {
                (
                    self.entries[entry].0.as_str(),
                    self.entries[entry].1.as_slice(),
                )
            })
            .collect();
        for (_, package) in &self.nested {
            out.extend(package.binaries());
        }
        out
    }

    /// Rewrite every entry [`Self::binaries`] lists, in place.
    pub fn rewrite_binaries(&mut self, rewrite: &mut dyn FnMut(&mut Vec<u8>)) {
        for &entry in &self.binaries {
            rewrite(&mut self.entries[entry].1);
        }
        for (_, package) in &mut self.nested {
            package.rewrite_binaries(rewrite);
        }
    }

    /// Record `replacements`, which [`Self::save`] applies, each touched node
    /// rewritten once. Returns how many characters go.
    pub fn replace(&mut self, replacements: &[Replacement]) -> Result<usize> {
        let nodes: Vec<usize> = self.node_counts();
        record(replacements, &mut self.edits, |part| nodes[part])
    }

    /// Empty the author metadata, and cut the folders from a path naming the
    /// author's machine, when [`Self::save`] runs.
    pub fn clear_authors(&mut self) {
        self.clear_authors = true;
    }

    /// Drop the thumbnails and the signatures, and what points at them, when
    /// [`Self::save`] runs.
    pub fn drop_thumbnails(&mut self) {
        self.drop_thumbnails = true;
    }

    /// How many text nodes each part holds, nested packages included, which is
    /// the range the node number of an anchor spans.
    fn node_counts(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .parts
            .iter()
            .map(|part| part.texts.len() + part.values.len())
            .collect();
        for (_, package) in &self.nested {
            out.extend(package.node_counts());
        }
        out
    }

    fn apply(
        &mut self,
        edits: &[BTreeMap<u32, NodeEdit>],
        clear_authors: bool,
        drop_thumbnails: bool,
    ) -> Result<()> {
        for (part, edits) in self.parts.iter().zip(edits) {
            let mut splices = Vec::new();
            for (n, node) in part.nodes().enumerate() {
                let edit = edits.get(&(n as u32));
                let scrub = clear_authors && node.scrub;
                let path = clear_authors && node.path;
                if edit.is_none() && !scrub && !path {
                    continue;
                }
                let mut kept: String = match (scrub, edit) {
                    (true, _) => String::new(),
                    (false, Some(edit)) => edit.apply(&node.text),
                    (false, None) => node.text.clone(),
                };
                if path {
                    kept = kept.chars().skip(directory_len(&kept)).collect();
                }
                if kept == node.text {
                    continue;
                }
                let blank_edge =
                    kept.starts_with(char::is_whitespace) || kept.ends_with(char::is_whitespace);
                if let (Some(at), true) = (node.preserve_at, blank_edge) {
                    splices.push((at..at, PRESERVE.to_vec()));
                }
                splices.push((node.span.clone(), escaped(&kept, n >= part.texts.len())));
            }
            if !splices.is_empty() {
                splice(&mut self.entries[part.entry].1, splices);
            }
        }

        let mut base = self.parts.len();
        for (entry, package) in &mut self.nested {
            let count = package.part_count();
            package.apply(&edits[base..base + count], clear_authors, drop_thumbnails)?;
            self.entries[*entry].1 = package.write()?;
            base += count;
        }

        if !drop_thumbnails {
            return Ok(());
        }
        let gone: BTreeSet<String> = self
            .entries
            .iter()
            .map(|(name, _)| name)
            .filter(|name| dropped(name))
            .cloned()
            .collect();
        if gone.is_empty() {
            return Ok(());
        }
        let is_gone = |name: &str| gone.contains(name);
        for (name, data) in &mut self.entries {
            if gone.contains(name.as_str()) {
                continue;
            }
            if name == "[Content_Types].xml" || name.ends_with(".rels") {
                if let Some(cut) = cut_references(name, data, &is_gone)? {
                    *data = cut;
                }
            }
        }
        self.drop_entries(&gone);
        Ok(())
    }

    /// Remove entries by name, keeping every index this package holds valid.
    fn drop_entries(&mut self, gone: &BTreeSet<String>) {
        let kept: Vec<Option<usize>> = {
            let mut next = 0;
            self.entries
                .iter()
                .map(|(name, _)| match gone.contains(name) {
                    true => None,
                    false => {
                        next += 1;
                        Some(next - 1)
                    }
                })
                .collect()
        };
        self.entries.retain(|(name, _)| !gone.contains(name));
        for part in &mut self.parts {
            part.entry = kept[part.entry].expect("a dropped entry is never a part");
        }
        self.nested.retain_mut(|(entry, _)| match kept[*entry] {
            Some(at) => {
                *entry = at;
                true
            }
            None => false,
        });
        self.binaries = self
            .binaries
            .iter()
            .filter_map(|entry| kept[*entry])
            .collect();
    }

    /// Apply what was recorded, then write the package back, every entry in
    /// its original order and name.
    pub fn save(mut self) -> Result<Vec<u8>> {
        let edits = std::mem::take(&mut self.edits);
        self.apply(&edits, self.clear_authors, self.drop_thumbnails)?;
        self.write()
    }

    fn write(&self) -> Result<Vec<u8>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &self.entries {
            writer
                .start_file(name, options)
                .context("writing a package entry")?;
            writer.write_all(data).context("writing a package entry")?;
        }
        Ok(writer.finish().context("closing the package")?.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn docx(body: &str) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="urn:w"><w:body><w:p>{body}</w:p></w:body></w:document>"#
        );
        package(&[
            ("[Content_Types].xml", b"<Types/>".to_vec()),
            ("_rels/.rels", b"<Relationships/>".to_vec()),
            ("word/document.xml", document.into_bytes()),
        ])
    }

    fn anchors_of(feed: &[(char, Anchor)], word: &str) -> Vec<Anchor> {
        let chars: Vec<char> = feed.iter().map(|(c, _)| *c).collect();
        let word: Vec<char> = word.chars().collect();
        let at = chars
            .windows(word.len())
            .position(|w| w == word.as_slice())
            .unwrap();
        feed[at..at + word.len()].iter().map(|(_, a)| *a).collect()
    }

    #[test]
    fn a_replacement_lands_where_its_first_character_stood_and_the_rest_goes() {
        let source = docx(
            r#"<w:r><w:t>Maitre Du</w:t></w:r><w:r><w:t>pont &amp; associes</w:t></w:r><w:hyperlink w:tooltip="Ecrire a Dupont"/>"#,
        );
        let mut package = OoxmlPackage::open(&source).unwrap();
        let replacements = [
            Replacement {
                anchors: anchors_of(&package.characters(), "Dupont"),
                text: "[A]".into(),
            },
            Replacement {
                anchors: anchors_of(&package.characters(), "a Dupont")[2..].to_vec(),
                text: "[B] & co".into(),
            },
        ];
        assert_eq!(package.replace(&replacements).unwrap(), 12);
        let out = OoxmlPackage::open(&package.save().unwrap()).unwrap();
        let document = String::from_utf8_lossy(out.entry("word/document.xml").unwrap());
        for kept in [
            "<w:t>Maitre [A]</w:t>",
            r#"<w:t xml:space="preserve"> &amp; associes</w:t>"#,
            r#"w:tooltip="Ecrire a [B] &amp; co""#,
        ] {
            assert!(document.contains(kept), "{kept} missing from {document}");
        }
    }

    #[test]
    fn a_package_unpacking_past_its_limit_is_refused() {
        let source = docx(&"<w:r><w:t>Dupont</w:t></w:r>".repeat(1000));
        let unpacked: u64 = unpack(&source, &mut MAX_UNPACKED.clone())
            .unwrap()
            .iter()
            .map(|(_, data)| data.len() as u64)
            .sum();
        assert!(unpack(&source, &mut unpacked.clone()).is_ok());
        assert!(unpack(&source, &mut (unpacked - 1)).is_err());
    }
}
