use lj_pdf::graphics::Point;
use lj_pdf::objects::{PdfError, parse_pdf};
use lj_pdf::{PdfDocument, RedactionPlan, RedactionTarget};

fn fixture(name: &str) -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .canonicalize()
        .expect("fixture directory should exist");
    std::fs::read(root.join(name)).expect("fixture should exist")
}

#[test]
fn extracts_text_from_simple_fixture() {
    let document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted.text.contains("Secret Alpha"));
    assert!(extracted.text.contains("Beta Gamma"));
}

#[test]
fn rectangle_redaction_removes_target_text_but_preserves_other_text() {
    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened text extraction should succeed");
    assert!(!extracted.text.contains("Secret"));
    assert!(extracted.text.contains("Beta Gamma"));

    // Verify that old content stream objects are removed from the output:
    // the raw bytes of the saved PDF must not contain the original unredacted text.
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Secret Alpha"),
        "original content stream survived in saved PDF"
    );
}

#[test]
fn search_derived_quads_can_drive_redaction() {
    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let matches = document
        .search_text(0, "Beta Gamma")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("quad redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened.extract_text(0).expect("extract should succeed");
    assert!(extracted.text.contains("Secret Alpha"));
    assert!(!extracted.text.contains("Beta Gamma"));
}

#[test]
fn type0_fonts_with_tounicode_are_searchable_and_redactable() {
    let mut document =
        PdfDocument::open(&fixture("type0-search.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted.text.contains("Secret CID"));

    let matches = document
        .search_text(0, "cid")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Secret"));
    assert!(!extracted_after.text.contains("CID"));
}

#[test]
fn type0_ucs2_h_extracts_searches_redacts_cjk_text() {
    let mut document =
        PdfDocument::open(&fixture("type0-ucs2-h.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert_eq!(
        extracted.text, "中文",
        "UniGB-UCS2-H bytes must decode directly to Unicode"
    );

    // Search finds the single target glyph, and its quads drive the
    // redaction directly.
    let matches = document
        .search_text(0, "中")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1, "exactly one match for '中'");
    assert_eq!(matches[0].quads.len(), 1, "match should be a single quad");

    let quads: Vec<[Point; 4]> = matches[0].quads.iter().map(|q| q.points).collect();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(!after.text.contains("中"), "redacted glyph must be gone");
    assert!(after.text.contains("文"), "non-redacted glyph must remain");
}

#[test]
fn type0_utf16_h_handles_surrogate_pair_redaction() {
    let mut document =
        PdfDocument::open(&fixture("type0-utf16-h.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert_eq!(
        extracted.text, "\u{20000}中",
        "UTF-16 surrogate pair must compose a single SMP scalar"
    );

    // The SMP scalar must be searchable as a single match. Use the search
    // quads directly for redaction.
    let matches = document
        .search_text(0, "\u{20000}")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1, "exactly one match for the SMP scalar");
    assert_eq!(
        matches[0].quads.len(),
        1,
        "surrogate pair should be a single quad"
    );

    let quads: Vec<[Point; 4]> = matches[0].quads.iter().map(|q| q.points).collect();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(
        report.text_glyphs_removed > 0,
        "the surrogate-pair glyph must be removed"
    );

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(
        !after.text.contains('\u{20000}'),
        "redacted SMP glyph must be gone"
    );
    assert!(after.text.contains('中'), "BMP glyph must remain");
}

#[test]
fn can_strip_metadata_and_attachments() {
    let mut document =
        PdfDocument::open(&fixture("metadata-attachments.pdf")).expect("fixture should open");
    document
        .apply_redactions(RedactionPlan {
            targets: Vec::new(),
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(true),
            strip_attachments: Some(true),
            sanitize_hidden_ocgs: None,
        })
        .expect("stripping should succeed");
    let saved = document.save().expect("save should succeed");
    let parsed = parse_pdf(&saved).expect("saved document should parse");
    assert!(!parsed.file.trailer.contains_key("Info"));
    let root_ref = match parsed.file.trailer.get("Root") {
        Some(lj_pdf::objects::PdfValue::Reference(reference)) => *reference,
        _ => panic!("Root reference missing"),
    };
    let root = parsed
        .file
        .get_dictionary(root_ref)
        .expect("catalog should exist");
    // Names key should be removed from catalog after stripping attachments
    assert!(
        root.get("Names").is_none(),
        "Names key should be removed from catalog"
    );
    // Verify that metadata and attachment content are not in the raw output bytes.
    // The Info dictionary contained "Fixture Generator" and "Metadata Fixture".
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Fixture Generator"),
        "Info dictionary content survived in saved PDF"
    );
}

#[test]
fn incremental_update_reads_latest_revision_and_redacts() {
    let mut document = PdfDocument::open(&fixture("incremental-update.pdf"))
        .expect("incremental fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Updated Secret"),
        "should see updated text, got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains("Original Secret"),
        "should not see original text"
    );

    let matches = document
        .search_text(0, "Updated")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Updated"));
    assert!(extracted_after.text.contains("Secret"));
}

#[test]
fn extgstate_font_text_is_extractable_and_redactable() {
    let mut document =
        PdfDocument::open(&fixture("extgstate-font.pdf")).expect("extgstate fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("ExtGState Secret"),
        "should extract text set via gs operator, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Normal Line"));

    let matches = document
        .search_text(0, "ExtGState")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("ExtGState"));
    assert!(extracted_after.text.contains("Normal Line"));
}

#[test]
fn inline_image_and_dictionary_operand_pages_are_parseable() {
    let mut document =
        PdfDocument::open(&fixture("inline-image.pdf")).expect("inline-image fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed despite inline image");
    assert!(
        extracted.text.contains("Inline Image Secret"),
        "should extract text before inline image, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("After Image"),
        "should extract text after inline image"
    );

    let matches = document
        .search_text(0, "Inline Image")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Inline Image"));
    assert!(extracted_after.text.contains("After Image"));
}

#[test]
fn ocg_hidden_layer_refuses_redaction() {
    // The fixture's catalog declares one Optional Content Group marked as
    // OFF in the default configuration. Redaction must refuse to run on such
    // a document because the hidden layer may carry text that the user never
    // saw and therefore cannot target. Text extraction must still work so
    // callers can inspect what is there.
    let document =
        PdfDocument::open(&fixture("ocg-hidden-layer.pdf")).expect("ocg fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should still succeed");
    assert!(extracted.text.contains("Visible Line"));

    let mut document =
        PdfDocument::open(&fixture("ocg-hidden-layer.pdf")).expect("ocg fixture should open");
    let err = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect_err("redaction must refuse documents with hidden OCGs");
    let message = err.to_string();
    assert!(
        message.contains("Optional Content Groups")
            || message.contains("OCProperties")
            || message.contains("hidden"),
        "error should mention hidden layers, got: {message}"
    );
}

#[test]
fn sanitize_hidden_ocgs_strips_hidden_content_and_allows_redaction() {
    // Fixture: a page with "Visible Line" (outside any OCG) plus
    // "Hidden Secret" inside a `BDC /OC /Hidden ... EMC` block pointing
    // at an OCG that is OFF by default. With `sanitize_hidden_ocgs`
    // set, the hidden block is stripped before redaction runs; the
    // visible line survives, the hidden line is gone, and re-opening
    // the saved PDF confirms the hidden bytes did not make it through.
    let document =
        PdfDocument::open(&fixture("ocg-hidden-content.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should still succeed");
    assert!(extracted.text.contains("Visible Line"));
    assert!(extracted.text.contains("Hidden Secret"));

    let mut document =
        PdfDocument::open(&fixture("ocg-hidden-content.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: Some(true),
        })
        .expect("sanitization should allow redaction to run");
    assert_eq!(report.text_glyphs_removed, 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Visible Line"));
    assert!(
        !extracted_after.text.contains("Hidden Secret"),
        "sanitized output must not contain the previously-hidden OCG text"
    );

    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Hidden Secret"),
        "Hidden OCG bytes must not survive in the saved PDF"
    );
}

#[test]
fn sanitize_hidden_ocgs_handles_base_state_off() {
    // /BaseState /OFF hides every OCG unless it is explicitly listed
    // under /ON. Verify the sanitization pass recognises that form and
    // still strips the hidden block.
    let mut document =
        PdfDocument::open(&fixture("ocg-base-state-off.pdf")).expect("fixture should open");
    document
        .apply_redactions(RedactionPlan {
            targets: vec![],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: Some(true),
        })
        .expect("sanitization should accept /BaseState /OFF");
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Visible Line"));
    assert!(!extracted_after.text.contains("Hidden Secret"));
}

#[test]
fn overlay_text_is_stamped_over_redacted_regions() {
    use lj_pdf::{FillColor, RedactionMode};

    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: Some(RedactionMode::Redact),
            fill_color: Some(FillColor { r: 0, g: 0, b: 0 }),
            overlay_text: Some("REDACTED".to_string()),
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction with overlay text should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted.text.contains("Secret Alpha"));
    // The overlay label is part of the page's text layer, so text
    // extraction should find it above the redacted region.
    assert!(
        extracted.text.contains("REDACTED"),
        "overlay text should survive save + reopen as page text, got: {}",
        extracted.text
    );
}

#[test]
fn overlay_text_rejected_for_strip_mode() {
    use lj_pdf::RedactionMode;

    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let err = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: Some(RedactionMode::Strip),
            fill_color: None,
            overlay_text: Some("REDACTED".to_string()),
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect_err("strip mode must refuse overlay_text");
    let message = err.to_string();
    assert!(
        message.contains("overlayText"),
        "error should mention overlayText, got: {message}"
    );
}

#[test]
fn encoding_differences_array_resolves_glyph_names() {
    // /Encoding is a dict with /BaseEncoding /WinAnsiEncoding and a
    // /Differences array that remaps byte 0x40 to /AE (Æ) and 0x7B to /fi
    // (the fi ligature). Text extraction must resolve both glyph names
    // through the AGL subset instead of falling back to U+FFFD.
    let document = PdfDocument::open(&fixture("encoding-differences.pdf"))
        .expect("encoding-differences fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains('Æ'),
        "/AE glyph name should resolve to Æ, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains('\u{FB01}'),
        "/fi glyph name should resolve to U+FB01 (ﬁ), got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains('\u{FFFD}'),
        "no replacement character should appear, got: {}",
        extracted.text
    );
    // Characters that are NOT in Differences still decode via the base WinAnsi
    // table, the space byte 0x20 remains a space, and the ASCII word stays
    // intact.
    assert!(extracted.text.contains("nice"));
}

#[test]
fn vector_v_and_y_curve_segments_are_included_in_path_bounds() {
    // The fixture draws a filled curved shape built from v and y Bezier
    // shorthands directly under "Curve Secret". A search-driven redaction
    // that hits the text must also neutralize the underlying path.
    let mut document =
        PdfDocument::open(&fixture("vector-vy-curves.pdf")).expect("vector-vy fixture should open");
    let matches = document
        .search_text(0, "Curve Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    assert!(
        report.path_paints_removed > 0,
        "v/y curve segments should be included in path bounds so the fill is neutralized"
    );

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let raw = String::from_utf8_lossy(&saved);
    assert!(!raw.contains("Curve Secret"));
    let _ = reopened; // ensure save round-trips cleanly
}

#[test]
fn form_xobject_text_is_extracted_and_searchable() {
    let document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Page Outer"),
        "outer page text should still be extracted, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("Form Inner Secret"),
        "Form XObject text should be extracted, got: {}",
        extracted.text
    );

    // Search should locate the Form XObject text and produce a valid quad.
    let matches = document
        .search_text(0, "Form Inner Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    assert!(!matches[0].quads.is_empty());

    // The quad should land inside the page bounds (612 x 792). The Form's
    // Matrix translates content by +100 in y, and the page content stream
    // places the Form origin at (72, 400), so the glyphs sit near y=500.
    let bbox = matches[0].quads[0].bounding_rect();
    assert!(
        bbox.x >= 0.0 && bbox.max_x() <= 612.0 && bbox.y >= 0.0 && bbox.max_y() <= 792.0,
        "quad should be within page bounds, got bbox: {:?}",
        bbox
    );
}

#[test]
fn form_xobject_does_not_block_redaction_when_target_is_on_page_content() {
    // Redacting the outer page text must succeed even though the page also
    // invokes a Form XObject, the Form does not overlap the target so the
    // engine should leave the Do alone and proceed with the rewrite.
    let mut document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let matches = document
        .search_text(0, "Page Outer")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction of outer text should succeed even with a Form present");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Page Outer"));
    assert!(
        extracted_after.text.contains("Form Inner Secret"),
        "Form XObject content should remain untouched since it did not intersect the target"
    );
}

#[test]
fn nested_form_xobject_redaction_recurses_into_inner_forms() {
    // form-xobject-nested.pdf is three layers deep: page → FmOuter → FmInner.
    // The targeted text "Nested Secret" lives in FmInner. The engine must
    // walk through both Forms, rewrite the inner Form's content, then
    // repoint the outer Form's own Resources.XObject at the redacted
    // inner copy.
    let mut document = PdfDocument::open(&fixture("form-xobject-nested.pdf"))
        .expect("nested form fixture should open");
    let extracted_before = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted_before.text.contains("Nested Secret"));
    assert!(extracted_before.text.contains("Middle Layer"));
    assert!(extracted_before.text.contains("Page Outer"));

    let matches = document
        .search_text(0, "Nested Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("nested Form redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    assert!(report.form_xobjects_rewritten >= 1);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted_after.text.contains("Nested Secret"),
        "inner-Form text should be gone after save, got: {}",
        extracted_after.text
    );
    assert!(
        extracted_after.text.contains("Page Outer"),
        "outermost text should survive"
    );
    assert!(
        extracted_after.text.contains("Middle Layer"),
        "middle Form text should survive (it was not targeted)"
    );
}

#[test]
fn form_xobject_redaction_rewrites_the_form_content_stream() {
    // The Form XObject in this fixture carries "Form Inner Secret".
    // Redacting that string succeeds: the engine allocates a per-page
    // copy of the Form, rewrites its content stream to strip the targeted
    // glyphs, and updates the page's Resources.XObject to point at the
    // copy. The page text "Page Outer" (which lives in the page's own
    // content stream) must survive untouched.
    let mut document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let matches = document
        .search_text(0, "Form Inner Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction inside a Form XObject should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted.text.contains("Form Inner Secret"),
        "Form-interior text should be gone after save, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("Page Outer"),
        "outer page text should remain untouched, got: {}",
        extracted.text
    );
}

#[test]
fn winansi_encoded_simple_font_decodes_non_ascii_bytes() {
    let mut document =
        PdfDocument::open(&fixture("winansi-font.pdf")).expect("winansi fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("CaffÉ"),
        "É byte (0xC9) should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("50°"),
        "° byte should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("€"),
        "€ byte (0x80) should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("l’anno"),
        "’ byte (0x92) should decode to U+2019, got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains('\u{FFFD}'),
        "no replacement characters should appear, got: {}",
        extracted.text
    );

    // Search + redact should still work end-to-end on the decoded text.
    let matches = document
        .search_text(0, "50°")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("50°"));
    assert!(extracted_after.text.contains("CaffÉ"));
}

#[test]
fn xref_stream_and_object_stream_fixture_is_parseable_and_redactable() {
    let mut document = PdfDocument::open(&fixture("xref-object-stream.pdf"))
        .expect("xref+ObjStm fixture should open");
    assert_eq!(document.page_count(), 1);

    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed on an ObjStm-backed page tree");
    assert!(
        extracted.text.contains("OBJSTM Secret"),
        "should extract text whose Page dictionary lives inside an ObjStm, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Beta Gamma"));

    let matches = document
        .search_text(0, "OBJSTM Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed on an ObjStm-backed page tree");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("OBJSTM Secret"));
    assert!(extracted_after.text.contains("Beta Gamma"));

    // The saved output mirrors the input shape: xref stream + ObjStm
    // compression of eligible structural objects, NOT a classic xref
    // table. The redacted content stream must not survive in the raw
    // bytes (the writer rebuilds ObjStms from the post-redaction object
    // map; the original ObjStm container is dropped at parse time).
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("OBJSTM Secret"),
        "original content stream survived in saved PDF"
    );
    assert!(
        contains_subslice(&saved, b"/Type /XRef"),
        "modern-input PDF must round-trip with an xref stream"
    );
    assert!(
        contains_subslice(&saved, b"/Type /ObjStm"),
        "modern-input PDF should pack eligible objects into a fresh ObjStm"
    );
    assert!(
        !contains_subslice(&saved, b"\nxref\n"),
        "saved bytes should not contain a classic xref keyword"
    );
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn xref_stream_input_without_objstm_round_trips_as_xref_stream() {
    // Input is xref-stream-shaped but has no compressed (ObjStm) members.
    // The writer must mirror the input's xref-stream shape on save.
    // Because every object is compressible (gen=0, non-stream values),
    // the writer is also expected to pack them into a fresh ObjStm.
    let document = PdfDocument::open(&fixture("xref-stream-no-objstm.pdf"))
        .expect("xref-stream fixture should open");
    let extracted = document.extract_text(0).expect("extraction should succeed");
    assert!(extracted.text.contains("Plain XRef Stream"));

    let saved = document.save().expect("save should succeed");
    assert!(
        contains_subslice(&saved, b"/Type /XRef"),
        "xref-stream-shaped input must save as xref stream"
    );
    assert!(
        !contains_subslice(&saved, b"\nxref\n"),
        "saved bytes should not contain a classic xref keyword"
    );

    let reopened = PdfDocument::open(&saved).expect("saved xref-stream PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(extracted_after.text.contains("Plain XRef Stream"));
}

#[test]
fn classic_xref_input_saves_as_classic_xref() {
    // A classic-xref fixture round-trips with a classic xref table, not
    // an xref stream.
    let document =
        PdfDocument::open(&fixture("simple-text.pdf")).expect("classic fixture should open");
    let saved = document.save().expect("save should succeed");
    assert!(
        contains_subslice(&saved, b"\nxref\n"),
        "classic-input PDF must continue saving as a classic xref table"
    );
    assert!(
        !contains_subslice(&saved, b"/Type /XRef"),
        "classic-input PDF must not emit an xref stream"
    );
    let reopened = PdfDocument::open(&saved).expect("saved classic PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(extracted_after.text.contains("Secret Alpha"));
}

#[test]
fn nested_cm_operators_produce_page_space_quads() {
    let mut document =
        PdfDocument::open(&fixture("nested-cm.pdf")).expect("nested-cm fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Nested CM Secret"),
        "should extract text from inner cm block, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Outer Text"));

    // Search-derived quads must be within page bounds (612x792)
    let matches = document
        .search_text(0, "Nested CM")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    for quad in &matches[0].quads {
        let bbox = quad.bounding_rect();
        assert!(
            bbox.x >= -1.0 && bbox.max_x() <= 613.0 && bbox.y >= -1.0 && bbox.max_y() <= 793.0,
            "quad should be within page bounds, got bbox: {:?}",
            bbox
        );
    }

    // Quads should also drive a successful redaction
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Nested CM"));
    assert!(extracted_after.text.contains("Outer Text"));
}

#[test]
fn standard_encoded_simple_font_decodes_quoteright_and_quoteleft() {
    // The fixture encodes the byte sequence `D o n 0x27 t 0x60 e`. Under
    // Adobe StandardEncoding 0x27 is `quoteright` = U+2019 and 0x60 is
    // `quoteleft` = U+2018, so the extracted text should be
    // "Don\u{2019}t\u{2018}e" rather than "Don't`e" that Identity /
    // WinAnsi decoders would produce.
    let document =
        PdfDocument::open(&fixture("standard-encoding.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Don\u{2019}t\u{2018}e"),
        "StandardEncoding 0x27/0x60 should decode to curly quotes: got {:?}",
        extracted.text
    );
}

#[test]
fn mac_roman_encoded_simple_font_decodes_mac_specific_bytes() {
    // The fixture encodes `Hello "World"` where the outer quotes are
    // Mac Roman bytes 0xD2 (U+201C) and 0xD3 (U+201D). Under WinAnsi
    // these bytes would decode to different glyphs (or U+FFFD), so a
    // correct MacRoman path is the only way the expected Unicode comes
    // out.
    let document =
        PdfDocument::open(&fixture("mac-roman-encoding.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Hello \u{201C}World\u{201D}"),
        "MacRoman bytes 0xD2/0xD3 should decode to curly double quotes: got {:?}",
        extracted.text
    );

    // Also verify redaction: search for "World" and confirm the match
    // covers only the World glyphs (not the curly quotes or "Hello"),
    // then redact and reopen.
    let mut document =
        PdfDocument::open(&fixture("mac-roman-encoding.pdf")).expect("fixture should open");
    let matches = document
        .search_text(0, "World")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("MacRoman redaction should succeed");
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let after = reopened.extract_text(0).expect("reopen extraction");
    assert!(!after.text.contains("World"));
    assert!(after.text.contains("Hello"));
}

#[test]
fn ultra_dense_rows_one_pt_apart_are_probed() {
    // Rows sit only 1pt apart at 8pt Helvetica. The line grouper splits
    // rows at a gap of exactly 1pt (`>=`), so the later row's trailing
    // glyphs stay out of the earlier row.
    let document =
        PdfDocument::open(&fixture("ultra-dense-layout.pdf")).expect("fixture should open");
    let extracted = document.extract_text(0).expect("extraction should succeed");
    assert!(extracted.text.contains("Row Alpha 1000"));
    assert!(extracted.text.contains("Row Beta 2000"));
    assert!(extracted.text.contains("Row Gamma 3000"));

    for needle in ["1000", "2000", "3000"] {
        let matches = document
            .search_text(0, needle)
            .expect("search should succeed");
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one match for {needle}, got {:?}",
            matches
        );
    }
}

#[test]
fn dense_layout_rows_are_not_merged_into_one_line() {
    // The fixture stacks four rows only 2pt apart in y at 6pt Helvetica.
    // Extraction should yield four distinct matches for "Account" and each
    // account number's surrounding row text should stay on its own visual
    // line so search and quad derivation don't confuse adjacent rows.
    let document = PdfDocument::open(&fixture("dense-layout.pdf")).expect("fixture should open");
    let extracted = document.extract_text(0).expect("extraction should succeed");
    assert!(extracted.text.contains("Account A 1111"));
    assert!(extracted.text.contains("Account B 2222"));
    assert!(extracted.text.contains("Account C 3333"));
    assert!(extracted.text.contains("Account D 4444"));

    let matches = document
        .search_text(0, "Account")
        .expect("search should succeed");
    assert_eq!(
        matches.len(),
        4,
        "each row's Account label should be its own match"
    );

    // Each match's quad should stay within a single row, height of a
    // single 6pt line (≈ 4.8pt), not span two rows (≈ 6.8pt+). If the
    // line grouper collapses two rows into one the resulting match quad
    // would straddle both rows and be noticeably taller.
    for m in &matches {
        let rect = m
            .quads
            .first()
            .expect("each match should have a quad")
            .bounding_rect();
        // A single 6pt row has glyph-height ~5.5pt from the ascent/descent
        // heuristic. Two merged rows sitting 2pt apart would produce a
        // match quad of ~7.5pt or more, guard against that boundary.
        assert!(
            rect.height < 7.0,
            "match should be confined to a single row: got height {} for {:?}",
            rect.height,
            m
        );
    }
}

#[test]
fn sub_pt_dense_rows_split_into_separate_lines() {
    // Three rows of 6pt Helvetica only 0.5pt apart in y. The proportional
    // `height_ref * 0.10` tolerance (0.48pt at 6pt) keeps each row separate.
    let document =
        PdfDocument::open(&fixture("sub-pt-dense-layout.pdf")).expect("fixture should open");
    let extracted = document.extract_text(0).expect("extraction should succeed");
    assert!(extracted.text.contains("Row A 111"));
    assert!(extracted.text.contains("Row B 222"));
    assert!(extracted.text.contains("Row C 333"));

    for needle in ["111", "222", "333"] {
        let matches = document
            .search_text(0, needle)
            .expect("search should succeed");
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one match for {needle}, got {:?}",
            matches
        );
        let rect = matches[0]
            .quads
            .first()
            .expect("each match should have a quad")
            .bounding_rect();
        assert!(
            rect.height < 6.0,
            "match quad for {needle} must fit within one 6pt row, got height {}",
            rect.height
        );
    }
}

#[test]
fn bx_ex_compat_section_lets_redaction_proceed() {
    let mut document =
        PdfDocument::open(&fixture("bx-ex-compat.pdf")).expect("fixture should open");
    let extracted = document.extract_text(0).expect("extraction should succeed");
    assert!(extracted.text.contains("Redact compat sample"));
    assert!(extracted.text.contains("Keep alpha"));

    // The fixture's content stream carries an unrecognized `sh` operator
    // inside a BX/EX section. Without the compatibility-section pass-through
    // the engine would refuse to redact; with it, the redaction succeeds and
    // the recognized text glyphs are still removed cleanly.
    let matches = document
        .search_text(0, "compat")
        .expect("search should succeed");
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed inside BX/EX fixture");

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let after = reopened.extract_text(0).expect("reopen extraction");
    assert!(!after.text.contains("compat"));
    assert!(after.text.contains("Keep alpha"));
}

#[test]
fn apply_redactions_invalidates_cached_page_text() {
    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");

    // Warm the per-page text cache.
    let before = document.extract_text(0).expect("extraction should succeed");
    assert!(before.text.contains("Secret Alpha"));

    // Redact the "Secret" glyph run in place.
    let matches = document
        .search_text(0, "Secret Alpha")
        .expect("search should succeed");
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");

    // The cache must have been cleared, a second extraction reflects
    // the post-redaction content stream.
    let after = document
        .extract_text(0)
        .expect("post-redaction extraction should succeed");
    assert!(
        !after.text.contains("Secret Alpha"),
        "cache should be invalidated after apply_redactions, got: {:?}",
        after.text
    );
    assert!(after.text.contains("Beta Gamma"));
}

#[test]
fn run_length_compressed_content_stream_is_readable_and_redactable() {
    let document =
        PdfDocument::open(&fixture("run-length-content.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Redact RLE sample"),
        "RunLength stream text should extract: got {:?}",
        extracted.text
    );
    assert!(extracted.text.contains("Keep alpha"));

    let mut document =
        PdfDocument::open(&fixture("run-length-content.pdf")).expect("fixture should open");
    let matches = document
        .search_text(0, "RLE")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("RunLength-stream redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted_after.text.contains("RLE"),
        "RLE glyphs should be removed after redaction: got {:?}",
        extracted_after.text
    );
    assert!(extracted_after.text.contains("Keep alpha"));
}

#[test]
fn lzw_compressed_content_stream_is_readable_and_redactable() {
    let document = PdfDocument::open(&fixture("lzw-content.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Redact LZW sample"),
        "LZW stream text should extract: got {:?}",
        extracted.text
    );
    assert!(extracted.text.contains("Keep alpha"));

    let mut document = PdfDocument::open(&fixture("lzw-content.pdf")).expect("fixture should open");
    let matches = document
        .search_text(0, "LZW")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("LZW-stream redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted_after.text.contains("LZW"),
        "LZW glyphs should be removed after redaction: got {:?}",
        extracted_after.text
    );
    assert!(extracted_after.text.contains("Keep alpha"));
}

// ---------------------------------------------------------------------
// Partial Image XObject rewriting tests.
//
// Each test builds an Image XObject in-code (Flate-compressed RGB, raw
// or DCT-encoded JPEG), assembles a minimal PDF that draws the image
// at known page-space coordinates, applies a redaction target covering
// part of the image, then inspects the saved bytes.
// ---------------------------------------------------------------------

fn build_partial_image_flate_pdf(width: u32, height: u32) -> Vec<u8> {
    build_partial_image_flate_pdf_in(width, height, "/DeviceRGB")
}

/// `colour_space` is written as the image's `/ColorSpace`; object 7 is an
/// RGB ICC profile it can name.
fn build_partial_image_flate_pdf_in(width: u32, height: u32, colour_space: &str) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    // 32x32 RGB gradient: each pixel's R, G, B is its (x, y, x^y) value.
    let mut raw: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            raw.push((x * 8) as u8);
            raw.push((y * 8) as u8);
            raw.push((x as u8) ^ (y as u8));
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).unwrap();
    let compressed = encoder.finish().unwrap();

    // Image is drawn at user-space (72, 600)..(232, 760) via cm 160 0 0 160 72 600.
    let content =
        "q\n160 0 0 160 72 600 cm\n/Im1 Do\nQ\nBT\n/F1 14 Tf\n72 760 Td\n(Above image) Tj\nET\n"
            .to_string();

    let mut pdf: Vec<u8> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let catalog_offset = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
    let page_offset = pdf.len();
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = pdf.len();
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let font_offset = pdf.len();
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let image_offset = pdf.len();
    pdf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace {colour_space} /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let profile_offset = pdf.len();
    pdf.extend_from_slice(b"7 0 obj\n<< /N 3 /Length 0 >>\nstream\n\nendstream\nendobj\n");

    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 8\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        content_offset,
        font_offset,
        image_offset,
        profile_offset,
    ] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(b"trailer\n<< /Size 8 /Root 1 0 R >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    pdf
}

fn build_partial_image_jpeg_pdf(width: u32, height: u32) -> Vec<u8> {
    use jpeg_encoder::{ColorType, Encoder};

    let mut raw: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            raw.push((x * 8) as u8);
            raw.push((y * 8) as u8);
            raw.push(((x as u8) ^ (y as u8)).wrapping_mul(2));
        }
    }
    let mut jpeg_bytes: Vec<u8> = Vec::new();
    {
        let encoder = Encoder::new(&mut jpeg_bytes, 90);
        encoder
            .encode(&raw, width as u16, height as u16, ColorType::Rgb)
            .unwrap();
    }

    let content =
        "q\n160 0 0 160 72 600 cm\n/Im1 Do\nQ\nBT\n/F1 14 Tf\n72 760 Td\n(Above image) Tj\nET\n";

    let mut pdf: Vec<u8> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let catalog_offset = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
    let page_offset = pdf.len();
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = pdf.len();
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let font_offset = pdf.len();
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let image_offset = pdf.len();
    pdf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            jpeg_bytes.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&jpeg_bytes);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 7\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        content_offset,
        font_offset,
        image_offset,
    ] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    pdf
}

fn page_image_stream(document: &lj_pdf::objects::ParsedDocument) -> &lj_pdf::objects::PdfStream {
    use lj_pdf::objects::{PdfObject, PdfValue};
    let page = &document.pages[0];
    let xobjects_value = page
        .resources
        .get("XObject")
        .expect("page resources have /XObject");
    let xobjects = document
        .file
        .resolve_dict(xobjects_value)
        .expect("XObject resolves to dict");
    let im1 = xobjects.get("Im1").expect("Im1 entry present");
    let image_ref = match im1 {
        PdfValue::Reference(r) => *r,
        _ => panic!("Im1 must be an indirect reference"),
    };
    match document.file.get_object(image_ref).expect("image obj") {
        PdfObject::Stream(stream) => stream,
        _ => panic!("Im1 should be a stream"),
    }
}

fn decode_flate_image(saved: &[u8]) -> (u32, u32, Vec<u8>) {
    let document = parse_pdf(saved).expect("saved PDF should reopen");
    let stream = page_image_stream(&document);
    let width = stream
        .dict
        .get("Width")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;
    let height = stream
        .dict
        .get("Height")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;
    let pixels = lj_pdf::objects::decode_stream(stream).expect("decode image");
    (width, height, pixels)
}

fn decode_jpeg_image(saved: &[u8]) -> (u32, u32, Vec<u8>) {
    use jpeg_decoder::Decoder;
    let document = parse_pdf(saved).expect("saved PDF should reopen");
    let stream = page_image_stream(&document);
    let width = stream
        .dict
        .get("Width")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;
    let height = stream
        .dict
        .get("Height")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;
    let mut decoder = Decoder::new(stream.data.as_slice());
    let pixels = decoder.decode().expect("JPEG decode");
    (width, height, pixels)
}

fn corner_plan() -> RedactionPlan {
    RedactionPlan {
        targets: vec![RedactionTarget::Rect {
            page_index: 0,
            x: 152.0,
            y: 680.0,
            width: 80.0,
            height: 80.0,
        }],
        mode: None,
        fill_color: None,
        overlay_text: None,
        remove_intersecting_annotations: Some(false),
        strip_metadata: Some(false),
        strip_attachments: Some(false),
        sanitize_hidden_ocgs: None,
    }
}

#[test]
fn partial_image_in_an_icc_profile_masks_pixels_and_keeps_its_profile() {
    let pdf = build_partial_image_flate_pdf_in(32, 32, "[/ICCBased 7 0 R]");
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let report = document
        .apply_redactions(corner_plan())
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_masked, 1);
    assert_eq!(report.image_draws_removed, 0);

    let saved = document.save().expect("save");
    let (_, _, pixels) = decode_flate_image(&saved);
    let at = |x: usize, y: usize| &pixels[(y * 32 + x) * 3..(y * 32 + x) * 3 + 3];
    assert_eq!(at(20, 4), [0, 0, 0]);
    assert_eq!(at(4, 20), [32, 160, 4 ^ 20]);
    let reopened = parse_pdf(&saved).expect("reopen");
    let space = page_image_stream(&reopened).dict.get("ColorSpace").cloned();
    assert!(
        matches!(space, Some(lj_pdf::objects::PdfValue::Array(_))),
        "{space:?}"
    );
}

#[test]
fn an_image_dropped_whole_leaves_no_resource_naming_it() {
    // An image in the Pattern space does not decode: it is dropped whole.
    let pdf = build_partial_image_flate_pdf_in(32, 32, "/Pattern");
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let report = document
        .apply_redactions(corner_plan())
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_removed, 1);
    document
        .apply_redactions(corner_plan())
        .expect("a second pass reads the page again");

    let saved = document.save().expect("save");
    let reopened = parse_pdf(&saved).expect("reopen");
    let xobjects = reopened.pages[0]
        .resources
        .get("XObject")
        .map(|value| reopened.file.resolve_dict(value).expect("dict").clone())
        .unwrap_or_default();
    assert!(!xobjects.contains_key("Im1"), "{xobjects:?}");
    assert!(
        !reopened.file.objects.values().any(
            |object| matches!(object, lj_pdf::objects::PdfObject::Stream(s)
                if s.dict.get("Subtype").and_then(|v| v.as_name()) == Some("Image"))
        ),
        "the dropped image stays in the file"
    );
}

#[test]
fn partial_image_flate_masks_pixels() {
    let pdf = build_partial_image_flate_pdf(32, 32);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    // Image is drawn at user-space (72, 600)..(232, 760), 160pt wide × 160pt tall,
    // rendering 32×32 px → 5 user-space pt per pixel.
    // Redact the upper-right 80×80 pt region: page-space (152, 680)..(232, 760).
    // Mapped to image space: top-right 16×16 pixels.
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 152.0,
                y: 680.0,
                width: 80.0,
                height: 80.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_masked, 1, "expected one partial mask");
    assert_eq!(report.image_draws_removed, 0, "no whole-image drops");

    let saved = document.save().expect("save");
    let (width, height, pixels) = decode_flate_image(&saved);
    assert_eq!(width, 32);
    assert_eq!(height, 32);
    assert_eq!(pixels.len(), 32 * 32 * 3);

    // The redacted region is the top-right 16×16 px of the image.
    // In image space (Y=0 at top), that's columns 16..32, rows 0..16.
    for y in 0..32u32 {
        for x in 0..32u32 {
            let off = ((y * 32 + x) * 3) as usize;
            let inside = (16..32).contains(&x) && (0..16).contains(&y);
            if inside {
                assert_eq!(pixels[off], 0, "pixel ({x},{y}) R should be 0");
                assert_eq!(pixels[off + 1], 0, "pixel ({x},{y}) G should be 0");
                assert_eq!(pixels[off + 2], 0, "pixel ({x},{y}) B should be 0");
            } else {
                // Outside the mask: original gradient values.
                let expected_r = (x * 8) as u8;
                let expected_g = (y * 8) as u8;
                let expected_b = (x as u8) ^ (y as u8);
                assert_eq!(pixels[off], expected_r, "unmasked pixel ({x},{y}) R drift");
                assert_eq!(pixels[off + 1], expected_g);
                assert_eq!(pixels[off + 2], expected_b);
            }
        }
    }
}

#[test]
fn partial_image_jpeg_masks_pixels() {
    let pdf = build_partial_image_jpeg_pdf(32, 32);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    // Same target geometry as the Flate test.
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 152.0,
                y: 680.0,
                width: 80.0,
                height: 80.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_masked, 1, "expected one partial mask");
    assert_eq!(report.image_draws_removed, 0);

    let saved = document.save().expect("save");
    let (width, height, pixels) = decode_jpeg_image(&saved);
    assert_eq!(width, 32);
    assert_eq!(height, 32);

    // JPEG is lossy: assert the masked region is "near black" (sum of
    // R+G+B per pixel below a tolerance) and the unmasked region still
    // has measurable colour content.
    let mut masked_sum = 0u32;
    let mut masked_count = 0u32;
    let mut unmasked_sum = 0u32;
    let mut unmasked_count = 0u32;
    for y in 0..32u32 {
        for x in 0..32u32 {
            let off = ((y * 32 + x) * 3) as usize;
            let total = pixels[off] as u32 + pixels[off + 1] as u32 + pixels[off + 2] as u32;
            let inside = (16..32).contains(&x) && (0..16).contains(&y);
            if inside {
                masked_sum += total;
                masked_count += 1;
            } else {
                unmasked_sum += total;
                unmasked_count += 1;
            }
        }
    }
    let masked_avg = masked_sum / masked_count;
    let unmasked_avg = unmasked_sum / unmasked_count;
    assert!(
        masked_avg < 30,
        "masked region should be near black, got avg sum {masked_avg}"
    );
    assert!(
        unmasked_avg > 100,
        "unmasked region should retain colour, got avg sum {unmasked_avg}"
    );
}

#[test]
fn full_cover_image_drops_to_n() {
    let pdf = build_partial_image_flate_pdf(32, 32);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    // Cover the entire image (and then some).
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 50.0,
                y: 580.0,
                width: 200.0,
                height: 200.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_masked, 0);
    assert_eq!(
        report.image_draws_removed, 1,
        "image should be fully dropped"
    );
}

#[test]
fn partial_image_drops_original_ref_when_unreferenced() {
    // After a partial mask the original image stream must not survive in
    // the saved PDF (no other live reference points at it). Otherwise a
    // reader could recover the unmasked pixels from the orphan indirect
    // object.
    let pdf = build_partial_image_flate_pdf(32, 32);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 152.0,
                y: 680.0,
                width: 80.0,
                height: 80.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(report.image_draws_masked, 1);

    let saved = document.save().expect("save");
    let reopened = parse_pdf(&saved).expect("saved PDF should reopen");

    // The fixture's only Image XObject is at object 6; after a partial
    // mask + COW the saved PDF should contain exactly one Image stream
    // (the masked copy) and no orphan original.
    use lj_pdf::objects::PdfObject;
    let mut image_refs: Vec<_> = reopened
        .file
        .objects
        .iter()
        .filter(|(_, object)| match object {
            PdfObject::Stream(stream) => {
                stream.dict.get("Subtype").and_then(|v| v.as_name()) == Some("Image")
            }
            _ => false,
        })
        .map(|(r, _)| *r)
        .collect();
    image_refs.sort_by_key(|r| r.object_number);
    assert_eq!(
        image_refs.len(),
        1,
        "exactly one Image XObject should survive in the saved PDF, got {image_refs:?}"
    );
}

#[test]
fn partial_image_two_invocations_mask_both_rects() {
    // When the same Image XObject is drawn twice on a page with two
    // different partial overlap rectangles, BOTH rectangles are masked
    // in the one copy /Im1 names in the saved file.

    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    // 32x32 RGB gradient.
    let width = 32u32;
    let height = 32u32;
    let mut raw: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            raw.push((x * 8) as u8);
            raw.push((y * 8) as u8);
            raw.push((x as u8) ^ (y as u8));
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).unwrap();
    let compressed = encoder.finish().unwrap();

    // Two Do invocations of /Im1 at non-overlapping page-space rects:
    //   first  at (72, 600)..(232, 760) , image space x∈[0,32), y∈[0,32)
    //   second at (300, 600)..(460, 760), same image-space tile, drawn elsewhere
    // The first redaction target hits ONLY the first invocation's
    // top-right quadrant; the second target hits ONLY the second
    // invocation's bottom-left quadrant.
    let content = "q\n160 0 0 160 72 600 cm\n/Im1 Do\nQ\nq\n160 0 0 160 300 600 cm\n/Im1 Do\nQ\n";

    let mut pdf: Vec<u8> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let catalog_offset = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
    let page_offset = pdf.len();
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = pdf.len();
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let font_offset = pdf.len();
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let image_offset = pdf.len();
    pdf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        content_offset,
        font_offset,
        image_offset,
    ] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![
                // First-invocation top-right 16x16 px → page rect (152, 680)..(232, 760)
                RedactionTarget::Rect {
                    page_index: 0,
                    x: 152.0,
                    y: 680.0,
                    width: 80.0,
                    height: 80.0,
                },
                // Second-invocation bottom-left 16x16 px → page rect (300, 600)..(380, 680)
                RedactionTarget::Rect {
                    page_index: 0,
                    x: 300.0,
                    y: 600.0,
                    width: 80.0,
                    height: 80.0,
                },
            ],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(
        report.image_draws_masked, 2,
        "both Do invocations should be tracked"
    );
    assert_eq!(report.image_draws_removed, 0);

    let saved = document.save().expect("save");
    let (width, height, pixels) = decode_flate_image(&saved);
    assert_eq!(width, 32);
    assert_eq!(height, 32);

    // Both rectangles must be black in the single COW masked copy.
    // First-invocation top-right: x∈[16,32), y∈[0,16)
    // Second-invocation bottom-left: x∈[0,16), y∈[16,32)
    for y in 0..32u32 {
        for x in 0..32u32 {
            let off = ((y * 32 + x) * 3) as usize;
            let in_first = (16..32).contains(&x) && (0..16).contains(&y);
            let in_second = (0..16).contains(&x) && (16..32).contains(&y);
            if in_first || in_second {
                assert_eq!(pixels[off], 0, "pixel ({x},{y}) R should be 0");
                assert_eq!(pixels[off + 1], 0, "pixel ({x},{y}) G should be 0");
                assert_eq!(pixels[off + 2], 0, "pixel ({x},{y}) B should be 0");
            } else {
                let expected_r = (x * 8) as u8;
                let expected_g = (y * 8) as u8;
                let expected_b = (x as u8) ^ (y as u8);
                assert_eq!(pixels[off], expected_r, "pixel ({x},{y}) R drift");
                assert_eq!(pixels[off + 1], expected_g);
                assert_eq!(pixels[off + 2], expected_b);
            }
        }
    }
}

#[test]
fn form_xobject_redaction_and_partial_image_mask_compose_on_one_page() {
    // When a single page has both a Form XObject that gets redacted (COW
    // with content rewrite, points the page's Resources.XObject[Fm1]
    // at the new ref) AND an Image XObject that gets a partial mask
    // (COW with pixel rewrite, points Resources.XObject[Im1] at the
    // new ref), neither override may clobber the other.
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    // 32x32 gradient image, FlateDecode-compressed.
    let width = 32u32;
    let height = 32u32;
    let mut raw: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            raw.push((x * 8) as u8);
            raw.push((y * 8) as u8);
            raw.push((x as u8) ^ (y as u8));
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).unwrap();
    let compressed = encoder.finish().unwrap();

    // Form XObject content: draws "Form Inner Secret" at position
    // (50, 50) inside the form, which is mapped via /Matrix [1 0 0 1
    // 0 100] to roughly (50, 150) in page space.
    let form_content = "BT\n/F2 14 Tf\n0 0 Td\n(Form Inner Secret) Tj\nET\n";

    // Page content: header text outside both targets (y=400, well
    // below the image at y∈[600,760] and far above the form at y≈150),
    // Form Do, image Do.
    let page_content = "BT\n/F1 18 Tf\n72 400 Td\n(Page Header) Tj\nET\n\
         q\n1 0 0 1 50 50 cm\n/Fm1 Do\nQ\n\
         q\n160 0 0 160 72 600 cm\n/Im1 Do\nQ\n";

    let mut pdf: Vec<u8> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let catalog_offset = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
    let page_offset = pdf.len();
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R /Im1 7 0 R >> >> \
          /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = pdf.len();
    pdf.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", page_content.len()).as_bytes(),
    );
    pdf.extend_from_slice(page_content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let font_offset = pdf.len();
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let form_offset = pdf.len();
    pdf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 400 200] \
         /Matrix [1 0 0 1 0 100] /Resources << /Font << /F2 8 0 R >> >> \
         /Length {} >>\nstream\n",
            form_content.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(form_content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let image_offset = pdf.len();
    pdf.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let form_font_offset = pdf.len();
    pdf.extend_from_slice(
        b"8 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );

    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 9\n0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        content_offset,
        font_offset,
        form_offset,
        image_offset,
        form_font_offset,
    ] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(b"trailer\n<< /Size 9 /Root 1 0 R >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    // Sanity: extraction sees both the Form text and the page header.
    let extracted_before = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted_before.text.contains("Form Inner Secret"));
    assert!(extracted_before.text.contains("Page Header"));

    // Two redaction targets:
    //  - The Form's "Form Inner Secret" sits around y=150 in page space.
    //  - The image's top-right 16×16 pixels at page-space (152, 680)..(232, 760).
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![
                RedactionTarget::Rect {
                    page_index: 0,
                    x: 40.0,
                    y: 140.0,
                    width: 220.0,
                    height: 40.0,
                },
                RedactionTarget::Rect {
                    page_index: 0,
                    x: 152.0,
                    y: 680.0,
                    width: 80.0,
                    height: 80.0,
                },
            ],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(
        report.form_xobjects_rewritten >= 1,
        "Form XObject should be COW-rewritten"
    );
    assert_eq!(
        report.image_draws_masked, 1,
        "image should be partial-masked"
    );

    let saved = document.save().expect("save");
    let reopened = PdfDocument::open(&saved).expect("saved PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");

    // Form text gone, page header survives.
    assert!(
        !extracted_after.text.contains("Form Inner Secret"),
        "Form override must persist through the image-mask rewire (text should be redacted): {:?}",
        extracted_after.text,
    );
    assert!(
        extracted_after.text.contains("Page Header"),
        "untouched page text should remain"
    );

    // Image still present and partially masked. Verify via the saved
    // page's Resources.XObject pointing at a fresh image stream.
    let document = parse_pdf(&saved).expect("saved PDF should reopen");
    let stream = page_image_stream(&document);
    assert_eq!(
        stream.dict.get("Subtype").and_then(|v| v.as_name()),
        Some("Image"),
        "Im1 should still be an Image XObject in the saved PDF"
    );
}

#[test]
fn overlay_isolated_from_unbalanced_clipping_path() {
    // A page whose content stream sets a clipping path before drawing an
    // image (the typical rounded image) and ends without a matching `Q`
    // leaves that clip active at its end. The rewritten page content is
    // wrapped in `q` / `Q`, so the overlay starts from the initial
    // graphics state and its rectangle paints in full.

    use lj_pdf::{FillColor, RedactionMode};

    // Page content with an unbalanced clipping path:
    //
    //   q                       <- save state
    //   100 100 50 50 re        <- 50x50 rect path at (100,100)
    //   W n                     <- intersect clipping path
    //   BT /F1 12 Tf 100 130 Td (Inside clip) Tj ET
    //   <- NO matching Q: clip leaks past end
    let content = "q\n100 100 50 50 re\nW n\nBT\n/F1 12 Tf\n100 130 Td\n(Inside clip) Tj\nET\n";

    let mut pdf: Vec<u8> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let catalog_offset = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
    let page_offset = pdf.len();
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = pdf.len();
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content.as_bytes());
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let font_offset = pdf.len();
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        content_offset,
        font_offset,
    ] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    let mut document = PdfDocument::open(&pdf).expect("fixture should open");

    // Redaction target far outside the [100,100,150,150] clip rect:
    // (200, 200)..(300, 250). Under the leftover clip the overlay
    // rectangle would not paint at all.
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 200.0,
                y: 200.0,
                width: 100.0,
                height: 50.0,
            }],
            mode: Some(RedactionMode::Redact),
            fill_color: Some(FillColor { r: 255, g: 0, b: 0 }),
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    let _ = report;

    let saved = document.save().expect("save");
    // Decode the saved content stream: the rewritten page content must
    // be wrapped in q...Q so the appended overlay paints starting from
    // the initial graphics state, not the leftover clip.
    use lj_pdf::objects::{PdfObject, decode_stream};
    let document = parse_pdf(&saved).expect("saved PDF should reopen");
    let page = &document.pages[0];
    let content_ref = page.content_refs.first().expect("content stream present");
    let stream = match document.file.get_object(*content_ref).expect("content obj") {
        PdfObject::Stream(s) => s,
        _ => panic!("Contents should be a stream"),
    };
    let decoded = decode_stream(stream).expect("decode content");
    let text = String::from_utf8_lossy(&decoded);
    // Sanity: the content starts with q (wrapping the original) and
    // contains a Q before the overlay's own q...Q block.
    assert!(
        text.starts_with("q\n"),
        "rewritten content must begin with `q` to isolate the overlay; got: {:?}",
        &text[..text.len().min(40)]
    );
    // Two `q\n` markers expected (outer wrap + overlay's own block).
    let q_count = text.matches("q\n").count();
    assert!(
        q_count >= 2,
        "expected outer `q` + overlay `q`, got {q_count} in: {text}"
    );
    // Two `Q\n` markers expected (one closes the wrap, one closes the overlay).
    let big_q_count = text.matches("Q\n").count();
    assert!(
        big_q_count >= 2,
        "expected outer `Q` + overlay `Q`, got {big_q_count} in: {text}"
    );
}

fn build_pdf(objects: &[&[u8]]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[test]
fn a_page_naming_a_missing_content_stream_keeps_its_text() {
    let content = b"BT /F1 12 Tf 20 50 Td (Hello) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    // Object 6 does not exist: the rewritten content is the first object
    // the redaction allocates.
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
          /Resources << /Font << /F1 4 0 R >> >> /Contents [5 0 R 6 0 R] >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &content_object,
    ]);
    let mut document = PdfDocument::open(&pdf).expect("open");
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 150.0,
                y: 150.0,
                width: 10.0,
                height: 10.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redact");
    let reopened = PdfDocument::open(&document.save().expect("save")).expect("reopen");
    let text = reopened.extract_text(0).expect("text").text;
    assert!(text.contains("Hello"), "{text:?}");
}

#[test]
fn a_removed_annotation_leaves_nothing_of_itself_in_the_saved_file() {
    let content = b"BT /F1 12 Tf 72 700 Td (Name Here) Tj ET";
    let appearance = b"BT /F1 12 Tf 2 6 Td (Field Value) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let appearance_object = [
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 150 22] \
             /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n",
            appearance.len()
        )
        .as_bytes(),
        appearance,
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [9 0 R] >> >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots 10 0 R >>",
        &content_object,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /Parent 9 0 R /Rect [70 690 220 712] \
          /AP << /N 8 0 R >> >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [70 690 220 712] \
          /Contents (Note Text) /Popup 11 0 R >>",
        &appearance_object,
        b"<< /FT /Tx /T (name) /V (Field Value) /Kids [6 0 R] >>",
        b"[6 0 R 7 0 R 11 0 R]",
        b"<< /Type /Annot /Subtype /Popup /Parent 7 0 R /Rect [300 300 400 400] >>",
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let text = document.extract_text(0).unwrap().text;
    assert!(text.contains("Name Here") && text.contains("Field Value"));
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 60.0,
                y: 685.0,
                width: 170.0,
                height: 35.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(true),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert_eq!(report.annotations_removed, 2);
    let saved = document.save().unwrap();
    let raw = String::from_utf8_lossy(&saved);
    for leaked in ["Name Here", "Field Value", "Note Text", "Popup"] {
        assert!(!raw.contains(leaked), "{leaked} survived in the saved file");
    }
    let reopened = parse_pdf(&saved).expect("saved file should reopen");
    assert!(reopened.pages[0].annotation_refs.is_empty());
}

#[test]
fn fields_and_free_text_without_appearance_read_their_value() {
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /V (Typed Value) /DA (/Helv 10 Tf 0 g) \
          /Rect [72 700 272 720] >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Ch /T (b) /Opt [(First) [(x) (Second)]] \
          /Rect [72 600 272 650] >>",
        b"<< /Type /Annot /Subtype /FreeText /Contents <FEFF005A0061014201050063007A> \
          /DA (/Helv 12 Tf) /Rect [72 500 272 520] >>",
    ]);
    let parsed = parse_pdf(&pdf).expect("fixture should open");
    let page = lj_pdf::text::analyze_page_text(&parsed.file, 0, &parsed.pages[0]).unwrap();
    let text: String = page.glyphs.iter().map(|g| g.text).collect();
    for expected in ["Typed Value", "FirstSecond", "Za\u{142}\u{105}cz"] {
        assert!(text.contains(expected), "{expected} missing from {text}");
    }
    let typed = page.glyphs.iter().find(|g| g.text == 'T').unwrap();
    assert!((typed.bbox.x - 74.0).abs() < 0.5 && typed.bbox.y > 700.0 && typed.bbox.y < 720.0);
}

#[test]
fn text_drawn_before_any_font_or_in_a_dangling_one_and_boxes_and_kids_given_by_reference_read() {
    let content = b"BT 12 TL 72 700 Td (Hi there) Tj /F9 12 Tf ( again) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids 6 0 R >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 5 0 R 792] /Contents 4 0 R \
          /Resources << /Font << /F9 9 0 R >> >> >>",
        &content_object,
        b"612",
        b"[3 0 R]",
    ]);
    let parsed = parse_pdf(&pdf).expect("fixture should open");
    assert_eq!(parsed.pages[0].page_box.media_box.width, 612.0);
    let page = lj_pdf::text::analyze_page_text(&parsed.file, 0, &parsed.pages[0]).unwrap();
    assert!(page.text.contains("Hi there again"), "{}", page.text);
}

#[test]
fn a_page_rotated_by_90_turns_clockwise_as_viewers_show_it() {
    let content = b"BT /F1 10 Tf 10 10 Td (A) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate 90 \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        &content_object,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let parsed = parse_pdf(&pdf).expect("fixture should open");
    let page = lj_pdf::text::analyze_page_text(&parsed.file, 0, &parsed.pages[0]).unwrap();
    let glyph = page.glyphs[0].bbox;
    assert!(glyph.x > 5.0 && glyph.x < 15.0, "{glyph:?}");
    assert!(
        glyph.y + glyph.height > 185.0 && glyph.y + glyph.height < 195.0,
        "{glyph:?}"
    );
}

#[test]
fn the_crop_box_stops_at_the_media_box_and_a_negative_size_keeps_its_box() {
    let content = b"BT /F1 10 Tf 45 480 Td (A) Tj /F1 -10 Tf 100 0 Td (B) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [39 39 696 759] /CropBox [0 -39 735 759] \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        &content_object,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let parsed = parse_pdf(&pdf).expect("fixture should open");
    let page = lj_pdf::text::analyze_page_text(&parsed.file, 0, &parsed.pages[0]).unwrap();
    let (a, b) = (page.glyphs[0].bbox, page.glyphs[1].bbox);
    assert!((a.x - 6.0).abs() < 0.5, "{a:?}");
    assert!(
        b.width > 1.0 && b.height > 1.0 && b.max_x() < a.x + 100.5,
        "{b:?}"
    );
}

#[test]
fn a_damaged_file_never_panics_the_reader() {
    use rand_chacha::ChaCha8Rng;
    use rand_core::{RngCore, SeedableRng};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let original = std::fs::read(&path).unwrap();
        for round in 0..150 {
            let mut bytes = original.clone();
            for _ in 0..1 + round % 8 {
                let at = rng.next_u32() as usize % bytes.len();
                bytes[at] = rng.next_u32() as u8;
            }
            if round % 5 == 0 {
                bytes.truncate(rng.next_u32() as usize % bytes.len());
            }
            if let Ok(parsed) = parse_pdf(&bytes) {
                for (index, page) in parsed.pages.iter().enumerate() {
                    let _ = lj_pdf::text::analyze_page_text(&parsed.file, index, page);
                }
            }
        }
    }
}

#[test]
fn an_annotation_written_inside_annots_is_read_and_removed() {
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [<< /Type /Annot \
          /Subtype /FreeText /Contents (Direct Note) /DA (/Helv 12 Tf) /Rect [72 700 272 720] >>] >>",
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    assert!(
        document
            .extract_text(0)
            .unwrap()
            .text
            .contains("Direct Note")
    );
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 100.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(true),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .unwrap();
    assert_eq!(report.annotations_removed, 1);
    let saved = document.save().unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Direct Note"));
}

#[test]
fn actual_text_reads_in_place_of_its_glyphs_and_leaves_with_them() {
    use lj_pdf::objects::{PdfObject, decode_stream};
    let content = b"BT /F1 12 Tf 72 700 Td /Span <</ActualText (Martin)>> BDC (Durand) Tj EMC \
        ( est la ) Tj /Span /P0 BDC (Petit) Tj EMC ET";
    let stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    let stream = [stream.as_slice(), content, b"\nendstream"].concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources \
          << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> \
          /Properties << /P0 5 0 R >> >> >>",
        &stream,
        b"<< /ActualText (Bernard) >>",
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let text = document.extract_text(0).unwrap().text;
    assert!(text.contains("Martin est la Bernard"), "{text}");
    let quads = ["Martin", "Bernard"]
        .into_iter()
        .flat_map(|name| document.search_text(0, name).unwrap())
        .flat_map(|found| found.quads)
        .map(|quad| quad.points)
        .collect();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .unwrap();
    let saved = parse_pdf(&document.save().unwrap()).unwrap();
    let everything: String = saved
        .file
        .objects
        .values()
        .map(|object| match object {
            PdfObject::Stream(stream) => {
                format!(
                    "{:?}{}",
                    stream.dict,
                    String::from_utf8_lossy(&decode_stream(stream).unwrap())
                )
            }
            PdfObject::Value(value) => format!("{value:?}"),
        })
        .collect();
    for name in ["Martin", "Durand", "Bernard", "Petit"] {
        assert!(!everything.contains(name), "{name} stays in the saved file");
    }
    assert!(everything.contains("est la"));
}

#[test]
fn a_form_named_through_an_indirect_xobject_dictionary_is_read_and_removed() {
    use lj_pdf::objects::{PdfObject, decode_stream};
    let stream = |content: &[u8], dict: &str| {
        let head = format!("<< {dict} /Length {} >>\nstream\n", content.len()).into_bytes();
        [head.as_slice(), content, b"\nendstream"].concat()
    };
    let page = stream(b"q /X0 Do Q", "");
    let form = stream(
        b"BT /F1 12 Tf 72 700 Td (Hidden Name) Tj ET",
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 \
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>",
    );
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /XObject 5 0 R >> >>",
        &page,
        b"<< /X0 6 0 R >>",
        &form,
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let text = document.extract_text(0).unwrap().text;
    assert!(text.contains("Hidden Name"), "{text:?}");
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 60.0,
                y: 690.0,
                width: 200.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .unwrap();
    let saved = parse_pdf(&document.save().unwrap()).unwrap();
    let leaked = saved.file.objects.values().any(|object| match object {
        PdfObject::Stream(stream) => {
            String::from_utf8_lossy(&decode_stream(stream).unwrap()).contains("Hidden")
        }
        PdfObject::Value(_) => false,
    });
    assert!(!leaked, "the form text stays in the saved file");
}

#[test]
fn a_type3_glyph_named_outside_the_glyph_list_reads_its_code_in_the_base_encoding() {
    let content = b"BT /T3 12 Tf 72 700 Td (AB) Tj ET";
    let content_object = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources \
          << /Font << /T3 << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] \
          /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /BT 5 0 R /CP 5 0 R >> \
          /Encoding << /BaseEncoding /WinAnsiEncoding /Differences [65 /BT /CP] >> \
          /FirstChar 65 /LastChar 66 /Widths [500 500] >> >> >> >>",
        &content_object,
        b"<< /Length 0 >>\nstream\n\nendstream",
    ]);
    let parsed = parse_pdf(&pdf).expect("fixture should open");
    let page = lj_pdf::text::analyze_page_text(&parsed.file, 0, &parsed.pages[0]).unwrap();
    assert_eq!(page.text, "AB");
}

#[test]
fn the_actual_text_of_a_structure_element_reads_in_place_and_leaves_with_its_glyphs() {
    use lj_pdf::objects::{PdfObject, decode_stream};
    let content = b"BT /F1 12 Tf 72 700 Td /Span <</MCID 0>> BDC (aaaaaa) Tj EMC \
        ( est la) Tj ET";
    let stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    let stream = [stream.as_slice(), content, b"\nendstream"].concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /StructParents 0 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 \
          /BaseFont /Helvetica >> >> >> >>",
        &stream,
        b"<< /Type /StructTreeRoot /K [6 0 R] /ParentTree << /Nums [0 [7 0 R]] >> >>",
        b"<< /S /P /P 5 0 R /K [7 0 R] /Alt (Martin) >>",
        b"<< /S /Span /P 6 0 R /Pg 3 0 R /K 0 /ActualText (Martin) >>",
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let text = document.extract_text(0).unwrap().text;
    assert!(text.contains("Martin est la"), "{text:?}");
    let quads = document
        .search_text(0, "Martin")
        .unwrap()
        .into_iter()
        .flat_map(|found| found.quads)
        .map(|quad| quad.points)
        .collect();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .unwrap();
    let saved = parse_pdf(&document.save().unwrap()).unwrap();
    let everything: String = saved
        .file
        .objects
        .values()
        .map(|object| match object {
            PdfObject::Stream(stream) => {
                String::from_utf8_lossy(&decode_stream(stream).unwrap()).into_owned()
            }
            PdfObject::Value(value) => format!("{value:?}"),
        })
        .collect();
    assert!(
        !everything.contains("Martin"),
        "the name stays in the saved file"
    );
    assert!(!everything.contains("aaaaaa"));
    assert!(everything.contains("est la"));
}

#[test]
fn a_word_drawn_as_a_picture_reads_by_its_structure_and_leaves_with_it() {
    use lj_pdf::objects::{PdfObject, decode_stream};
    let content = b"/Span <</MCID 0>> BDC q 60 0 0 12 72 700 cm /Im0 Do Q EMC";
    let stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    let stream = [stream.as_slice(), content, b"\nendstream"].concat();
    let image = [
        b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray \
          /BitsPerComponent 8 /Length 1 >>\nstream\n"
            .as_slice(),
        &[0u8],
        b"\nendstream",
    ]
    .concat();
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /StructParents 0 /Resources << /XObject << /Im0 7 0 R >> >> >>",
        &stream,
        b"<< /Type /StructTreeRoot /K [6 0 R] >>",
        b"<< /S /Span /P 5 0 R /Pg 3 0 R /K 0 /ActualText (Martin) >>",
        &image,
    ]);
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    assert!(document.extract_text(0).unwrap().text.contains("Martin"));
    let quads: Vec<_> = document
        .search_text(0, "Martin")
        .unwrap()
        .into_iter()
        .flat_map(|found| found.quads)
        .map(|quad| quad.points)
        .collect();
    assert!(!quads.is_empty());
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .unwrap();
    assert!(report.image_draws_removed + report.image_draws_masked > 0);
    let saved = parse_pdf(&document.save().unwrap()).unwrap();
    let leaked = saved.file.objects.values().any(|object| match object {
        PdfObject::Stream(stream) => {
            String::from_utf8_lossy(&decode_stream(stream).unwrap()).contains("Martin")
        }
        PdfObject::Value(value) => format!("{value:?}").contains("Martin"),
    });
    assert!(!leaked, "the name stays in the saved file");
}

#[test]
fn a_page_named_twice_keeps_the_redactions_of_both_copies() {
    let content =
        b"BT /F1 24 Tf 72 700 Td (Secret Alpha) Tj ET BT /F1 24 Tf 72 600 Td (Beta Gamma) Tj ET";
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>\nendobj\n");
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n",
    );
    let mut document = PdfDocument::open(&pdf).expect("fixture should open");
    let rect = |page_index, y| RedactionTarget::Rect {
        page_index,
        x: 70.0,
        y,
        width: 200.0,
        height: 30.0,
    };
    document
        .apply_redactions(RedactionPlan {
            targets: vec![rect(0, 695.0), rect(1, 595.0)],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    for (page_index, document) in [0, 1]
        .into_iter()
        .flat_map(|i| [(i, &document), (i, &reopened)])
    {
        let text = document.extract_text(page_index).expect("text").text;
        assert!(!text.contains("Secret"), "page {page_index}: {text}");
        assert!(!text.contains("Beta"), "page {page_index}: {text}");
    }
}

/// Two pages sharing one resource dictionary, each drawing its own image over
/// the whole page: `SCRT` on the first, `OKOK` on the second.
fn build_shared_resources_scan_pdf() -> Vec<u8> {
    let stream = |content: &[u8]| {
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat()
    };
    let image = |pixels: &str| {
        format!(
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Length 4 >>\nstream\n{pixels}\nendstream"
        )
    };
    build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources 5 0 R /Contents 6 0 R >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources 5 0 R /Contents 7 0 R >>",
        b"<< /XObject << /Im1 8 0 R /Im2 9 0 R >> >>",
        &stream(b"q 100 0 0 100 50 50 cm /Im1 Do Q"),
        &stream(b"q 100 0 0 100 50 50 cm /Im2 Do Q"),
        image("SCRT").as_bytes(),
        image("OKOK").as_bytes(),
    ])
}

#[test]
fn an_image_of_a_shared_resource_dictionary_leaves_the_file_once_no_page_draws_it() {
    for (width, removed, masked) in [(200.0, 1, 0), (50.0, 0, 1)] {
        let mut document = PdfDocument::open(&build_shared_resources_scan_pdf()).expect("open");
        let report = document
            .apply_redactions(RedactionPlan {
                targets: vec![RedactionTarget::Rect {
                    page_index: 0,
                    x: 50.0,
                    y: 50.0,
                    width,
                    height: width,
                }],
                mode: None,
                fill_color: None,
                overlay_text: None,
                remove_intersecting_annotations: Some(false),
                strip_metadata: Some(false),
                strip_attachments: Some(false),
                sanitize_hidden_ocgs: None,
            })
            .expect("redact");
        assert_eq!(
            (report.image_draws_removed, report.image_draws_masked),
            (removed, masked)
        );
        let saved = document.save().expect("save");
        let reopened = parse_pdf(&saved).expect("reopen");
        let pixels: Vec<&[u8]> = reopened
            .file
            .objects
            .values()
            .filter_map(|object| match object {
                lj_pdf::objects::PdfObject::Stream(s) => Some(s.data.as_slice()),
                _ => None,
            })
            .collect();
        assert!(!pixels.contains(&b"SCRT".as_slice()), "width {width}");
        assert!(pixels.contains(&b"OKOK".as_slice()), "width {width}");
    }
}

/// Eight forms, each calling the next ten times: 10^7 glyphs from 2 KB.
fn fanned_forms_pdf() -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R \
         /Resources << /XObject << /X 6 0 R >> >> >>"
            .to_string(),
        "<< /Length 5 >>\nstream\n/X Do\nendstream".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for level in 0..8 {
        let (body, resources) = match level {
            7 => (
                "BT /F 12 Tf 10 10 Td (a) Tj ET".to_string(),
                "<< /Font << /F 5 0 R >> >>".to_string(),
            ),
            _ => (
                ["q /X Do Q"; 10].join(" "),
                format!("<< /XObject << /X {} 0 R >> >>", 7 + level),
            ),
        };
        objects.push(format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources {resources} \
             /Length {} >>\nstream\n{body}\nendstream",
            body.len()
        ));
    }
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    pdf.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    pdf
}

#[test]
fn forms_fanned_out_past_the_operation_budget_are_refused() {
    let document = PdfDocument::open(&fanned_forms_pdf()).expect("fanned pdf should open");
    assert!(matches!(
        document.extract_text(0),
        Err(PdfError::Unsupported(_))
    ));
    assert!(matches!(
        document.draw_page(0),
        Err(PdfError::Unsupported(_))
    ));
}

/// Every hit of `words` on the first page as one target, as the workbench
/// groups a page's hits, applied; the bytes saved and the quads painted.
fn redact_words(bytes: &[u8], words: &[&str]) -> (Vec<u8>, usize) {
    let mut document = PdfDocument::open(bytes).expect("fixture should open");
    let mut quads = Vec::new();
    for word in words {
        for hit in document
            .search_text(0, word)
            .expect("search should succeed")
        {
            quads.extend(hit.quads.iter().map(|quad| quad.points));
        }
    }
    let painted = quads.len();
    document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(true),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    (document.save().expect("save should succeed"), painted)
}

fn filled_shapes(items: &[lj_draw::Item]) -> usize {
    items
        .iter()
        .map(|item| match item {
            lj_draw::Item::Shape { fill: Some(_), .. } => 1,
            lj_draw::Item::Group(group) => filled_shapes(&group.items),
            _ => 0,
        })
        .sum()
}

fn page_fills(bytes: &[u8]) -> usize {
    let page = PdfDocument::open(bytes)
        .and_then(|document| document.draw_page(0))
        .expect("page should draw");
    filled_shapes(&page.items)
}

/// A later pass spans "Secret" and "Gamma", and the box the first pass painted
/// over "Alpha" lies between them: it stays.
#[test]
fn a_later_pass_keeps_the_boxes_lying_between_its_hits() {
    let source = fixture("simple-text.pdf");
    let (first, alpha) = redact_words(&source, &["Alpha"]);
    assert!(alpha > 0);
    assert_eq!(page_fills(&first), page_fills(&source) + alpha);
    let (second, around) = redact_words(&first, &["Secret", "Gamma"]);
    assert!(around > 0);
    assert_eq!(page_fills(&second), page_fills(&first) + around);
}
