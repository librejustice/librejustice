//! Word documents: `lj-office` reads the characters and offers the edits, this
//! module chooses them, and re-reads the file produced before it goes out.

mod doc;
mod html;
mod odt;
mod ooxml;

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashSet};

use anyhow::Result;
use lj_office::{Format, Replacement};

use crate::labels::{Labels, Needle};
use crate::{Anchor, Occurrence, Outcome, Suggestion};

/// Every character of a `.docx`, `.odt`, `.doc` or web page that [`redact`] searches,
/// comments, properties and deleted text included.
pub fn characters(bytes: &[u8]) -> Result<Vec<(char, Anchor)>> {
    Ok(anchored(lj_office::characters(bytes)?))
}

/// `text` as a feed, each character anchored at its position, which is how a
/// text that is no document is searched and labelled.
pub fn plain(text: &str) -> Result<Vec<(char, Anchor)>> {
    text.chars()
        .enumerate()
        .map(|(at, c)| {
            let anchor = lj_office::Anchor::XmlText {
                part: 0,
                node: 0,
                offset: u32::try_from(at)?,
            };
            Ok((c, Anchor::Office(anchor)))
        })
        .collect()
}

/// The views of [`characters`] a reader may have besides the whole of it,
/// tracked changes accepted or rejected, hidden text hidden, as the
/// positions each keeps: a name a change or hidden text cuts in the file
/// reads whole in one of them.
pub fn views(bytes: &[u8]) -> Result<Vec<Vec<usize>>> {
    Ok(lj_office::view_positions(&lj_office::views(bytes)?))
}

/// An occurrence of a needle in a feed or in one of its views: the position
/// in the feed of each character the view reads.
struct Found {
    needle: usize,
    positions: Vec<usize>,
}

/// Every occurrence of `needles` in `feed` and in each of its `views`. A
/// position past the end of `feed` is left out of a view, as a Word 6 reading
/// in another code page stops before the strings kept outside the stories.
fn found(feed: &[(char, Anchor)], views: &[Vec<usize>], needles: &[String]) -> Vec<Found> {
    let mut out: Vec<Found> = crate::locate(feed, needles)
        .into_iter()
        .map(|o| Found {
            needle: o.needle,
            positions: o.source.collect(),
        })
        .collect();
    for view in views {
        let kept: Vec<usize> = view.iter().copied().filter(|&at| at < feed.len()).collect();
        let glyphs: Vec<(char, Anchor)> = kept.iter().map(|&at| feed[at]).collect();
        for o in crate::locate(&glyphs, needles) {
            out.push(Found {
                needle: o.needle,
                positions: kept[o.source].to_vec(),
            });
        }
    }
    out
}

/// [`crate::locate`] over `feed` and each of its `views`: an occurrence a view
/// reads covers the characters of `feed` from its first to its last, what the
/// view leaves out between them included.
pub fn locate(
    feed: &[(char, Anchor)],
    views: &[Vec<usize>],
    needles: &[String],
) -> Vec<Occurrence> {
    let mut seen = HashSet::new();
    found(feed, views, needles)
        .into_iter()
        .filter_map(|f| {
            let source = f.positions[0]..f.positions[f.positions.len() - 1] + 1;
            seen.insert((source.start, source.end)).then(|| Occurrence {
                needle: f.needle,
                anchors: feed[source.clone()].iter().map(|(_, a)| *a).collect(),
                source,
            })
        })
        .collect()
}

/// Which needles `feed` or one of its `views` still yields.
fn survivors(feed: &[(char, Anchor)], views: &[Vec<usize>], needles: &[String]) -> Vec<String> {
    let found: BTreeSet<usize> = found(feed, views, needles)
        .into_iter()
        .map(|f| f.needle)
        .collect();
    found.into_iter().map(|n| needles[n].clone()).collect()
}

/// What the rules propose to hide in `feed` and in each of its `views`, as
/// they read its [`prose`], a suggestion a view alone makes coming after the others.
pub fn suggestions(
    bytes: &[u8],
    feed: &[(char, Anchor)],
    views: &[Vec<usize>],
) -> Result<Vec<Suggestion>> {
    let prose = prose(bytes, feed)?;
    let mut out = crate::suggest(&prose);
    for view in views {
        let glyphs: Vec<(char, Anchor)> = view.iter().map(|&at| prose[at]).collect();
        for s in crate::suggest(&glyphs) {
            if !out
                .iter()
                .any(|o| o.kind == s.kind && o.text.eq_ignore_ascii_case(&s.text))
            {
                out.push(s);
            }
        }
    }
    Ok(out)
}

/// `feed`, the characters of `bytes`, as the rules read it: the markup among
/// them, a field's keyword and switches or a style name, blanked.
pub fn prose(bytes: &[u8], feed: &[(char, Anchor)]) -> Result<Vec<(char, Anchor)>> {
    let markup = lj_office::markup(bytes)?;
    Ok(feed
        .iter()
        .map(
            |&(c, a)| match a.office().is_some_and(|a| markup.contains(&a)) {
                true => (' ', a),
                false => (c, a),
            },
        )
        .collect())
}

/// Replace every occurrence of `needles` by its label, then re-read the
/// bytes produced: a needle still found there is in [`Outcome::leaks`]. An
/// empty label can join what surrounds it into another occurrence, "the Word
/// Add-In" losing "Word" to read "theAdd" once spaces are stripped, so the
/// pass runs again on its own output, three times at most.
pub fn redact(bytes: &[u8], needles: &[Needle]) -> Result<Outcome> {
    let texts: Vec<String> = needles
        .iter()
        .filter(|n| !n.held)
        .map(|n| n.text.to_owned())
        .collect();
    redact_with(bytes, &texts, &Labels::assign(needles))
}

fn redact_with(bytes: &[u8], needles: &[String], labels: &Labels) -> Result<Outcome> {
    let once = |bytes: &[u8]| match Format::of(bytes)? {
        Format::Docx => ooxml::redact(bytes, needles, labels),
        Format::Odt => odt::redact(bytes, needles, labels),
        Format::Doc => doc::redact(bytes, needles, labels),
        Format::Html => html::redact(bytes, needles, labels),
    };
    let mut outcome = once(bytes)?;
    for _ in 1..3 {
        if outcome.leaks.is_empty() {
            break;
        }
        let next = once(&outcome.bytes)?;
        outcome = Outcome {
            found: outcome.found + next.found,
            removed: outcome.removed + next.removed,
            source_chars: outcome.source_chars,
            ..next
        };
    }
    Ok(outcome)
}

/// Needles still readable in `bytes`, wherever the format carries text.
fn leaks(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    match Format::of(bytes)? {
        Format::Docx => ooxml::leaks(bytes, needles),
        Format::Odt => odt::leaks(bytes, needles),
        Format::Doc => doc::leaks(bytes, needles),
        Format::Html => html::leaks(bytes, needles),
    }
}

/// The edits that put each occurrence of `needles` in `feeds`, or in one of
/// their `views`, under its label, one per run [`Labels::pieces`] cuts, and
/// how many occurrences they replace. A run a view reads takes every
/// character of the feed from its first to its last: a name cut by a tracked
/// change or hidden text goes with the deleted, inserted or hidden text
/// inside it. Occurrences go leftmost then longest first, and a character an
/// earlier one took is not taken again: "Dupont" inside "Jean Dupont" adds
/// nothing, and a feed read again in another code page adds nothing either.
pub fn replacements(
    feeds: &[Vec<(char, Anchor)>],
    views: &[Vec<usize>],
    needles: &[String],
    labels: &Labels,
) -> Result<(usize, Vec<Replacement>)> {
    let mut taken: HashSet<lj_office::Anchor> = HashSet::new();
    let mut count = 0;
    let mut out = Vec::new();
    for feed in feeds {
        let mut occurrences = found(feed, views, needles);
        occurrences.sort_by_key(|o| (o.positions[0], Reverse(o.positions[o.positions.len() - 1])));
        for occurrence in occurrences {
            let text: String = occurrence.positions.iter().map(|&at| feed[at].0).collect();
            let mut replaced = false;
            for (run, with) in labels.pieces(occurrence.needle, &text)? {
                let (first, last) = (
                    occurrence.positions[run.start],
                    occurrence.positions[run.end - 1],
                );
                let anchors: Vec<lj_office::Anchor> = feed[first..=last]
                    .iter()
                    .filter_map(|(_, a)| a.office())
                    .filter(|a| taken.insert(*a))
                    .collect();
                if !anchors.is_empty() {
                    replaced = true;
                    out.push(Replacement {
                        anchors,
                        text: with,
                    });
                }
            }
            count += usize::from(replaced);
        }
    }
    Ok((count, out))
}

fn anchored(feed: Vec<(char, lj_office::Anchor)>) -> Vec<(char, Anchor)> {
    feed.into_iter()
        .map(|(c, a)| (c, Anchor::Office(a)))
        .collect()
}

/// Needles as a binary entry may hold them: UTF-8, Latin-1, UTF-16LE and
/// UTF-16BE, in lower and upper case, each compared ignoring ASCII case, with
/// an `X` in that encoding.
fn byte_patterns(needle: &str) -> Vec<(Vec<u8>, &'static [u8])> {
    let mut out: Vec<(Vec<u8>, &'static [u8])> = Vec::new();
    for form in [needle.to_lowercase(), needle.to_uppercase()] {
        out.push((form.as_bytes().to_vec(), b"X"));
        if form.chars().all(|c| (c as u32) < 0x100) {
            out.push((form.chars().map(|c| c as u8).collect(), b"X"));
        }
        out.push((
            form.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            b"X\0",
        ));
        out.push((
            form.encode_utf16().flat_map(u16::to_be_bytes).collect(),
            b"\0X",
        ));
    }
    out.retain(|(p, _)| !p.is_empty());
    out.sort();
    out.dedup();
    out
}

/// Where `haystack` holds `pattern` from `at` on, ignoring ASCII case, looking
/// further only where the first byte matches: a picture can inflate to megabytes.
fn find_folded(haystack: &[u8], pattern: &[u8], at: usize) -> Option<usize> {
    let (lower, upper) = (
        pattern[0].to_ascii_lowercase(),
        pattern[0].to_ascii_uppercase(),
    );
    memchr::memchr2_iter(lower, upper, haystack.get(at..)?)
        .map(|i| at + i)
        .find(|&i| {
            haystack
                .get(i..i + pattern.len())
                .is_some_and(|w| w.eq_ignore_ascii_case(pattern))
        })
}

/// Which needles occur in `haystack`, ignoring ASCII case, in one pass: an
/// embedded picture can inflate to tens of megabytes.
fn found_in(haystack: &[u8], patterns: &[(usize, Vec<u8>)]) -> BTreeSet<usize> {
    let mut starting: [Vec<&(usize, Vec<u8>)>; 256] = std::array::from_fn(|_| Vec::new());
    for pattern in patterns {
        let first = pattern.1[0];
        starting[first.to_ascii_lowercase() as usize].push(pattern);
        if first.to_ascii_uppercase() != first.to_ascii_lowercase() {
            starting[first.to_ascii_uppercase() as usize].push(pattern);
        }
    }
    let mut found = BTreeSet::new();
    for (at, byte) in haystack.iter().enumerate() {
        for (needle, p) in &starting[*byte as usize] {
            if !found.contains(needle)
                && haystack
                    .get(at..at + p.len())
                    .is_some_and(|w| w.eq_ignore_ascii_case(p))
            {
                found.insert(*needle);
            }
        }
    }
    found
}

/// Overwrite each occurrence of `needle` in bytes no XML reader walks with as
/// many `X`, in the encoding it was found in, so no offset of a picture's
/// metadata or an OLE stream moves. Returns how many occurrences went.
fn erase_bytes(haystack: &mut [u8], needle: &str) -> usize {
    let mut erased = 0;
    for (pattern, x) in byte_patterns(needle) {
        let mut at = 0;
        while let Some(found) = find_folded(haystack, &pattern, at) {
            for unit in haystack[found..found + pattern.len()].chunks_mut(x.len()) {
                unit.copy_from_slice(x);
            }
            erased += 1;
            at = found + pattern.len();
        }
    }
    erased
}

/// Needles erased rather than labelled, as a test of what goes wants them.
#[cfg(test)]
fn erasing(needles: &[&str]) -> (Vec<String>, Labels) {
    let labels = Labels::assign(
        &needles
            .iter()
            .map(|text| Needle {
                text,
                kind: None,
                label: Some(""),
                held: false,
            })
            .collect::<Vec<_>>(),
    );
    (needles.iter().map(|n| n.to_string()).collect(), labels)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use lj_office::model::{Block, Inline};
    use lj_office::ooxml::OoxmlPackage;

    use super::*;
    use crate::Kind;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn package(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            zip.start_file(*name, stored).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn docx(paragraphs: &str) -> Vec<u8> {
        docx_with(paragraphs, &[])
    }

    fn docx_with(paragraphs: &str, parts: &[(&str, &str)]) -> Vec<u8> {
        let body = format!(r#"<w:document {W}><w:body>{paragraphs}</w:body></w:document>"#);
        let mut entries = vec![
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
            ),
            ("word/document.xml", &body),
        ];
        entries.extend_from_slice(parts);
        package(&entries)
    }

    fn odt(content: &str) -> Vec<u8> {
        let content = format!(
            r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3">{content}</office:document-content>"#
        );
        package(&[
            ("mimetype", "application/vnd.oasis.opendocument.text"),
            ("content.xml", &content),
        ])
    }

    fn text(bytes: &[u8]) -> String {
        let text: String = characters(bytes)
            .unwrap()
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        text.trim().to_string()
    }

    fn of(kind: Kind, text: &str) -> Needle<'_> {
        Needle {
            text,
            kind: Some(kind),
            label: None,
            held: false,
        }
    }

    fn field(instruction: &str, result: &str) -> String {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    }

    #[test]
    fn a_field_keyword_a_switch_or_a_style_name_is_never_proposed_and_the_rest_of_the_file_is() {
        let docx = docx_with(
            &[
                r#"<w:p><w:r><w:t xml:space="preserve">Écrire à </w:t></w:r>"#,
                &field(
                    r#" HYPERLINK "mailto:jean.dupont@exemple.fr" \o "Courriel" "#,
                    "Maître Jean DUPONT",
                ),
                r#"<w:r><w:t xml:space="preserve">, </w:t></w:r>"#,
                r#"<w:r><w:rPr><w:vanish/></w:rPr><w:t>Monsieur Paul MARTIN</w:t></w:r>"#,
                r#"<w:r><w:t xml:space="preserve"> signe.</w:t></w:r></w:p>"#,
                r#"<w:p><w:r><w:t xml:space="preserve">Figure </w:t></w:r>"#,
                &field(r" SEQ Figure \* ARABIC ", "1"),
                &field(r" PAGE \* MERGEFORMAT ", "2"),
                "</w:p>",
            ]
            .concat(),
            &[(
                "word/comments.xml",
                &format!(
                    r#"<w:comments {W}><w:comment w:id="0"><w:p><w:r><w:t>Voir Monsieur Luc BERNARD</w:t></w:r></w:p></w:comment></w:comments>"#
                ),
            )],
        );
        let odt = odt(concat!(
            r#"<office:automatic-styles><style:style style:name="T1" style:family="text">"#,
            r#"<style:text-properties text:display="none"/></style:style>"#,
            r#"<style:style style:name="P1" style:display-name="CORPS ARIAL" style:family="paragraph"/>"#,
            r#"</office:automatic-styles><office:body><office:text><text:p text:style-name="P1">Écrire à "#,
            r#"<text:a xlink:href="mailto:jean.dupont@exemple.fr">Maître Jean DUPONT</text:a>, "#,
            r#"<text:span text:style-name="T1">Monsieur Paul MARTIN</text:span> signe."#,
            r#"<office:annotation><dc:creator>R</dc:creator><text:p>Voir Monsieur Luc BERNARD</text:p></office:annotation>"#,
            r#"</text:p></office:text></office:body>"#,
        ));
        let doc = include_bytes!("../../tests/fixtures/field-hidden.doc").to_vec();
        for bytes in [docx, odt, doc] {
            let feed = characters(&bytes).unwrap();
            let proposed: Vec<String> = crate::suggest(&prose(&bytes, &feed).unwrap())
                .into_iter()
                .map(|s| s.text)
                .collect();
            for kept in [
                "Jean DUPONT",
                "jean.dupont@exemple.fr",
                "Paul MARTIN",
                "Luc BERNARD",
            ] {
                assert!(
                    proposed.iter().any(|s| s == kept),
                    "{kept} not in {proposed:?}"
                );
            }
            for markup in ["HYPERLINK", "ARABIC", "MERGEFORMAT", "ARIAL"] {
                assert!(
                    !proposed.iter().any(|s| s.contains(markup)),
                    "{markup} in {proposed:?}"
                );
            }
        }
    }

    #[test]
    fn an_attribute_a_person_wrote_is_searched_proposed_and_removed_and_a_name_of_markup_is_not() {
        let relationships = concat!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="mailto:luc.bernard@exemple.fr" TargetMode="External"/>"#,
            r#"<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>"#,
            r#"</Relationships>"#,
        );
        let settings = format!(
            r#"<w:settings {W}><w:docVars><w:docVar w:name="NomClient" w:val="Maître Sophie LEROY"/></w:docVars></w:settings>"#
        );
        let docx = docx_with(
            concat!(
                r#"<w:p><w:bookmarkStart w:id="0" w:name="_Toc123"/>"#,
                r#"<w:hyperlink r:id="rId9" w:tooltip="Écrire à Maître Paul MARTIN"><w:r><w:t>Écrire au confrère</w:t></w:r></w:hyperlink>"#,
                r#"<w:bookmarkEnd w:id="0"/><w:hyperlink w:anchor="_Ref456"><w:r><w:t>voir plus haut</w:t></w:r></w:hyperlink></w:p>"#,
                r#"<w:p><w:fldSimple w:instr=" HYPERLINK &quot;mailto:claire.durand@exemple.fr&quot; "><w:r><w:t>le greffe</w:t></w:r></w:fldSimple></w:p>"#,
                r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Image 281969989" descr="Portrait de Monsieur Jean DUPONT"/></wp:inline></w:drawing></w:r></w:p>"#,
            ),
            &[
                ("word/_rels/document.xml.rels", relationships),
                ("word/settings.xml", &settings),
            ],
        );
        let odt = odt(concat!(
            r#"<office:body><office:text><text:user-field-decls>"#,
            r#"<text:user-field-decl office:value-type="string" office:string-value="Maître Sophie LEROY" text:name="NomClient"/>"#,
            r#"</text:user-field-decls><text:p><text:bookmark text:name="_Toc123"/>"#,
            r#"<text:a xlink:href="mailto:luc.bernard@exemple.fr" office:title="Écrire à Maître Paul MARTIN" office:target-frame-name="_blank">Écrire au confrère</text:a>"#,
            r#"<text:a xlink:href="mailto:claire.durand@exemple.fr">le greffe</text:a>"#,
            r##"<text:a xlink:href="#_Ref456">voir plus haut</text:a></text:p>"##,
            r#"<text:p><draw:frame draw:name="Image 281969989"><draw:image xlink:href="Pictures/image1.png"/>"#,
            r#"<svg:title>Portrait de Monsieur Jean DUPONT</svg:title></draw:frame></text:p>"#,
            r#"</office:text></office:body>"#,
        ));
        let mut written = [
            "luc.bernard@exemple.fr",
            "Paul MARTIN",
            "claire.durand@exemple.fr",
            "Jean DUPONT",
            "Sophie LEROY",
        ]
        .map(String::from)
        .to_vec();
        written.sort();
        for bytes in [docx, odt] {
            let feed = characters(&bytes).unwrap();
            let searched = [written.clone(), vec!["image1".into()]].concat();
            assert_eq!(crate::survivors(&feed, &searched), written);
            let prose = prose(&bytes, &feed).unwrap();
            let proposed: Vec<String> =
                crate::suggest(&prose).into_iter().map(|s| s.text).collect();
            for kept in &written {
                assert!(proposed.contains(kept), "{kept} not in {proposed:?}");
            }
            let read: String = prose.iter().map(|(c, _)| *c).collect();
            for markup in [
                "_Toc123",
                "_Ref456",
                "281969989",
                "NomClient",
                "_blank",
                "HYPERLINK",
            ] {
                assert!(!read.contains(markup), "{markup} in {read:?}");
            }
            let needles: Vec<Needle> = written
                .iter()
                .map(|w| {
                    of(
                        if w.contains('@') {
                            Kind::Email
                        } else {
                            Kind::Person
                        },
                        w,
                    )
                })
                .collect();
            let outcome = redact(&bytes, &needles).unwrap();
            assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
            assert_eq!(outcome.found, written.len());
        }
    }

    #[test]
    fn a_removal_that_joins_its_neighbours_into_a_needle_is_removed_again() {
        let bytes = docx("<w:p><w:r><w:t>Maître DU(X)PONT plaide</w:t></w:r></w:p>");
        let erased = |text| Needle {
            text,
            kind: None,
            label: Some(""),
            held: false,
        };
        let outcome = redact(&bytes, &[erased("(X)"), erased("Dupont")]).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(text(&outcome.bytes), "Maître  plaide");
    }

    #[test]
    fn a_surname_alone_takes_the_letter_it_has_in_the_full_name() {
        let bytes = docx(
            "<w:p><w:r><w:t>Jean Dupont assigne M. Dupont, puis DUPONT seul.</w:t></w:r></w:p>",
        );
        let entries = [of(Kind::Person, "Jean Dupont"), of(Kind::Person, "Dupont")];
        let outcome = redact(&bytes, &entries).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(
            text(&outcome.bytes),
            "[A] [B] assigne M. [B], puis [B] seul."
        );
        assert_eq!(outcome.found, 3);
    }

    #[test]
    fn each_letter_lands_in_the_run_its_word_starts_in() {
        let bytes = docx(concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Jean </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>DUPONT</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>Maître Du</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>pont plaide</w:t></w:r></w:p>"#,
        ));
        let entries = [of(Kind::Person, "Jean Dupont"), of(Kind::Person, "Dupont")];
        let outcome = redact(&bytes, &entries).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let package = OoxmlPackage::open(&outcome.bytes).unwrap();
        let document = String::from_utf8_lossy(package.entry("word/document.xml").unwrap());
        for kept in [
            r#"<w:t xml:space="preserve">[A] </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>[B]</w:t>"#,
            r#"<w:t>Maître [B]</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve"> plaide</w:t>"#,
        ] {
            assert!(document.contains(kept), "{kept} missing from {document}");
        }
    }

    #[test]
    fn a_doc_variable_and_the_alternative_text_of_a_drawing_are_searched_proposed_and_take_their_labels(
    ) {
        let bytes = include_bytes!("../../tests/fixtures/attributes.doc");
        let feed = characters(bytes).unwrap();
        let read: String = feed.iter().map(|(c, _)| *c).collect();
        for value in [
            "NomClient\nMaître Sophie LEROY",
            "Signature\nSignature de Maître Paul MARTIN",
            "Portrait de Monsieur Jean DUPONT",
        ] {
            assert!(read.contains(value), "{value} not in {read:?}");
        }
        let proposed: Vec<String> = crate::suggest(&prose(bytes, &feed).unwrap())
            .into_iter()
            .map(|s| s.text)
            .collect();
        for kept in ["Sophie LEROY", "Paul MARTIN", "Jean DUPONT"] {
            assert!(
                proposed.iter().any(|s| s == kept),
                "{kept} not in {proposed:?}"
            );
        }
        for markup in ["NomClient", "Signature\n"] {
            let prose: String = prose(bytes, &feed)
                .unwrap()
                .iter()
                .map(|(c, _)| *c)
                .collect();
            assert!(!prose.contains(markup), "{markup} in {prose:?}");
        }
        let entries = [
            of(Kind::Person, "Jean DUPONT"),
            of(Kind::BirthPlace, "MARTIN"),
            of(Kind::Person, "Sophie LEROY"),
        ];
        let outcome = redact(bytes, &entries).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(outcome.found, 3);
        let read = text(&outcome.bytes);
        for value in [
            "NomClient\nMaître [C] [D]",
            "Signature\nSignature de Maître Paul [Localité 1]",
            "Portrait de Monsieur [A] [B]",
        ] {
            assert!(read.contains(value), "{value} not in {read:?}");
        }
    }

    #[test]
    fn the_alternative_text_of_an_inline_picture_of_a_doc_takes_a_label_longer_than_itself() {
        let bytes = include_bytes!("../../tests/fixtures/attributes.doc");
        let entries = [of(Kind::Address, "DUPONT"), of(Kind::BirthPlace, "Jean")];
        let outcome = redact(bytes, &entries).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        let read = text(&outcome.bytes);
        let value = "Portrait de Monsieur [Localité 1] [Adresse 1]";
        assert!(read.contains(value), "{value} not in {read:?}");
    }

    #[test]
    fn a_doc_keeps_a_word_that_only_contains_a_needle() {
        let bytes = include_bytes!("../../tests/fixtures/labels.doc");
        let outcome = redact(bytes, &[of(Kind::Person, "Lila")]).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(outcome.found, 0);
        assert_eq!(text(&outcome.bytes), text(bytes));
    }

    #[test]
    fn a_doc_takes_labels_longer_and_shorter_than_what_they_replace() {
        let bytes = include_bytes!("../../tests/fixtures/labels.doc");
        assert_eq!(
            text(bytes),
            "Jean Dupont demeure 3 rue des Lilas à Tessalit.\nM. Dupont signe."
        );
        let entries = [
            of(Kind::Person, "Jean Dupont"),
            of(Kind::Person, "Dupont"),
            of(Kind::Address, "3 rue des Lilas"),
            of(Kind::BirthPlace, "Tessalit"),
        ];
        let outcome = redact(bytes, &entries).unwrap();
        assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
        assert_eq!(
            text(&outcome.bytes),
            "[A] [B] demeure [Adresse 1] à [Localité 1].\nM. [B] signe."
        );
        let document = lj_office::open(&outcome.bytes).unwrap();
        let Block::Paragraph(first) = &document.sections[0].blocks[0] else {
            panic!("a paragraph first");
        };
        let bold: String = first
            .inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Text(t) if t.props.bold => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bold, "[B]");
    }

    /// Six names, each cut by what a reader of one view or another does not
    /// see: a deletion then an insertion, a deletion, an insertion, hidden
    /// text, all three, text a character style hides.
    const CUT: [&str; 6] = [
        "Dupontel",
        "Martinez",
        "Bernardin",
        "Lefebvre",
        "Rousseau",
        "Gautier",
    ];

    fn cut_docx() -> Vec<u8> {
        let run = |rpr: &str, text: &str| {
            format!(r#"<w:r>{rpr}<w:t xml:space="preserve">{text}</w:t></w:r>"#)
        };
        let deleted = |text: &str| {
            format!(r#"<w:del w:id="1"><w:r><w:delText>{text}</w:delText></w:r></w:del>"#)
        };
        let inserted = |text: &str| format!(r#"<w:ins w:id="2">{}</w:ins>"#, run("", text));
        let vanish = "<w:rPr><w:vanish/></w:rPr>";
        let paragraphs = [
            [
                run("", "Maître Dupo"),
                deleted("m"),
                inserted("n"),
                run("", "tel plaide."),
            ]
            .concat(),
            [
                run("", "Maître Marti"),
                deleted("X"),
                run("", "nez plaide."),
            ]
            .concat(),
            [
                run("", "Maître Berna"),
                inserted("X"),
                run("", "rdin plaide."),
            ]
            .concat(),
            [
                run("", "Maître Lefe"),
                run(vanish, "ZZ"),
                run("", "bvre plaide."),
            ]
            .concat(),
            [
                run("", "Maître Rou"),
                deleted("ss"),
                run(vanish, "Z"),
                inserted("eau"),
                run("", " plaide."),
            ]
            .concat(),
            [
                run("", "Maître Gau"),
                run(r#"<w:rPr><w:rStyle w:val="Cache"/></w:rPr>"#, "ZZ"),
                run("", "tier plaide."),
            ]
            .concat(),
        ]
        .map(|p| format!("<w:p>{p}</w:p>"))
        .concat();
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let relationships = |kind: &str, target: &str| {
            format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/{kind}" Target="{target}"/></Relationships>"#
            )
        };
        docx_with(
            &paragraphs,
            &[
                (
                    "_rels/.rels",
                    &relationships("officeDocument", "word/document.xml"),
                ),
                (
                    "word/_rels/document.xml.rels",
                    &relationships("styles", "styles.xml"),
                ),
                (
                    "word/styles.xml",
                    &format!(
                        r#"<w:styles {W}><w:style w:type="character" w:styleId="Cache"><w:rPr><w:vanish/></w:rPr></w:style></w:styles>"#
                    ),
                ),
            ],
        )
    }

    fn cut_odt() -> Vec<u8> {
        odt(concat!(
            r#"<office:automatic-styles><style:style style:name="T1" style:family="text">"#,
            r#"<style:text-properties text:display="none"/></style:style></office:automatic-styles>"#,
            r#"<office:body><office:text><text:tracked-changes>"#,
            r#"<text:changed-region text:id="d1"><text:deletion><text:p>m</text:p></text:deletion></text:changed-region>"#,
            r#"<text:changed-region text:id="d2"><text:deletion><text:p>X</text:p></text:deletion></text:changed-region>"#,
            r#"<text:changed-region text:id="d3"><text:deletion><text:p>ss</text:p></text:deletion></text:changed-region>"#,
            r#"<text:changed-region text:id="i1"><text:insertion/></text:changed-region>"#,
            r#"<text:changed-region text:id="i2"><text:insertion/></text:changed-region>"#,
            r#"<text:changed-region text:id="i3"><text:insertion/></text:changed-region>"#,
            r#"</text:tracked-changes>"#,
            r#"<text:p>Maître Dupo<text:change text:change-id="d1"/><text:change-start text:change-id="i1"/>n<text:change-end text:change-id="i1"/>tel plaide.</text:p>"#,
            r#"<text:p>Maître Marti<text:change text:change-id="d2"/>nez plaide.</text:p>"#,
            r#"<text:p>Maître Berna<text:change-start text:change-id="i2"/>X<text:change-end text:change-id="i2"/>rdin plaide.</text:p>"#,
            r#"<text:p>Maître Lefe<text:span text:style-name="T1">ZZ</text:span>bvre plaide.</text:p>"#,
            r#"<text:p>Maître Rou<text:change text:change-id="d3"/><text:span text:style-name="T1">Z</text:span><text:change-start text:change-id="i3"/>eau<text:change-end text:change-id="i3"/> plaide.</text:p>"#,
            r#"<text:p>Maître Gau<text:span text:style-name="T1">ZZ</text:span>tier plaide.</text:p>"#,
            r#"</office:text></office:body>"#,
        ))
    }

    /// The text of each of the six views of `bytes`.
    fn each_view(bytes: &[u8]) -> Vec<String> {
        let characters = lj_office::characters(bytes).unwrap();
        let views = lj_office::views(bytes).unwrap();
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
    fn a_name_a_tracked_change_or_hidden_text_cuts_is_found_in_the_view_that_joins_it_and_goes_whole(
    ) {
        let doc = include_bytes!("../../tests/fixtures/revisions.doc").to_vec();
        for bytes in [cut_docx(), cut_odt(), doc] {
            let feed = characters(&bytes).unwrap();
            let views = views(&bytes).unwrap();
            let needles: Vec<String> = CUT.map(String::from).to_vec();
            assert_eq!(crate::survivors(&feed, &needles), Vec::<String>::new());
            assert_eq!(survivors(&feed, &views, &needles), needles);
            let proposed: Vec<String> = suggestions(&bytes, &feed, &views)
                .unwrap()
                .into_iter()
                .map(|s| s.text)
                .collect();
            for name in CUT {
                assert!(
                    proposed
                        .iter()
                        .any(|p| p == &format!("Maître {name}") || p == name),
                    "{name} not in {proposed:?}"
                );
            }

            let outcome = redact(&bytes, &CUT.map(|name| of(Kind::Person, name))).unwrap();
            assert!(outcome.leaks.is_empty(), "{:?}", outcome.leaks);
            assert_eq!(outcome.found, CUT.len());
            for (view, text) in each_view(&outcome.bytes).iter().enumerate() {
                for letter in ["A", "B", "C", "D", "E", "F"] {
                    let line = format!("Maître [{letter}] plaide.");
                    assert!(text.contains(&line), "{line} not in view {view}: {text:?}");
                }
            }
        }
    }
}
