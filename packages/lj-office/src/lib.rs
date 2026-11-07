//! Word documents read into one model, laid out as LibreOffice lays them out,
//! and drawn page by page, every character anchored in the file it came from,
//! and rewritten by edits that name characters by those anchors.

pub mod doc;
mod docx;
mod field;
pub mod fonts;
pub mod html;
pub mod layout;
mod metafile;
pub mod model;
mod numbering;
pub mod odt;
pub mod ooxml;
mod output;

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{bail, Result};

pub use fonts::Fonts;
pub use layout::{Glyphs, Item, Layout, Page};
pub use model::Document;

/// Where one character sits in the file it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// A character in a ZIP-XML package or a web page: which part, which text
    /// node of that part, and the character's offset in the node's unescaped
    /// text. A web page is one part.
    XmlText { part: u16, node: u32, offset: u32 },
    /// A character of a Word 97 binary document: the story it belongs to and
    /// its character position, which alone locates its bytes.
    DocChar { story: u8, cp: u32 },
    /// A character of a string a Word 97 binary document keeps outside its
    /// stories, a document variable or a drawing's alternative text: which
    /// string, in the order the reader lists them, and the offset in it.
    DocValue { value: u32, offset: u32 },
}

/// Characters to replace, and the text that lands where the first of them
/// stood. A character several replacements name goes once.
#[derive(Clone, Debug)]
pub struct Replacement {
    pub anchors: Vec<Anchor>,
    pub text: String,
}

/// What replacements do to one text node: the offsets that go, and the text
/// put before the character at an offset.
#[derive(Clone, Default)]
pub(crate) struct NodeEdit {
    pub removed: BTreeSet<u32>,
    pub inserted: BTreeMap<u32, String>,
}

impl NodeEdit {
    pub fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, c) in text.chars().enumerate() {
            let i = i as u32;
            if let Some(with) = self.inserted.get(&i) {
                out.push_str(with);
            }
            if !self.removed.contains(&i) {
                out.push(c);
            }
        }
        out
    }
}

/// Record `replacements` among a ZIP-XML package's edits, by part then node,
/// and return how many characters go that no earlier replacement named.
pub(crate) fn record(
    replacements: &[Replacement],
    parts: &mut [BTreeMap<u32, NodeEdit>],
    nodes_in: impl Fn(usize) -> usize,
) -> Result<usize> {
    let mut removed = 0;
    for replacement in replacements {
        for (i, anchor) in replacement.anchors.iter().enumerate() {
            let Anchor::XmlText { part, node, offset } = *anchor else {
                bail!("a .doc character anchored in a ZIP-XML package");
            };
            let Some(edits) = parts.get_mut(part as usize) else {
                bail!("an anchor points outside the package");
            };
            if node as usize >= nodes_in(part as usize) {
                bail!("an anchor points outside its part");
            }
            let edit = edits.entry(node).or_default();
            if i == 0 && !replacement.text.is_empty() {
                edit.inserted
                    .entry(offset)
                    .or_default()
                    .push_str(&replacement.text);
            }
            if edit.removed.insert(offset) {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Elements a part may nest, far deeper than a real document: the recursive
/// walks of the model and the layout must fit the 1 MiB stack of a wasm module.
pub(crate) const MAX_DEPTH: usize = 1000;

/// The word-processing formats, told apart by their bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    Odt,
    Doc,
    /// A web page, as Word saves one, often under a `.doc` name.
    Html,
}

impl Format {
    pub fn of(bytes: &[u8]) -> Result<Self> {
        const ZIP: &[u8] = b"PK\x03\x04";
        const OLE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
        if bytes.starts_with(OLE) {
            return Ok(Self::Doc);
        }
        if html::is_html(bytes) {
            return Ok(Self::Html);
        }
        if !bytes.starts_with(ZIP) {
            bail!("not a Word document: neither a ZIP package, an OLE file nor a web page");
        }
        Ok(match opendocument(bytes) {
            true => Self::Odt,
            false => Self::Docx,
        })
    }
}

/// Read a `.docx`, `.odt`, `.doc` or a web page.
pub fn open(bytes: &[u8]) -> Result<Document> {
    match Format::of(bytes)? {
        Format::Docx => docx::read(bytes),
        Format::Odt => Ok(odt::to_model(&odt::read(bytes)?)),
        Format::Doc => {
            let doc = doc::Doc::parse(bytes)?;
            Ok(doc::office::to_model(
                &doc,
                &doc::model::Document::read(&doc),
            ))
        }
        Format::Html => Ok(html::HtmlPackage::open(bytes)?.document()),
    }
}

/// Every character of the file, anchored as the layout draws it and as the
/// edits name it: each text node of each part of a package, each story of a
/// `.doc`, with what no page shows (comments, properties) included.
pub fn characters(bytes: &[u8]) -> Result<Vec<(char, Anchor)>> {
    Ok(match Format::of(bytes)? {
        Format::Docx => ooxml::OoxmlPackage::open(bytes)?.characters(),
        Format::Odt => odt::OdtPackage::open(bytes)?.characters(),
        Format::Doc => doc::package::feed(&doc::Doc::parse(bytes)?),
        Format::Html => html::HtmlPackage::open(bytes)?.characters(),
    })
}

/// The views a reader can have of a document, one bit each: tracked changes
/// shown, accepted or rejected, each with hidden text shown or not. Bit 0,
/// changes and hidden text shown, is the whole of [`characters`].
pub type Views = u8;

/// Every view: what no tracked change and no hidden text touches.
pub const EVERY_VIEW: Views = 0b11_1111;

/// The views that show a character a tracked change deleted, inserted or
/// both, or that is hidden.
pub(crate) fn views_showing(deleted: bool, inserted: bool, hidden: bool) -> Views {
    let mut out = 0;
    for changes in 0..3 {
        for hiding in 0..2 {
            let kept = match changes {
                0 => true,
                1 => !deleted,
                _ => !inserted,
            };
            if kept && (hiding == 0 || !hidden) {
                out |= 1 << (changes * 2 + hiding);
            }
        }
    }
    out
}

/// For each character of [`characters`], the views that show it. A line
/// break a deleted, inserted or hidden paragraph mark stands for leaves the
/// views that join the two paragraphs.
pub fn views(bytes: &[u8]) -> Result<Vec<Views>> {
    Ok(match Format::of(bytes)? {
        Format::Docx => ooxml::OoxmlPackage::open(bytes)?.views(),
        Format::Odt => odt::OdtPackage::open(bytes)?.views()?,
        Format::Doc => doc::package::views(&doc::Doc::parse(bytes)?),
        Format::Html => html::HtmlPackage::open(bytes)?.views(),
    })
}

/// Each view that leaves out a character of `views`, as the positions it
/// keeps, in order, a view equal to an earlier one given once.
pub fn view_positions(views: &[Views]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for view in 1..6 {
        let kept: Vec<usize> = (0..views.len())
            .filter(|&at| views[at] & (1 << view) != 0)
            .collect();
        if kept.len() < views.len() && !out.contains(&kept) {
            out.push(kept);
        }
    }
    out
}

/// The characters of [`characters`] that are markup rather than text: the
/// keyword and switches of a field, the name of a bookmark, a drawing or a style.
pub fn markup(bytes: &[u8]) -> Result<HashSet<Anchor>> {
    Ok(match Format::of(bytes)? {
        Format::Docx => ooxml::OoxmlPackage::open(bytes)?.markup(),
        Format::Odt => odt::OdtPackage::open(bytes)?.markup(),
        Format::Doc => doc::package::markup(&doc::Doc::parse(bytes)?),
        Format::Html => html::HtmlPackage::open(bytes)?.markup(),
    })
}

/// An OpenDocument package names its media type in a `mimetype` entry, which
/// LibreOffice stores first and some producers anywhere.
fn opendocument(bytes: &[u8]) -> bool {
    if leading_mimetype(bytes) {
        return true;
    }
    zip::ZipArchive::new(std::io::Cursor::new(bytes)).is_ok_and(|archive| {
        archive.index_for_name("[Content_Types].xml").is_none()
            && (archive.index_for_name("mimetype").is_some()
                || archive.index_for_name("META-INF/manifest.xml").is_some())
    })
}

fn leading_mimetype(bytes: &[u8]) -> bool {
    let Some(name_length) = bytes.get(26..28) else {
        return false;
    };
    let name_length = u16::from_le_bytes([name_length[0], name_length[1]]) as usize;
    let Some(extra_length) = bytes.get(28..30) else {
        return false;
    };
    let extra_length = u16::from_le_bytes([extra_length[0], extra_length[1]]) as usize;
    let name = bytes.get(30..30 + name_length);
    let data = bytes.get(30 + name_length + extra_length..);
    name == Some(b"mimetype".as_slice())
        && data.is_some_and(|d| d.starts_with(b"application/vnd.oasis.opendocument"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Write;

    use super::*;

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape""#;

    fn package(entries: &[(&str, String)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            zip.start_file(*name, stored).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn key(anchor: Anchor) -> (u16, u32, u32) {
        match anchor {
            Anchor::XmlText { part, node, offset } => (part, node, offset),
            other => panic!("a .docx character anchored as {other:?}"),
        }
    }

    #[test]
    fn every_character_is_anchored_where_the_splice_finds_it() {
        let body = format!(
            r#"<w:document {NS}><w:body>
<w:p><w:r><w:t xml:space="preserve">Maître Dupont &amp; associés, </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>né à Lyon</w:t></w:r><w:r><w:tab/><w:t>&#233;t&#xE9;</w:t></w:r></w:p>
<w:p><w:del w:id="1" w:author="a"><w:r><w:delText>supprimé</w:delText></w:r></w:del><w:ins w:id="2" w:author="a"><w:r><w:t>inséré</w:t></w:r></w:ins></w:p>
<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> REF _Ref1 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Article 3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
<w:p><mc:AlternateContent><mc:Choice Requires="wps"><w:r><w:t>choix</w:t></w:r></mc:Choice><mc:Fallback><w:r><w:t>repli</w:t></w:r></mc:Fallback></mc:AlternateContent><w:hyperlink r:id="rId9"><w:r><w:t>lien</w:t></w:r></w:hyperlink><w:sdt><w:sdtContent><w:r><w:t>contrôle</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cellule</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>appel</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>
<w:sectPr><w:headerReference w:type="default" r:id="rId2"/></w:sectPr>
</w:body></w:document>"#
        );
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let bytes = package(&[
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#
                    .into(),
            ),
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/></Relationships>"#
                ),
            ),
            (
                "word/_rels/document.xml.rels",
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="{rel}/header" Target="header1.xml"/><Relationship Id="rId9" Type="{rel}/hyperlink" Target="https://example.org/" TargetMode="External"/><Relationship Id="rId3" Type="{rel}/footnotes" Target="footnotes.xml"/></Relationships>"#
                ),
            ),
            (
                "word/header1.xml",
                format!(
                    r#"<w:hdr {NS}><w:p><w:r><w:t>En-tête &lt;confidentiel&gt;</w:t></w:r></w:p></w:hdr>"#
                ),
            ),
            (
                "word/footnotes.xml",
                format!(
                    r#"<w:footnotes {NS}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Note de Me Durand</w:t></w:r></w:p></w:footnote></w:footnotes>"#
                ),
            ),
            ("word/document.xml", body),
        ]);

        let feed = ooxml::OoxmlPackage::open(&bytes).unwrap().characters();
        let ours: Vec<(char, Anchor)> = open(&bytes)
            .unwrap()
            .characters()
            .into_iter()
            .filter(|(c, _)| *c != '\n')
            .collect();
        let text: String = ours.iter().map(|(c, _)| *c).collect();
        for expected in [
            "Maître Dupont & associés, né à Lyonété",
            "suppriméinséré",
            "Article 3",
            "lien",
            "contrôle",
            "cellule",
            "En-tête <confidentiel>",
            "Note de Me Durand",
        ] {
            assert!(
                text.contains(expected),
                "{expected:?} missing from {text:?}"
            );
        }
        let mut seen = HashSet::new();
        for (c, anchor) in ours {
            assert!(seen.insert(key(anchor)), "{c:?} shares its anchor");
            let found = feed
                .iter()
                .find(|(f, a)| *f != '\n' && key(*a) == key(anchor))
                .map(|(f, _)| *f);
            assert_eq!(found, Some(c), "{anchor:?}");
        }
    }

    fn docx(body: &str, styles: &str) -> Vec<u8> {
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        package(&[
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#
                    .into(),
            ),
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/></Relationships>"#
                ),
            ),
            (
                "word/_rels/document.xml.rels",
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="{rel}/styles" Target="styles.xml"/></Relationships>"#
                ),
            ),
            (
                "word/styles.xml",
                format!(r#"<w:styles {NS}>{styles}</w:styles>"#),
            ),
            (
                "word/document.xml",
                format!(r#"<w:document {NS}><w:body>{body}</w:body></w:document>"#),
            ),
        ])
    }

    #[test]
    fn an_index_result_drops_the_character_style_down_its_nested_fields_until_it_ends() {
        let field = |instruction: &str, result: &str| {
            format!(
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
            )
        };
        let strong = |text: &str| {
            format!(r#"<w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr><w:t>{text}</w:t></w:r>"#)
        };
        let reference = field(" PAGEREF _Toc1 \\h ", &strong("b"));
        let index = field(
            " TOC \\o ",
            &format!("{}{reference}{}", strong("a"), strong("c")),
        );
        let bytes = docx(
            &format!("<w:p>{index}{}</w:p>", strong("d")),
            r#"<w:style w:type="character" w:styleId="Strong"><w:rPr><w:b/></w:rPr></w:style>"#,
        );

        let document = open(&bytes).unwrap();
        let model::Block::Paragraph(paragraph) = &document.sections[0].blocks[0] else {
            panic!("the first block is a paragraph");
        };
        let bold: Vec<(&str, bool)> = paragraph
            .inlines
            .iter()
            .filter_map(|inline| match inline {
                model::Inline::Text(t) => Some((t.text.as_str(), t.props.bold)),
                _ => None,
            })
            .collect();
        assert_eq!(
            bold,
            [("a", false), ("b", false), ("c", false), ("d", true)]
        );
    }

    #[test]
    fn fields_opened_and_never_closed_read_in_linear_time() {
        let open_page = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
        let bytes = docx(&format!("<w:p>{}</w:p>", open_page.repeat(20_000)), "");
        let started = std::time::Instant::now();
        open(&bytes).unwrap();
        // A loose bound, which only a quadratic reading of the open fields exceeds.
        assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
    }

    /// Paragraphs alike in `w:framePr` float in one text frame the next
    /// paragraph anchors, as LibreOffice converts them.
    #[test]
    fn paragraphs_sharing_a_frame_float_in_one_the_next_paragraph_anchors() {
        let framed = |text: &str| {
            format!(
                r#"<w:p><w:pPr><w:framePr w:wrap="around" w:hAnchor="margin" w:xAlign="right" w:y="1"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
            )
        };
        let bytes = docx(
            &format!(
                "<w:sdt><w:sdtContent>{}{}</w:sdtContent></w:sdt><w:p><w:r><w:t>c</w:t></w:r></w:p>",
                framed("a"),
                framed("b")
            ),
            "",
        );
        let document = open(&bytes).unwrap();
        let blocks = &document.sections[0].blocks;
        let [model::Block::Paragraph(anchor)] = &blocks[..] else {
            panic!("one paragraph anchors the frame");
        };
        let model::Inline::Drawing(frame) = &anchor.inlines[0] else {
            panic!("the frame first");
        };
        let float = frame.float.as_deref().expect("a floating frame");
        assert_eq!(float.horizontal.from, model::Relative::Margin);
        assert_eq!(float.horizontal.place, model::Place::End);
        assert_eq!(frame.shape.first_story().map(<[_]>::len), Some(2));
        assert!(matches!(&anchor.inlines[1], model::Inline::Text(t) if t.text == "c"));
    }

    /// A paragraph mark within a field code is hidden with the code, the
    /// paragraph it closes joined to the next.
    #[test]
    fn a_paragraph_mark_within_a_field_code_joins_the_next_paragraph() {
        let char = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
        let bytes = docx(
            &format!(
                r#"<w:p><w:r><w:t>a</w:t></w:r>{}<w:r><w:instrText>ADDRESSBLOCK</w:instrText></w:r></w:p><w:p><w:r><w:instrText> </w:instrText></w:r></w:p><w:p>{}<w:r><w:t>b</w:t></w:r>{}</w:p><w:p><w:r><w:t>c</w:t></w:r></w:p>"#,
                char("begin"),
                char("separate"),
                char("end")
            ),
            "",
        );
        let document = open(&bytes).unwrap();
        let texts: Vec<Vec<&str>> = document.sections[0]
            .blocks
            .iter()
            .map(|b| match b {
                model::Block::Paragraph(p) => p
                    .inlines
                    .iter()
                    .filter_map(|i| match i {
                        model::Inline::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect(),
                model::Block::Table(_) => panic!("paragraphs only"),
            })
            .collect();
        assert_eq!(texts, [vec!["a", "b"], vec!["c"]]);
    }

    /// A paragraph held by an open field code stays before the table that
    /// follows it rather than joining the table's first cell.
    #[test]
    fn a_held_paragraph_stays_before_the_next_table() {
        let char = |kind: &str| format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#);
        let bytes = docx(
            &format!(
                r#"<w:p><w:r><w:t>a</w:t></w:r>{}<w:r><w:instrText>ADDRESSBLOCK</w:instrText></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p>{}<w:r><w:t>b</w:t></w:r>{}</w:p>"#,
                char("begin"),
                char("separate"),
                char("end")
            ),
            "",
        );
        let document = open(&bytes).unwrap();
        let blocks = &document.sections[0].blocks;
        let model::Block::Paragraph(first) = &blocks[0] else {
            panic!("the held paragraph first");
        };
        assert!(matches!(&first.inlines[0], model::Inline::Text(t) if t.text == "a"));
        assert!(matches!(&blocks[1], model::Block::Table(_)));
    }

    /// Word's automatic spacing, 14 pt, is dropped at the start of a table
    /// cell, as LibreOffice imports it.
    #[test]
    fn a_table_cell_drops_automatic_spacing_at_its_start() {
        let auto = r#"<w:pPr><w:spacing w:beforeAutospacing="1" w:afterAutospacing="1"/></w:pPr>"#;
        let bytes = docx(
            &format!(
                "<w:tbl><w:tr><w:tc><w:p>{auto}</w:p><w:p>{auto}</w:p></w:tc></w:tr></w:tbl><w:p>{auto}</w:p>"
            ),
            "",
        );
        let document = open(&bytes).unwrap();
        let blocks = &document.sections[0].blocks;
        let model::Block::Table(table) = &blocks[0] else {
            panic!("a table first");
        };
        let spacing = |b: &model::Block| match b {
            model::Block::Paragraph(p) => (p.props.spacing.before, p.props.spacing.after),
            model::Block::Table(_) => panic!("a paragraph"),
        };
        let cell = &table.rows[0].cells[0].blocks;
        assert_eq!(spacing(&cell[0]), (0.0, 14.0));
        assert_eq!(spacing(&cell[1]), (14.0, 14.0));
        assert_eq!(spacing(&blocks[1]), (14.0, 14.0));
    }

    /// The text each of the six views reads, in the order of [`Views`].
    fn each_view(bytes: &[u8]) -> Vec<String> {
        let characters = characters(bytes).unwrap();
        let views = views(bytes).unwrap();
        assert_eq!(characters.len(), views.len());
        (0..6)
            .map(|view| {
                characters
                    .iter()
                    .zip(&views)
                    .filter(|(_, v)| *v & (1 << view) != 0)
                    .map(|((c, _), _)| *c)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_docx_reads_with_its_changes_shown_accepted_or_rejected_and_its_hidden_text_shown_or_not() {
        let run = |rpr: &str, text: &str| format!(r#"<w:r>{rpr}<w:t>{text}</w:t></w:r>"#);
        let body = [
            "<w:p>".to_string(),
            run("", "Du"),
            r#"<w:del w:id="1"><w:r><w:delText>po</w:delText></w:r></w:del>"#.to_string(),
            run("<w:rPr><w:vanish/></w:rPr>", "Z"),
            format!(r#"<w:ins w:id="2">{}</w:ins>"#, run("", "ntel")),
            "</w:p>".to_string(),
            r#"<w:p><w:pPr><w:rPr><w:del w:id="3"/></w:rPr></w:pPr>"#.to_string(),
            run("", "Pere"),
            "</w:p>".to_string(),
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr>"#.to_string(),
            run("", "z"),
            run(r#"<w:rPr><w:rStyle w:val="Cache"/></w:rPr>"#, "Q"),
            run(
                r#"<w:rPr><w:rPrChange w:id="4"><w:rPr><w:vanish/></w:rPr></w:rPrChange></w:rPr>"#,
                ".",
            ),
            "</w:p>".to_string(),
        ]
        .concat();
        let bytes = docx(
            &body,
            r#"<w:style w:type="character" w:styleId="Cache"><w:rPr><w:vanish/></w:rPr></w:style>"#,
        );
        assert_eq!(
            each_view(&bytes),
            [
                "DupoZntel\nPere\nzQ.\n",
                "Dupontel\nPere\nz.\n",
                "DuZntel\nPerezQ.\n",
                "Duntel\nPerez.\n",
                "DupoZ\nPere\nzQ.\n",
                "Dupo\nPere\nz.\n",
            ]
        );
    }

    #[test]
    fn a_tracked_change_counts_under_whatever_prefix_binds_wordprocessingml() {
        let body = concat!(
            r#"<x:p xmlns:x="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            r#"<x:r><x:t>Du</x:t></x:r><x:del x:id="1"><x:r><x:delText>X</x:delText></x:r></x:del>"#,
            r#"<x:r><x:t>pont</x:t></x:r></x:p><w:p><w:del w:id="2" xmlns:w="urn:other">"#,
            r#"<w:r><w:t>Y</w:t></w:r></w:del></w:p>"#,
        );
        let views = each_view(&docx(body, ""));
        assert_eq!(views[0], "DuXpont\nY\n");
        assert_eq!(views[2], "Dupont\nY\n");
    }

    #[test]
    fn an_odt_reads_with_its_changes_shown_accepted_or_rejected_and_its_hidden_text_shown_or_not() {
        let content = concat!(
            r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3">"#,
            r#"<office:automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties text:display="none"/></style:style></office:automatic-styles>"#,
            r#"<office:body><office:text><text:tracked-changes>"#,
            r#"<text:changed-region text:id="d1"><text:deletion><text:p>po</text:p></text:deletion></text:changed-region>"#,
            r#"<text:changed-region text:id="i1"><text:insertion/></text:changed-region>"#,
            r#"<text:changed-region text:id="i2"><text:insertion/></text:changed-region>"#,
            r#"</text:tracked-changes>"#,
            r#"<text:p>Du<text:change text:change-id="d1"/><text:span text:style-name="T1">Z</text:span>"#,
            r#"<text:change-start text:change-id="i1"/>ntel<text:change-end text:change-id="i1"/></text:p>"#,
            r#"<text:p>Pere<text:change-start text:change-id="i2"/></text:p>"#,
            r#"<text:p><text:change-end text:change-id="i2"/>z.</text:p>"#,
            r#"</office:text></office:body></office:document-content>"#,
        );
        let bytes = package(&[
            ("mimetype", "application/vnd.oasis.opendocument.text".into()),
            ("content.xml", content.into()),
        ]);
        assert_eq!(
            each_view(&bytes),
            [
                "DupoZntel\nPere\nz.\nT1\nT1",
                "Dupontel\nPere\nz.\nT1\nT1",
                "DuZntel\nPere\nz.\nT1\nT1",
                "Duntel\nPere\nz.\nT1\nT1",
                "DupoZ\nPerez.\nT1\nT1",
                "Dupo\nPerez.\nT1\nT1",
            ]
        );
    }
}
