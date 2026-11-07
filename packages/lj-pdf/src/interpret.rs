//! One walk over a page's content, its forms and its annotations, following
//! the graphics state and handing what it meets to a device: the text
//! extraction or the drawing.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use lj_draw::{Blend, Cap, Join, Rgb};

use crate::content::{Operation, page_content_bytes, parse_content_stream};
use crate::draw::color::{Color, ColorSpace, Tile};
use crate::draw::pattern::Pattern;
use crate::draw::shading::Shading;
use crate::draw::state::{ClipPath, GraphicsState, PathBuilder, TextState};
use crate::graphics::{Matrix, Rect};
use crate::objects::{
    ObjectRef, PageInfo, PdfDictionary, PdfError, PdfFile, PdfResult, PdfStream, PdfString,
    PdfValue, decode_stream, document::get_stream,
};
use crate::text::{
    Appearance, DecodedGlyph, Fonts, LoadedFont, ShowOperand, annotation_appearances,
    decode_font_glyphs, lookup_xobject_ref, matrix_from_operands, matrix_from_pdf_values,
    operand_name, operand_number, operand_string,
    program::{Drawn, Type3, UNITS},
};

/// Maximum Form XObject recursion depth. Prevents pathological or adversarial
/// PDFs from driving the interpreter into unbounded recursion.
const MAX_FORM_XOBJECT_DEPTH: usize = 16;

/// Operations run inside forms, Type3 glyphs, tiles and soft masks, per page.
/// Forms that each call the next ten times reach 10^7 rectangles from 2 KB,
/// where a real page stays well under the cap.
const MAX_NESTED_OPERATIONS: usize = 2_000_000;

/// One glyph of a shown string, before the text matrix moves past it.
pub(crate) struct ShownGlyph<'a> {
    pub font: &'a LoadedFont,
    pub decoded: &'a DecodedGlyph,
    /// Text space to the normalised page.
    pub text_to_page: Matrix,
    /// The glyph's box in text space, as the extraction reads it.
    pub local_rect: Rect,
    /// The glyph's own space, one unit an em, origin on the pen, to the
    /// normalised page.
    pub em_to_page: Matrix,
}

/// An image XObject, or an inline image when the walk draws.
pub(crate) struct ShownImage<'a> {
    pub operation_index: usize,
    /// The XObject, `None` for an inline image.
    pub source: Option<ObjectRef>,
    /// Keys spelled out in full.
    pub dict: &'a PdfDictionary,
    /// Still encoded.
    pub data: &'a [u8],
    /// Where a colour space the image names is looked up.
    pub resources: &'a PdfDictionary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PathPaint {
    /// `Some(even_odd)` when the path is filled.
    pub fill: Option<bool>,
    pub stroke: bool,
}

/// What the walk hands over. Paths, clips and colours are only followed for a
/// device that draws.
pub(crate) trait Device {
    const DRAWS: bool;

    /// A new line of text starts: a vertical move, `T*`, `'`, `"`, or an
    /// annotation.
    fn line_break(&mut self) {}
    /// `BMC` or `BDC`.
    fn begin_marked(&mut self, _file: &PdfFile, _resources: &PdfDictionary, _op: &Operation) {}
    fn end_marked(&mut self) {}
    fn begin_show(&mut self, _operation_index: usize, _operand: ShowOperand) {}
    fn glyph(&mut self, _glyph: &ShownGlyph<'_>, _state: &GraphicsState) {}
    fn end_show(&mut self) {}
    /// An image XObject, drawn over the unit square the CTM maps.
    fn image(&mut self, _file: &PdfFile, _image: &ShownImage<'_>, _state: &GraphicsState) {}
    /// A form's content starts, its matrix already in the CTM; `group` when
    /// it is a transparency group.
    fn begin_form(
        &mut self,
        _form: ObjectRef,
        _group: Option<TransparencyGroup>,
        _state: &GraphicsState,
    ) {
    }
    fn end_form(&mut self) {}
    /// A path in user space, its points under `state.ctm`.
    fn paint(&mut self, _path: &[lj_draw::Segment], _paint: PathPaint, _state: &GraphicsState) {}
    /// `sh`: the shading fills the clip, in user space.
    fn shading(&mut self, _shading: &Shading, _state: &GraphicsState) {}
    /// A soft mask's group starts, drawn where it stands on the page.
    fn begin_mask(&mut self) {}
    /// The mask drawn, by the luminosity of its group over `backdrop`, black
    /// by default, or by its alpha.
    fn end_mask(&mut self, _luminosity: bool, _backdrop: Option<Rgb>) -> Option<usize> {
        None
    }
    /// Whether the device still lacks the cell of `tile`.
    fn wants_tile(&self, _tile: Tile) -> bool {
        false
    }
    /// A tiling pattern's cell starts, drawn in the pattern's own space.
    fn begin_tile(&mut self) {}
    fn end_tile(&mut self, _tile: Tile) {}
    /// An ExtGState, once the walk has applied its line, alpha, blend and
    /// soft mask entries.
    fn ext_gstate(&mut self, _file: &PdfFile, _dict: &PdfDictionary, _state: &mut GraphicsState) {}
}

struct Scope<'r> {
    resources: &'r PdfDictionary,
    fonts: &'r Fonts<'r>,
}

struct Walk<'a, D> {
    file: &'a PdfFile,
    page_transform: Matrix,
    state: GraphicsState,
    stack: Vec<GraphicsState>,
    forms: BTreeSet<ObjectRef>,
    path: PathBuilder,
    /// `Some(even_odd)` between `W` and the operator that ends the path.
    clip: Option<bool>,
    /// The boxes of the glyphs a text object shows in a clipping mode, on the
    /// normalised page: the clip it leaves.
    text_clip: Vec<lj_draw::Segment>,
    /// Inside a Type3 procedure that `d1` opens, which paints in the colour
    /// of the text rather than its own.
    uncoloured: bool,
    /// Type3 procedures already parsed on this page.
    procedures: HashMap<ObjectRef, Rc<[Operation]>>,
    /// The optional content groups a viewer hides, read when drawing.
    hidden_groups: BTreeSet<ObjectRef>,
    /// For each open marked-content sequence, whether it hides what it holds.
    marked: Vec<bool>,
    /// Colour spaces, shadings and patterns read once per page, `None` when
    /// unreadable.
    spaces: HashMap<ObjectRef, Option<Arc<ColorSpace>>>,
    shadings: HashMap<ObjectRef, Option<Rc<Shading>>>,
    patterns: HashMap<ObjectRef, Option<Rc<Pattern>>>,
    /// While the group of a soft mask is drawn.
    masking: bool,
    /// Inside an isolated knockout group.
    knockout: bool,
    nested_operations: usize,
    device: D,
}

/// Walks the page's content, then the appearance of each annotation a viewer
/// shows.
pub(crate) fn walk_page<D: Device>(file: &PdfFile, page: &PageInfo, device: D) -> PdfResult<D> {
    let operations = parse_content_stream(&page_content_bytes(file, page))?.operations;
    let fonts = Fonts::new(file, &page.resources, D::DRAWS, None)?;
    let scope = Scope {
        resources: &page.resources,
        fonts: &fonts,
    };
    let mut walk = Walk {
        file,
        page_transform: page.page_box.normalized_transform(),
        state: GraphicsState::default(),
        stack: Vec::new(),
        forms: BTreeSet::new(),
        path: PathBuilder::default(),
        clip: None,
        text_clip: Vec::new(),
        uncoloured: false,
        procedures: HashMap::new(),
        hidden_groups: match D::DRAWS {
            true => crate::redact::collect_hidden_ocg_refs(file)?,
            false => BTreeSet::new(),
        },
        marked: Vec::new(),
        spaces: HashMap::new(),
        shadings: HashMap::new(),
        patterns: HashMap::new(),
        masking: false,
        knockout: false,
        nested_operations: 0,
        device,
    };
    walk.run_operations(&operations, &scope, 0)?;

    for (appearance, rect) in annotation_appearances(file, page) {
        walk.state = GraphicsState {
            text: walk.state.text.clone(),
            ..GraphicsState::default()
        };
        walk.device.line_break();
        match appearance {
            Appearance::Stream(stream_ref) => walk.run_form(stream_ref, Some(rect), &scope, 0)?,
            Appearance::Drawn(annotation_ref, stream) => {
                walk.run_form_stream(annotation_ref, &stream, Some(rect), &scope, 0)?
            }
        }
    }
    walk.within_budget()?;
    Ok(walk.device)
}

impl<D: Device> Walk<'_, D> {
    fn run_operations(
        &mut self,
        operations: &[Operation],
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        if depth > 0 {
            self.nested_operations += operations.len();
            self.within_budget()?;
        }
        for (operation_index, operation) in operations.iter().enumerate() {
            // A malformed operation draws nothing in a viewer, and the page goes on.
            match self.operation(operation_index, operation, scope, depth) {
                Err(PdfError::Corrupt(_) | PdfError::Parse(_)) => {}
                other => other?,
            }
        }
        Ok(())
    }

    fn operation(
        &mut self,
        operation_index: usize,
        operation: &Operation,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        let text = &mut self.state.text;
        match operation.operator.as_str() {
            "q" => self.stack.push(self.state.clone()),
            "Q" => self.state = self.stack.pop().unwrap_or_default(),
            "gs" => {
                let gs_name = operand_name(operation, 0)?;
                if let Some((font_key, font_size)) = scope.fonts.state_font(gs_name) {
                    text.font = Some(font_key);
                    text.font_size = font_size;
                }
                if D::DRAWS {
                    self.ext_gstate(scope, gs_name, depth);
                }
            }
            "cm" => {
                let matrix = matrix_from_operands(&operation.operands)?;
                self.state.ctm = matrix.multiply(self.state.ctm);
            }
            "BT" => {
                text.text_matrix = Matrix::identity();
                text.line_matrix = Matrix::identity();
                self.text_clip.clear();
            }
            "ET" => {
                if !self.text_clip.is_empty() {
                    self.state.clips.push(Rc::new(ClipPath {
                        segments: std::mem::take(&mut self.text_clip),
                        transform: Matrix::identity(),
                        even_odd: false,
                    }));
                }
            }
            "Tf" => {
                let resource_name = operand_name(operation, 0)?;
                text.font = Some(resource_name.to_string());
                text.font_size = operand_number(operation, 1)?;
            }
            "Tm" => {
                text.text_matrix = matrix_from_operands(&operation.operands)?;
                text.line_matrix = text.text_matrix;
            }
            "Td" => {
                let tx = operand_number(operation, 0)?;
                let ty = operand_number(operation, 1)?;
                // A text-space translation composes before the line matrix.
                text.line_matrix = Matrix::translate(tx, ty).multiply(text.line_matrix);
                text.text_matrix = text.line_matrix;
                if ty.abs() > f64::EPSILON {
                    self.device.line_break();
                }
            }
            "TD" => {
                let tx = operand_number(operation, 0)?;
                let ty = operand_number(operation, 1)?;
                text.leading = -ty;
                text.line_matrix = Matrix::translate(tx, ty).multiply(text.line_matrix);
                text.text_matrix = text.line_matrix;
                self.device.line_break();
            }
            "T*" => {
                text.line_matrix = Matrix::translate(0.0, -text.leading).multiply(text.line_matrix);
                text.text_matrix = text.line_matrix;
                self.device.line_break();
            }
            "Tc" => text.character_spacing = operand_number(operation, 0)?,
            "Tw" => text.word_spacing = operand_number(operation, 0)?,
            "TL" => text.leading = operand_number(operation, 0)?,
            "Tr" => {
                text.text_render_mode = operation
                    .operands
                    .first()
                    .and_then(PdfValue::as_integer)
                    .unwrap_or(0);
            }
            "Ts" => text.text_rise = operand_number(operation, 0)?,
            "Tz" => text.horizontal_scaling = operand_number(operation, 0)?,
            "Tj" => {
                let string = operand_string(operation, 0)?;
                let operand = ShowOperand::Direct { operand_index: 0 };
                self.show_text(operation_index, operand, string, scope, depth)?;
            }
            "'" => {
                text.line_matrix = Matrix::translate(0.0, -text.leading).multiply(text.line_matrix);
                text.text_matrix = text.line_matrix;
                self.device.line_break();
                let string = operand_string(operation, 0)?;
                let operand = ShowOperand::Direct { operand_index: 0 };
                self.show_text(operation_index, operand, string, scope, depth)?;
            }
            "\"" => {
                text.word_spacing = operand_number(operation, 0)?;
                text.character_spacing = operand_number(operation, 1)?;
                text.line_matrix = Matrix::translate(0.0, -text.leading).multiply(text.line_matrix);
                text.text_matrix = text.line_matrix;
                self.device.line_break();
                let string = operand_string(operation, 2)?;
                let operand = ShowOperand::Direct { operand_index: 2 };
                self.show_text(operation_index, operand, string, scope, depth)?;
            }
            "TJ" => {
                let segments = operation
                    .operands
                    .first()
                    .and_then(PdfValue::as_array)
                    .ok_or_else(|| PdfError::Corrupt("TJ expects an array operand".to_string()))?;
                for (element_index, segment) in segments.iter().enumerate() {
                    match segment {
                        PdfValue::String(string) => {
                            let operand = ShowOperand::Array {
                                operand_index: 0,
                                element_index,
                            };
                            self.show_text(operation_index, operand, string, scope, depth)?;
                        }
                        value => {
                            let Some(adjustment) = value.as_number() else {
                                continue;
                            };
                            let text = &mut self.state.text;
                            let shift = -(adjustment / 1000.0) * text.font_size;
                            let vertical = text
                                .font
                                .as_ref()
                                .and_then(|name| scope.fonts.get(name))
                                .is_some_and(|font| font.vertical());
                            let step = match vertical {
                                true => Matrix::translate(0.0, shift),
                                false => Matrix::translate(
                                    shift * (text.horizontal_scaling / 100.0),
                                    0.0,
                                ),
                            };
                            text.text_matrix = step.multiply(text.text_matrix);
                        }
                    }
                }
            }
            "BI" => {
                if let [PdfValue::Dictionary(written), PdfValue::String(data)] =
                    operation.operands.as_slice()
                    && !self.hidden(None)
                {
                    let dict = crate::draw::image::inline_dict(written);
                    let image = ShownImage {
                        operation_index,
                        source: None,
                        dict: &dict,
                        data: &data.0,
                        resources: scope.resources,
                    };
                    self.device.image(self.file, &image, &self.state);
                }
            }
            "BMC" | "BDC" => {
                let hides = D::DRAWS
                    && operation.operands.first().and_then(PdfValue::as_name) == Some("OC")
                    && self.hides(marked_properties(self.file, scope.resources, operation));
                self.marked.push(hides);
                self.device
                    .begin_marked(self.file, scope.resources, operation)
            }
            "EMC" => {
                self.marked.pop();
                self.device.end_marked()
            }
            "Do" => {
                let name = operand_name(operation, 0)?;
                if let Some(xobject) = lookup_xobject_ref(self.file, scope.resources, name)
                    && let Ok(stream) = get_stream(self.file, xobject)
                    && stream.dict.get("Subtype").and_then(PdfValue::as_name) == Some("Image")
                    && !self.hidden(stream.dict.get("OC"))
                {
                    let image = ShownImage {
                        operation_index,
                        source: Some(xobject),
                        dict: &stream.dict,
                        data: &stream.data,
                        resources: scope.resources,
                    };
                    self.device.image(self.file, &image, &self.state);
                }
                self.enter_form_xobject(name, scope, depth)?;
            }
            _ if D::DRAWS => self.draw_operation(operation, scope, depth)?,
            _ => {}
        }
        Ok(())
    }

    /// The operators only a drawing reads: paths, clips, line style, colour.
    fn draw_operation(
        &mut self,
        operation: &Operation,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        let n = |index: usize| operand_number(operation, index);
        let state = &mut self.state;
        let colour = matches!(
            operation.operator.as_str(),
            "g" | "G" | "rg" | "RG" | "k" | "K" | "cs" | "CS" | "sc" | "scn" | "SC" | "SCN"
        );
        if colour && self.uncoloured {
            return Ok(());
        }
        match operation.operator.as_str() {
            "m" => self.path.move_to([n(0)?, n(1)?]),
            "l" => self.path.line_to([n(0)?, n(1)?]),
            "c" => self
                .path
                .curve_to(Some([n(0)?, n(1)?]), Some([n(2)?, n(3)?]), [n(4)?, n(5)?]),
            "v" => self
                .path
                .curve_to(None, Some([n(0)?, n(1)?]), [n(2)?, n(3)?]),
            "y" => self
                .path
                .curve_to(Some([n(0)?, n(1)?]), None, [n(2)?, n(3)?]),
            "h" => self.path.close(),
            "re" => self.path.rect(n(0)?, n(1)?, n(2)?, n(3)?),
            "S" => self.end_path(false, None, true, scope, depth),
            "s" => self.end_path(true, None, true, scope, depth),
            "f" | "F" => self.end_path(false, Some(false), false, scope, depth),
            "f*" => self.end_path(false, Some(true), false, scope, depth),
            "B" => self.end_path(false, Some(false), true, scope, depth),
            "B*" => self.end_path(false, Some(true), true, scope, depth),
            "b" => self.end_path(true, Some(false), true, scope, depth),
            "b*" => self.end_path(true, Some(true), true, scope, depth),
            "n" => self.end_path(false, None, false, scope, depth),
            "W" => self.clip = Some(false),
            "W*" => self.clip = Some(true),
            "w" => state.line_width = n(0)?,
            "J" => state.cap = cap(n(0)?),
            "j" => state.join = join(n(0)?),
            "M" => state.miter_limit = n(0)?,
            "d" => {
                let dash = operation
                    .operands
                    .first()
                    .and_then(PdfValue::as_array)
                    .ok_or_else(|| PdfError::Corrupt("d expects an array".to_string()))?;
                state.dash = dash.iter().filter_map(PdfValue::as_number).collect();
                state.dash_phase = n(1).unwrap_or(0.0);
            }
            "g" | "G" | "rg" | "RG" | "k" | "K" => {
                let space = match operation.operator.as_str() {
                    "g" | "G" => ColorSpace::Gray,
                    "rg" | "RG" => ColorSpace::Rgb,
                    _ => ColorSpace::Cmyk,
                };
                let mut color = Color::new(Arc::new(space));
                color.set(&numbers(operation));
                match operation.operator.as_str() {
                    "g" | "rg" | "k" => state.fill = color,
                    _ => state.stroke = color,
                }
            }
            "cs" | "CS" => {
                let name = operand_name(operation, 0)?;
                let space = self.color_space(scope.resources, name)?;
                let color = Color::new(space);
                match operation.operator.as_str() {
                    "cs" => self.state.fill = color,
                    _ => self.state.stroke = color,
                }
            }
            "sc" | "scn" | "SC" | "SCN" => {
                let pattern = match operation.operands.last() {
                    Some(PdfValue::Name(name)) => self.pattern(scope.resources, name),
                    _ => None,
                };
                let color = match operation.operator.as_str() {
                    "sc" | "scn" => &mut self.state.fill,
                    _ => &mut self.state.stroke,
                };
                color.set(&numbers(operation));
                if let ColorSpace::Pattern(_) = *color.space {
                    color.pattern = pattern;
                }
            }
            "sh" if !self.hidden(None) => {
                let name = operand_name(operation, 0)?;
                if let Some(value) = resource(self.file, scope.resources, "Shading", name)
                    && let Some(shading) = self.shading(value)
                {
                    self.device.shading(&shading, &self.state);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Paints the current path, then makes it the clip if `W` asked.
    fn end_path(
        &mut self,
        close: bool,
        fill: Option<bool>,
        stroke: bool,
        scope: &Scope<'_>,
        depth: usize,
    ) {
        if close {
            self.path.close();
        }
        let segments = self.path.take();
        let clip = self.clip.take();
        if segments.is_empty() {
            return;
        }
        if (fill.is_some() || stroke) && !self.hidden(None) {
            if fill.is_some() {
                self.prepare_tile(true, scope, depth);
            }
            if stroke {
                self.prepare_tile(false, scope, depth);
            }
            self.device
                .paint(&segments, PathPaint { fill, stroke }, &self.state);
        }
        if let Some(even_odd) = clip {
            self.state.clips.push(Rc::new(ClipPath {
                segments,
                transform: self.state.ctm.multiply(self.page_transform),
                even_odd,
            }));
        }
    }

    fn ext_gstate(&mut self, scope: &Scope<'_>, name: &str, depth: usize) {
        let file = self.file;
        let Some(dict) = scope
            .resources
            .get("ExtGState")
            .and_then(|all| file.resolve_dict(all).ok())
            .and_then(|all| all.get(name))
            .and_then(|dict| file.resolve_dict(dict).ok())
        else {
            return;
        };
        let state = &mut self.state;
        let number = |key: &str| {
            dict.get(key)
                .and_then(|v| file.resolve(v).ok())
                .and_then(PdfValue::as_number)
        };
        if let Some(width) = number("LW") {
            state.line_width = width;
        }
        if let Some(value) = number("LC") {
            state.cap = cap(value);
        }
        if let Some(value) = number("LJ") {
            state.join = join(value);
        }
        if let Some(limit) = number("ML") {
            state.miter_limit = limit;
        }
        if let Some(alpha) = number("CA") {
            state.stroke_alpha = alpha.clamp(0.0, 1.0);
        }
        if let Some(alpha) = number("ca") {
            state.fill_alpha = alpha.clamp(0.0, 1.0);
        }
        if let Some(dash) = dict
            .get("D")
            .and_then(|v| file.resolve(v).ok())
            .and_then(PdfValue::as_array)
        {
            state.dash = dash
                .first()
                .and_then(|v| file.resolve(v).ok())
                .and_then(PdfValue::as_array)
                .map(|array| array.iter().filter_map(PdfValue::as_number).collect())
                .unwrap_or_default();
            state.dash_phase = dash.get(1).and_then(PdfValue::as_number).unwrap_or(0.0);
        }
        if let Some(mode) = dict.get("BM").and_then(|v| file.resolve(v).ok()) {
            let name = match mode {
                PdfValue::Array(modes) => modes.first().and_then(PdfValue::as_name),
                other => other.as_name(),
            };
            state.blend = name.and_then(blend).filter(|_| !self.knockout);
        }
        if let Some(mask) = dict.get("SMask") {
            self.state.soft_mask = None;
            // A soft mask inside the group of another is left out: a
            // rasteriser draws each mask again for every use of the outer one.
            if !self.masking
                && let Ok(mask) = file.resolve_dict(mask)
            {
                self.state.soft_mask = self.soft_mask(mask, scope, depth);
            }
        }
        self.device.ext_gstate(file, dict, &mut self.state);
    }

    /// Draws the group of a soft mask for the device, placed by the CTM in
    /// force.
    fn soft_mask(
        &mut self,
        mask: &PdfDictionary,
        scope: &Scope<'_>,
        depth: usize,
    ) -> Option<usize> {
        let file = self.file;
        let Some(PdfValue::Reference(group)) = mask.get("G") else {
            return None;
        };
        let group = *group;
        if depth + 1 > MAX_FORM_XOBJECT_DEPTH || self.forms.contains(&group) {
            return None;
        }
        let luminosity = mask.get("S").and_then(PdfValue::as_name) != Some("Alpha");
        let space = get_stream(file, group)
            .ok()
            .and_then(|stream| stream.dict.get("Group"))
            .and_then(|group| file.resolve_dict(group).ok())
            .and_then(|group| group.get("CS"))
            .and_then(|space| ColorSpace::parse(file, space, Some(scope.resources)).ok());
        let backdrop = match (space, mask.get("BC").and_then(|bc| file.resolve(bc).ok())) {
            (Some(space), Some(PdfValue::Array(bc))) => {
                let values: Vec<f64> = bc.iter().filter_map(PdfValue::as_number).collect();
                (values.len() == space.components()).then(|| space.rgb(&values))
            }
            _ => None,
        };
        let outer = GraphicsState {
            ctm: self.state.ctm,
            pattern_base: self.state.ctm,
            text: self.state.text.clone(),
            ..GraphicsState::default()
        };
        let saved = std::mem::replace(&mut self.state, outer);
        let stack = std::mem::take(&mut self.stack);
        self.device.begin_mask();
        self.masking = true;
        let _ = self.run_form(group, None, scope, depth + 1);
        self.masking = false;
        let drawn = self.device.end_mask(luminosity, backdrop);
        self.stack = stack;
        self.state = saved;
        drawn
    }

    /// The space `cs` names: a device one, or one of the resources.
    fn color_space(&mut self, resources: &PdfDictionary, name: &str) -> PdfResult<Arc<ColorSpace>> {
        let file = self.file;
        let unreadable = || PdfError::Corrupt(format!("colour space {name}"));
        let Some(value) = resource(file, resources, "ColorSpace", name) else {
            return ColorSpace::parse(file, &PdfValue::Name(name.to_string()), None)
                .map(Arc::new)
                .map_err(|_| unreadable());
        };
        let parse = || ColorSpace::parse(file, value, None).ok().map(Arc::new);
        let space = match value {
            PdfValue::Reference(object_ref) => {
                self.spaces.entry(*object_ref).or_insert_with(parse).clone()
            }
            _ => parse(),
        };
        space.ok_or_else(unreadable)
    }

    fn shading(&mut self, value: &PdfValue) -> Option<Rc<Shading>> {
        let file = self.file;
        let parse = || Shading::parse(file, value).ok().map(Rc::new);
        match value {
            PdfValue::Reference(object_ref) => self
                .shadings
                .entry(*object_ref)
                .or_insert_with(parse)
                .clone(),
            _ => parse(),
        }
    }

    fn pattern(&mut self, resources: &PdfDictionary, name: &str) -> Option<Rc<Pattern>> {
        let file = self.file;
        let value = resource(file, resources, "Pattern", name)?;
        if let PdfValue::Reference(object_ref) = value
            && let Some(pattern) = self.patterns.get(object_ref)
        {
            return pattern.clone();
        }
        let pattern = Pattern::parse(file, value, |shading| {
            self.shading(shading)
                .ok_or_else(|| PdfError::Corrupt("unreadable shading".into()))
        })
        .ok()
        .map(Rc::new);
        if let PdfValue::Reference(object_ref) = value {
            self.patterns.insert(*object_ref, pattern.clone());
        }
        pattern
    }

    fn show_text(
        &mut self,
        operation_index: usize,
        show_operand: ShowOperand,
        string: &PdfString,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        self.device.begin_show(operation_index, show_operand);
        if D::DRAWS {
            self.prepare_tile(true, scope, depth);
            self.prepare_tile(false, scope, depth);
        }
        let font = self
            .state
            .text
            .font
            .as_ref()
            .and_then(|name| scope.fonts.get(name))
            .unwrap_or_else(|| Rc::new(LoadedFont::fallback()));
        let font = &*font;
        let scaling = self.state.text.horizontal_scaling / 100.0;
        for decoded in decode_font_glyphs(font, &string.0) {
            let text_state = &self.state.text;
            let spacing = text_state.character_spacing
                + if decoded.word_space {
                    text_state.word_spacing
                } else {
                    0.0
                };
            let size = text_state.font_size;
            let rise = text_state.text_rise;
            let text_to_page = text_state
                .text_matrix
                .multiply(self.state.ctm)
                .multiply(self.page_transform);
            // Heuristic ascent and descent, 80% and 12% of the em, so that a
            // box does not reach into the next line.
            let (local_rect, step, origin) = match decoded.vertical {
                Some(vertical) => {
                    let width = decoded.width_units / 1000.0 * size;
                    let origin_x = vertical.origin_x.map_or(width / 2.0, |x| x / 1000.0 * size);
                    let origin_y = vertical.origin_y / 1000.0 * size;
                    let advance = vertical.advance / 1000.0 * size + spacing;
                    let rect = Rect {
                        x: -origin_x,
                        y: -origin_y + rise - size * 0.12,
                        width,
                        height: size * 0.8,
                    };
                    let origin = Matrix {
                        a: size,
                        b: 0.0,
                        c: 0.0,
                        d: size,
                        e: -origin_x,
                        f: -origin_y + rise,
                    };
                    (rect, Matrix::translate(0.0, advance), origin)
                }
                None => {
                    let glyph_width = decoded.width_units / 1000.0 * size * scaling;
                    let advance = glyph_width + spacing * scaling;
                    let rect = Rect {
                        x: 0.0,
                        y: rise - size * 0.12,
                        width: glyph_width,
                        height: size * 0.8,
                    };
                    let origin = Matrix {
                        a: size * scaling,
                        b: 0.0,
                        c: 0.0,
                        d: size,
                        e: 0.0,
                        f: rise,
                    };
                    (rect, Matrix::translate(advance, 0.0), origin)
                }
            };
            let glyph = ShownGlyph {
                font,
                decoded: &decoded,
                text_to_page,
                local_rect,
                em_to_page: origin.multiply(text_to_page),
            };
            let drawn = match D::DRAWS {
                true => font.drawn(&decoded),
                false => Drawn::Text,
            };
            let mode = self.state.text.text_render_mode;
            if D::DRAWS && (4..=7).contains(&mode) {
                self.clip_glyph(&glyph, &drawn);
            }
            match drawn {
                Drawn::Procedure(type3, procedure) => {
                    if !matches!(mode, 3 | 7) {
                        let glyph_to_user = origin
                            .multiply(self.state.text.text_matrix)
                            .multiply(self.state.ctm);
                        self.run_type3(type3, procedure, glyph_to_user, scope, depth)?;
                    }
                }
                _ if self.hidden(None) => {}
                _ => self.device.glyph(&glyph, &self.state),
            }
            self.state.text.text_matrix = step.multiply(self.state.text.text_matrix);
        }
        self.device.end_show();
        Ok(())
    }

    /// Adds a glyph shown in a clipping mode to the clip its text object
    /// leaves: its outline, or else its box.
    fn clip_glyph(&mut self, glyph: &ShownGlyph<'_>, drawn: &Drawn<'_>) {
        let outline = match drawn {
            Drawn::Outline(program, gid) => program.outline(*gid),
            Drawn::Nothing => Some(Vec::new()),
            Drawn::Text | Drawn::Procedure(..) => None,
        };
        let point = |m: Matrix, x: f64, y: f64| {
            let p = m.transform_point(crate::graphics::Point::new(x, y));
            [p.x, p.y]
        };
        match outline {
            Some(segments) => {
                let m = Matrix::scale(1.0 / UNITS, 1.0 / UNITS).multiply(glyph.em_to_page);
                self.text_clip
                    .extend(segments.into_iter().map(|segment| match segment {
                        lj_draw::Segment::Move([x, y]) => lj_draw::Segment::Move(point(m, x, y)),
                        lj_draw::Segment::Line([x, y]) => lj_draw::Segment::Line(point(m, x, y)),
                        lj_draw::Segment::Cubic([ax, ay], [bx, by], [x, y]) => {
                            lj_draw::Segment::Cubic(
                                point(m, ax, ay),
                                point(m, bx, by),
                                point(m, x, y),
                            )
                        }
                        lj_draw::Segment::Close => lj_draw::Segment::Close,
                    }));
            }
            None => {
                let [a, b, c, d] = glyph
                    .local_rect
                    .to_quad()
                    .transform(glyph.text_to_page)
                    .points;
                self.text_clip.extend([
                    lj_draw::Segment::Move([a.x, a.y]),
                    lj_draw::Segment::Line([b.x, b.y]),
                    lj_draw::Segment::Line([c.x, c.y]),
                    lj_draw::Segment::Line([d.x, d.y]),
                    lj_draw::Segment::Close,
                ]);
            }
        }
    }

    /// Runs a Type3 glyph procedure as a form, its glyph space mapped to the
    /// user space by `glyph_to_user`.
    fn run_type3(
        &mut self,
        font: &Type3,
        procedure: ObjectRef,
        glyph_to_user: Matrix,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        if depth + 1 > MAX_FORM_XOBJECT_DEPTH || !self.forms.insert(procedure) {
            return Ok(());
        }
        let result = self.run_procedure(font, procedure, glyph_to_user, scope, depth);
        self.forms.remove(&procedure);
        result
    }

    fn run_procedure(
        &mut self,
        font: &Type3,
        procedure: ObjectRef,
        glyph_to_user: Matrix,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        let stream = get_stream(self.file, procedure)?;
        let operations = match self.procedures.get(&procedure) {
            Some(operations) => operations.clone(),
            None => {
                let parsed: Rc<[Operation]> = parse_content_stream(&decode_stream(stream)?)?
                    .operations
                    .into();
                self.procedures.insert(procedure, parsed.clone());
                parsed
            }
        };
        let saved = self.state.clone();
        let stack = self.stack.len();
        let uncoloured = self.uncoloured;
        self.uncoloured = operations.first().is_some_and(|op| op.operator == "d1");
        self.state.ctm = font.matrix.multiply(glyph_to_user);
        self.state.pattern_base = self.state.ctm;
        self.state.text = TextState::default();
        self.device.begin_form(procedure, None, &self.state);
        let procedure_scope = Scope {
            resources: font.resources.as_ref().unwrap_or(scope.resources),
            fonts: scope.fonts,
        };
        let result = self.run_operations(&operations, &procedure_scope, depth + 1);
        self.device.end_form();
        self.stack.truncate(stack);
        self.uncoloured = uncoloured;
        self.state = saved;
        result
    }

    /// Draws the cell of the tiling pattern the fill or the stroke paints
    /// with, once for the device.
    fn prepare_tile(&mut self, fill: bool, scope: &Scope<'_>, depth: usize) {
        let color = match fill {
            true => &self.state.fill,
            false => &self.state.stroke,
        };
        let Some(tile) = color.tile() else {
            return;
        };
        let Some(Pattern::Tiling { bbox, .. }) = color.pattern.as_deref() else {
            return;
        };
        let bbox = *bbox;
        let under = color.clone();
        if !self.device.wants_tile(tile)
            || depth + 1 > MAX_FORM_XOBJECT_DEPTH
            || self.forms.contains(&tile.source)
        {
            return;
        }
        let Ok(stream) = get_stream(self.file, tile.source) else {
            return;
        };
        let file = self.file;
        let resources: PdfDictionary = stream
            .dict
            .get("Resources")
            .and_then(|value| file.resolve_dict(value).ok().cloned())
            .unwrap_or_else(|| scope.resources.clone());
        let Ok(fonts) = Fonts::new(file, &resources, D::DRAWS, None) else {
            return;
        };
        let Ok(operations) = decode_stream(stream).and_then(|data| parse_content_stream(&data))
        else {
            return;
        };
        self.forms.insert(tile.source);
        let saved = std::mem::take(&mut self.state);
        let stack = std::mem::take(&mut self.stack);
        let page_transform = std::mem::replace(&mut self.page_transform, Matrix::identity());
        let uncoloured = self.uncoloured;
        if tile.under.is_some() {
            let color = match &*under.space {
                ColorSpace::Pattern(Some(space)) => {
                    let mut color = Color::new(Arc::new((**space).clone()));
                    color.set(&under.components);
                    color
                }
                _ => Color::black(),
            };
            self.state.fill = color.clone();
            self.state.stroke = color;
            self.uncoloured = true;
        }
        let mut cell = PathBuilder::default();
        cell.rect(bbox[0], bbox[1], bbox[2] - bbox[0], bbox[3] - bbox[1]);
        self.state.clips.push(Rc::new(ClipPath {
            segments: cell.take(),
            transform: Matrix::identity(),
            even_odd: false,
        }));
        self.device.begin_tile();
        let cell_scope = Scope {
            resources: &resources,
            fonts: &fonts,
        };
        let _ = self.run_operations(&operations.operations, &cell_scope, depth + 1);
        self.device.end_tile(tile);
        self.uncoloured = uncoloured;
        self.page_transform = page_transform;
        self.stack = stack;
        self.state = saved;
        self.forms.remove(&tile.source);
    }

    fn enter_form_xobject(&mut self, name: &str, scope: &Scope<'_>, depth: usize) -> PdfResult<()> {
        if depth + 1 > MAX_FORM_XOBJECT_DEPTH {
            return Ok(());
        }
        let Some(xobject_ref) = lookup_xobject_ref(self.file, scope.resources, name) else {
            return Ok(());
        };
        self.run_form(xobject_ref, None, scope, depth)
    }

    /// Runs a form's content, placed by its Matrix under the current CTM, or,
    /// for an annotation's appearance, fitted from its BBox onto `annotation`.
    fn run_form(
        &mut self,
        xobject_ref: ObjectRef,
        annotation: Option<Rect>,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        if !self.forms.insert(xobject_ref) {
            return Ok(());
        }
        let file = self.file;
        let result = get_stream(file, xobject_ref)
            .and_then(|stream| self.run_form_stream(xobject_ref, stream, annotation, scope, depth));
        self.forms.remove(&xobject_ref);
        result
    }

    fn run_form_stream(
        &mut self,
        source: ObjectRef,
        stream: &PdfStream,
        annotation: Option<Rect>,
        scope: &Scope<'_>,
        depth: usize,
    ) -> PdfResult<()> {
        if !matches!(
            stream.dict.get("Subtype").and_then(PdfValue::as_name),
            Some("Form") | None
        ) || self.hidden(stream.dict.get("OC"))
        {
            return Ok(());
        }
        let file = self.file;

        let form_matrix = stream
            .dict
            .get("Matrix")
            .and_then(PdfValue::as_array)
            .map(matrix_from_pdf_values)
            .transpose()?
            .unwrap_or_else(Matrix::identity);

        let form_resources: PdfDictionary = stream
            .dict
            .get("Resources")
            .map(|value| file.resolve_dict(value).cloned())
            .transpose()?
            .unwrap_or_else(|| scope.resources.clone());

        // A name the form's own resources lack falls back to the caller's.
        let fonts = Fonts::new(file, &form_resources, D::DRAWS, Some(scope.fonts))?;

        let decoded = decode_stream(stream)?;
        let form_operations = parse_content_stream(&decoded)?.operations;

        // Form invocation is bracketed by an implicit q/Q.
        let saved = self.state.clone();
        let bbox = stream
            .dict
            .get("BBox")
            .and_then(|v| crate::objects::document::parse_rect(v).ok());
        self.state.ctm = match annotation {
            None => form_matrix.multiply(saved.ctm),
            Some(target) => {
                let drawn = form_matrix.transform_rect(bbox.unwrap_or(target).normalize());
                let scale = |to: f64, from: f64| {
                    if from.abs() > f64::EPSILON {
                        to / from
                    } else {
                        1.0
                    }
                };
                let fit = Matrix::translate(-drawn.x, -drawn.y)
                    .multiply(Matrix::scale(
                        scale(target.width, drawn.width),
                        scale(target.height, drawn.height),
                    ))
                    .multiply(Matrix::translate(target.x, target.y));
                self.state.text = TextState::default();
                form_matrix.multiply(fit)
            }
        };
        self.state.pattern_base = self.state.ctm;
        // A group inside an isolated knockout group meets a transparent
        // backdrop, as an isolated one does.
        let group = transparency_group(file, &stream.dict).map(|group| TransparencyGroup {
            isolated: group.isolated || self.knockout,
            ..group
        });
        self.device.begin_form(source, group, &self.state);
        // A transparency group takes the alpha, blend and soft mask as a
        // whole: what it holds starts from none.
        let knockout = self.knockout;
        if let Some(group) = group {
            self.state.fill_alpha = 1.0;
            self.state.stroke_alpha = 1.0;
            self.state.blend = None;
            self.state.soft_mask = None;
            // In an isolated knockout group each object meets a transparent
            // backdrop, where a blend mode paints as the normal one.
            self.knockout = group.isolated && group.knockout;
        }
        if D::DRAWS
            && let Some(bbox) = bbox
        {
            let mut path = PathBuilder::default();
            path.rect(bbox.x, bbox.y, bbox.width, bbox.height);
            self.state.clips.push(Rc::new(ClipPath {
                segments: path.take(),
                transform: self.state.ctm.multiply(self.page_transform),
                even_odd: false,
            }));
        }

        let form_scope = Scope {
            resources: &form_resources,
            fonts: &fonts,
        };
        let result = self.run_operations(&form_operations, &form_scope, depth + 1);
        self.knockout = knockout;
        result?;

        self.device.end_form();
        self.state = saved;
        Ok(())
    }
}

impl<D: Device> Walk<'_, D> {
    /// The budget stays spent once crossed, so a soft mask or a tile that
    /// swallows the error still fails the page.
    fn within_budget(&self) -> PdfResult<()> {
        match self.nested_operations > MAX_NESTED_OPERATIONS {
            true => Err(PdfError::Unsupported(format!(
                "more than {MAX_NESTED_OPERATIONS} operations inside the forms of one page"
            ))),
            false => Ok(()),
        }
    }

    /// Whether a drawing leaves this out: inside hidden marked content, or
    /// under an `/OC` entry, `oc`, that a viewer hides.
    fn hidden(&self, oc: Option<&PdfValue>) -> bool {
        D::DRAWS && (self.marked.contains(&true) || oc.is_some_and(|oc| self.hides(Some(oc))))
    }

    /// An optional content group, or a membership dictionary over several.
    fn hides(&self, oc: Option<&PdfValue>) -> bool {
        let Some(oc) = oc else {
            return false;
        };
        if let PdfValue::Reference(group) = oc
            && self.hidden_groups.contains(group)
        {
            return true;
        }
        let Ok(dict) = self.file.resolve_dict(oc) else {
            return false;
        };
        if dict.get("Type").and_then(PdfValue::as_name) != Some("OCMD") {
            return false;
        }
        let groups: Vec<bool> = match dict.get("OCGs") {
            Some(PdfValue::Reference(group)) => {
                match self.file.resolve(&PdfValue::Reference(*group)) {
                    Ok(PdfValue::Array(items)) => items
                        .iter()
                        .filter_map(|item| match item {
                            PdfValue::Reference(r) => Some(self.hidden_groups.contains(r)),
                            _ => None,
                        })
                        .collect(),
                    _ => vec![self.hidden_groups.contains(group)],
                }
            }
            Some(PdfValue::Array(items)) => items
                .iter()
                .filter_map(|item| match item {
                    PdfValue::Reference(r) => Some(self.hidden_groups.contains(r)),
                    _ => None,
                })
                .collect(),
            _ => return false,
        };
        if groups.is_empty() {
            return false;
        }
        match dict.get("P").and_then(PdfValue::as_name).unwrap_or("AnyOn") {
            "AllOn" => groups.iter().any(|&off| off),
            "AnyOff" => groups.iter().all(|&off| !off),
            "AllOff" => groups.iter().any(|&off| !off),
            _ => groups.iter().all(|&off| off),
        }
    }
}

/// The property list a `BDC` names, or carries inline.
fn marked_properties<'r>(
    file: &'r PdfFile,
    resources: &'r PdfDictionary,
    operation: &'r Operation,
) -> Option<&'r PdfValue> {
    match operation.operands.get(1)? {
        PdfValue::Name(name) => resources
            .get("Properties")
            .and_then(|all| file.resolve_dict(all).ok())
            .and_then(|all| all.get(name.as_str())),
        other => Some(other),
    }
}

fn cap(value: f64) -> Cap {
    match value as i64 {
        1 => Cap::Round,
        2 => Cap::Square,
        _ => Cap::Butt,
    }
}

fn join(value: f64) -> Join {
    match value as i64 {
        1 => Join::Round,
        2 => Join::Bevel,
        _ => Join::Miter,
    }
}

fn blend(name: &str) -> Option<Blend> {
    Some(match name {
        "Multiply" => Blend::Multiply,
        "Screen" => Blend::Screen,
        "Overlay" => Blend::Overlay,
        "Darken" => Blend::Darken,
        "Lighten" => Blend::Lighten,
        "ColorDodge" => Blend::ColorDodge,
        "ColorBurn" => Blend::ColorBurn,
        "HardLight" => Blend::HardLight,
        "SoftLight" => Blend::SoftLight,
        "Difference" => Blend::Difference,
        "Exclusion" => Blend::Exclusion,
        "Hue" => Blend::Hue,
        "Saturation" => Blend::Saturation,
        "Color" => Blend::Color,
        "Luminosity" => Blend::Luminosity,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TransparencyGroup {
    /// What the group holds composites over nothing, not over the page.
    pub isolated: bool,
    /// Each object composites over the group's backdrop, not over the
    /// objects before it.
    pub knockout: bool,
}

fn transparency_group(file: &PdfFile, dict: &PdfDictionary) -> Option<TransparencyGroup> {
    let group = dict
        .get("Group")
        .and_then(|group| file.resolve_dict(group).ok())?;
    (group.get("S").and_then(PdfValue::as_name) == Some("Transparency")).then(|| {
        TransparencyGroup {
            isolated: group.get("I").and_then(PdfValue::as_bool) == Some(true),
            knockout: group.get("K").and_then(PdfValue::as_bool) == Some(true),
        }
    })
}

/// The numbers among an operation's operands.
fn numbers(operation: &Operation) -> Vec<f64> {
    operation
        .operands
        .iter()
        .filter_map(PdfValue::as_number)
        .collect()
}

/// The entry `name` of the resource category `category`.
fn resource<'a>(
    file: &'a PdfFile,
    resources: &'a PdfDictionary,
    category: &str,
    name: &str,
) -> Option<&'a PdfValue> {
    resources
        .get(category)
        .and_then(|all| file.resolve_dict(all).ok())
        .and_then(|all| all.get(name))
}
