use crate::graphics::{Color, Point, Quad, Rect};
use crate::objects::{PdfError, PdfResult};
use serde::{Deserialize, Serialize};

/// Controls the visual output of text redaction.
///
/// - `Strip`, physically remove bytes; text shifts, no overlay.
/// - `Redact`, replace bytes with kern compensation, draw colored overlay. **(default)**
/// - `Erase`, replace bytes with kern compensation, no overlay (blank space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactionMode {
    Strip,
    #[default]
    Redact,
    Erase,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<FillColor> for Color {
    fn from(value: FillColor) -> Self {
        Color {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RectTarget {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "pageIndex")]
    pub page_index: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuadTarget {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "pageIndex")]
    pub page_index: usize,
    pub points: [Point; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuadTargetGroup {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "pageIndex")]
    pub page_index: usize,
    pub quads: Vec<[Point; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum RedactionTarget {
    #[serde(rename = "rect")]
    Rect {
        #[serde(rename = "pageIndex")]
        page_index: usize,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    #[serde(rename = "quad")]
    Quad {
        #[serde(rename = "pageIndex")]
        page_index: usize,
        points: [Point; 4],
    },
    #[serde(rename = "quadGroup")]
    QuadGroup {
        #[serde(rename = "pageIndex")]
        page_index: usize,
        quads: Vec<[Point; 4]>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactionPlan {
    pub targets: Vec<RedactionTarget>,
    pub mode: Option<RedactionMode>,
    #[serde(rename = "fillColor")]
    pub fill_color: Option<FillColor>,
    #[serde(rename = "overlayText")]
    pub overlay_text: Option<String>,
    #[serde(rename = "removeIntersectingAnnotations")]
    pub remove_intersecting_annotations: Option<bool>,
    #[serde(rename = "stripMetadata")]
    pub strip_metadata: Option<bool>,
    #[serde(rename = "stripAttachments")]
    pub strip_attachments: Option<bool>,
    /// When `Some(true)`, the engine accepts documents whose catalog
    /// declares hidden-by-default Optional Content Groups. Content
    /// stream runs gated by those OCGs (`BDC /OC /name ... EMC`) are
    /// stripped so the output carries neither the hidden bytes nor the
    /// visible-but-previously-redacted ones. Defaults to rejecting such
    /// documents (`None` / `Some(false)`).
    #[serde(rename = "sanitizeHiddenOcgs")]
    pub sanitize_hidden_ocgs: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct NormalizedPageTarget {
    pub page_index: usize,
    pub quads: Vec<Quad>,
    pub bounds: Rect,
}

impl NormalizedPageTarget {
    pub fn intersects_rect(&self, rect: &Rect) -> bool {
        self.quads.iter().any(|quad| quad.intersects_rect(rect))
    }

    pub fn intersects_quad(&self, quad: &Quad) -> bool {
        self.quads
            .iter()
            .any(|candidate| candidate.intersects_quad(quad))
    }
}

#[derive(Debug, Clone)]
pub struct NormalizedRedactionPlan {
    pub targets: Vec<NormalizedPageTarget>,
    pub mode: RedactionMode,
    pub fill_color: Color,
    pub overlay_text: Option<String>,
    pub remove_intersecting_annotations: bool,
    pub strip_metadata: bool,
    pub strip_attachments: bool,
    pub sanitize_hidden_ocgs: bool,
}

pub fn normalize_plan(
    plan: RedactionPlan,
    page_sizes: &[crate::graphics::Size],
) -> PdfResult<NormalizedRedactionPlan> {
    if let Some(text) = plan.overlay_text.as_deref() {
        if text.is_empty() {
            return Err(PdfError::UnsupportedOption(
                "overlayText must not be empty".to_string(),
            ));
        }
        if plan.mode == Some(RedactionMode::Strip) {
            return Err(PdfError::UnsupportedOption(
                "overlayText requires mode \"redact\" (strip mode paints nothing)".to_string(),
            ));
        }
    }

    let mut targets = Vec::new();
    for target in plan.targets {
        let normalized = match target {
            RedactionTarget::Rect {
                page_index,
                x,
                y,
                width,
                height,
            } => {
                validate_page(page_index, page_sizes)?;
                let rect = Rect {
                    x,
                    y,
                    width,
                    height,
                }
                .normalize();
                validate_rect(page_index, rect, page_sizes)?;
                NormalizedPageTarget {
                    page_index,
                    quads: vec![rect.to_quad()],
                    bounds: rect,
                }
            }
            RedactionTarget::Quad { page_index, points } => {
                validate_page(page_index, page_sizes)?;
                let quad = Quad { points };
                validate_rect(page_index, quad.bounding_rect(), page_sizes)?;
                NormalizedPageTarget {
                    page_index,
                    quads: vec![quad],
                    bounds: quad.bounding_rect(),
                }
            }
            RedactionTarget::QuadGroup { page_index, quads } => {
                validate_page(page_index, page_sizes)?;
                if quads.is_empty() {
                    return Err(PdfError::Parse(
                        "quadGroup target must contain at least one quad".to_string(),
                    ));
                }
                let quads = quads
                    .into_iter()
                    .map(|points| Quad { points })
                    .collect::<Vec<_>>();
                let mut bounds = quads[0].bounding_rect();
                for quad in &quads[1..] {
                    bounds = bounds.union(&quad.bounding_rect());
                }
                validate_rect(page_index, bounds, page_sizes)?;
                NormalizedPageTarget {
                    page_index,
                    quads,
                    bounds,
                }
            }
        };
        targets.push(normalized);
    }

    Ok(NormalizedRedactionPlan {
        targets,
        mode: plan.mode.unwrap_or_default(),
        fill_color: plan.fill_color.unwrap_or_default().into(),
        overlay_text: plan.overlay_text,
        remove_intersecting_annotations: plan.remove_intersecting_annotations.unwrap_or(true),
        strip_metadata: plan.strip_metadata.unwrap_or(false),
        strip_attachments: plan.strip_attachments.unwrap_or(false),
        sanitize_hidden_ocgs: plan.sanitize_hidden_ocgs.unwrap_or(false),
    })
}

fn validate_page(page_index: usize, page_sizes: &[crate::graphics::Size]) -> PdfResult<()> {
    if page_index >= page_sizes.len() {
        return Err(PdfError::InvalidPageIndex(page_index));
    }
    Ok(())
}

fn validate_rect(
    page_index: usize,
    rect: Rect,
    page_sizes: &[crate::graphics::Size],
) -> PdfResult<()> {
    let size = page_sizes[page_index];
    let page_rect = Rect {
        x: 0.0,
        y: 0.0,
        width: size.width,
        height: size.height,
    };
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(PdfError::Parse(
            "redaction target dimensions must be positive".to_string(),
        ));
    }
    if !page_rect.intersects(&rect) {
        return Err(PdfError::Parse(
            "redaction target does not intersect the page bounds".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::graphics::{Point, Size};

    use super::{RedactionPlan, RedactionTarget, normalize_plan};

    #[test]
    fn normalizes_rect_targets() {
        let plan = RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 10.0,
                y: 20.0,
                width: 50.0,
                height: 40.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: None,
            strip_metadata: None,
            strip_attachments: None,
            sanitize_hidden_ocgs: None,
        };
        let normalized = normalize_plan(
            plan,
            &[Size {
                width: 200.0,
                height: 300.0,
            }],
        )
        .expect("plan should normalize");
        assert_eq!(normalized.targets.len(), 1);
    }

    #[test]
    fn rejects_quad_group_without_quads() {
        let plan = RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads: Vec::<[Point; 4]>::new(),
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: None,
            strip_metadata: None,
            strip_attachments: None,
            sanitize_hidden_ocgs: None,
        };
        assert!(
            normalize_plan(
                plan,
                &[Size {
                    width: 100.0,
                    height: 100.0,
                }]
            )
            .is_err()
        );
    }
}
