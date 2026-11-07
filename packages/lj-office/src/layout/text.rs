//! One paragraph set in lines: each character given its face, shaped with its
//! neighbours, broken where UAX 14 allows, tabbed, aligned and measured.

use std::cell::Cell;
use std::sync::Arc;

use crate::Anchor;
use unicode_linebreak::linebreaks;

use super::float::{self, Band};
use super::metrics::{font_box, gridded, spaced, twips, LineBox};
use super::{Env, Fields, Glyphs, Item};
use crate::fonts::{self, FaceId, Resolved};
use crate::model::*;

const EPSILON: f64 = 1e-6;

/// Font, size, weight and slant a run gives a character, by the class of
/// character Word picks the font for.
pub(crate) fn slot(c: char, props: &RunProps) -> (&Arc<str>, f64, bool, bool) {
    let fonts = &props.fonts;
    if props.complex || props.rtl || complex_script(c) {
        (
            &fonts.complex,
            props.size_complex,
            props.bold_complex,
            props.italic_complex,
        )
    } else if east_asian(c) || (props.east_asian_hint && weak(c)) {
        (&fonts.east_asia, props.size, props.bold, props.italic)
    } else {
        // LibreOffice reads no hAnsi font: every western character takes the ascii one.
        (&fonts.ascii, props.size, props.bold, props.italic)
    }
}

fn complex_script(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// The punctuation and symbols of no script LibreOffice gives a run's
/// script hint to.
fn weak(c: char) -> bool {
    matches!(u32::from(c), 0x2000..=0x2BFF)
}

/// An East Asian character a full em wide in every font that draws it.
fn wide(c: char) -> bool {
    east_asian(c) && !matches!(u32::from(c), 0xFF61..=0xFFDC | 0xFFE8..=0xFFEE)
}

fn east_asian(c: char) -> bool {
    matches!(u32::from(c), 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Glyph,
    /// A space, which may hang past the right edge and stretches on a
    /// justified line.
    Space,
    Tab,
    Break,
    /// A soft hyphen: nothing, unless the line breaks after it.
    Hyphen,
    /// A drawing set in the line like a character.
    Object,
}

struct Atom<'d> {
    kind: Kind,
    c: char,
    anchor: Option<Anchor>,
    props: &'d RunProps,
    face: Resolved,
    /// The face the run resolved to, whose metrics size the line even where
    /// the character is drawn from a fallback face, as LibreOffice sizes it.
    line_face: FaceId,
    size: f64,
    /// Baseline raise, negative to lower.
    rise: f64,
    advance: f64,
    leader: Leader,
    object: Option<&'d Drawing>,
}

/// A line set: items with x from the left edge of the column and y from the
/// baseline, and the box the line takes.
pub(crate) struct Line {
    pub items: Vec<Item>,
    pub bounds: LineBox,
    /// Space above the text, part of `bounds`, taken to pass below an
    /// object the line cannot run beside.
    pub skip: f64,
    /// Space a proportional spacing adds below the text of a line after the
    /// paragraph's first, which may hang past the bottom of the column.
    pub hang: f64,
    /// The notes referenced on the line.
    pub notes: Vec<Arc<Note>>,
    /// Where the next line starts, for [`set_rest`].
    pub end: usize,
}

struct Span {
    start: usize,
    end: usize,
    /// Where the line runs beside the bands, from the column's left edge.
    left: f64,
    right: f64,
    skip: f64,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            left: f64::NEG_INFINITY,
            right: f64::INFINITY,
            skip: 0.0,
        }
    }
}

struct Setter<'e, 'a, 'd> {
    env: &'e Env<'a>,
    paragraph: &'d Paragraph,
    width: f64,
    atoms: Vec<Atom<'d>>,
    /// A line may end before the atom at this index.
    allowed: Vec<bool>,
    /// Atoms of the list label and its suffix, which lead the first line.
    label: usize,
    /// How far the label reaches left of the first line's indent.
    label_shift: f64,
    fields: Fields,
    /// The objects the lines run beside.
    bands: &'e [Band],
    /// Where the line being set runs, when bands narrow it.
    run: Cell<Option<(f64, f64)>>,
    /// The pitch of the grid the lines snap to.
    grid: Option<f64>,
    /// Each note referenced, after the atom at this index.
    notes: Vec<(usize, &'d Arc<Note>)>,
}

/// Set `inlines`, all or part of `paragraph`, in a column `width` wide.
/// `label` draws the list label before the first line.
pub(crate) fn set(
    env: &Env<'_>,
    paragraph: &Paragraph,
    inlines: &[Inline],
    label: bool,
    width: f64,
    fields: Fields,
    grid: Option<f64>,
) -> Vec<Line> {
    set_around(env, paragraph, inlines, label, width, fields, grid, &[])
}

/// [`set`], each line running beside the `bands` it meets.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_around(
    env: &Env<'_>,
    paragraph: &Paragraph,
    inlines: &[Inline],
    label: bool,
    width: f64,
    fields: Fields,
    grid: Option<f64>,
    bands: &[Band],
) -> Vec<Line> {
    let mut setter = Setter {
        env,
        paragraph,
        width,
        atoms: Vec::new(),
        allowed: Vec::new(),
        label: 0,
        label_shift: 0.0,
        fields,
        bands,
        run: Cell::new(None),
        grid,
        notes: Vec::new(),
    };
    setter.collect(inlines, label, fields);
    setter.shape();
    setter.align_label();
    setter.opportunities();
    let spans = setter.fill(0, true);
    let count = spans.len();
    spans
        .iter()
        .enumerate()
        .map(|(i, span)| setter.finish(span, i == 0, i + 1 == count))
        .collect()
}

/// The lines of [`set`] from the one starting at `from` on, set again in a
/// column `width` wide: the rest of a paragraph a narrower or wider column
/// takes over.
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_rest(
    env: &Env<'_>,
    paragraph: &Paragraph,
    inlines: &[Inline],
    label: bool,
    width: f64,
    fields: Fields,
    grid: Option<f64>,
    from: usize,
) -> Vec<Line> {
    let mut setter = Setter {
        env,
        paragraph,
        width,
        atoms: Vec::new(),
        allowed: Vec::new(),
        label: 0,
        label_shift: 0.0,
        fields,
        bands: &[],
        run: Cell::new(None),
        grid,
        notes: Vec::new(),
    };
    setter.collect(inlines, label, fields);
    setter.shape();
    setter.align_label();
    setter.opportunities();
    let spans = setter.fill(from, false);
    let count = spans.len();
    spans
        .iter()
        .enumerate()
        .map(|(i, span)| setter.finish(span, false, i + 1 == count))
        .collect()
}

impl<'e, 'a, 'd> Setter<'e, 'a, 'd> {
    fn push(&mut self, c: char, anchor: Option<Anchor>, props: &'d RunProps) {
        let kind = match c {
            '\t' => Kind::Tab,
            '\n' => Kind::Break,
            ' ' | '\u{3000}' => Kind::Space,
            '\u{AD}' => Kind::Hyphen,
            _ => Kind::Glyph,
        };
        let mut shown = if props.caps || props.small_caps {
            c.to_uppercase().next().unwrap_or(c)
        } else {
            c
        };
        let (family, mut size, bold, italic) = slot(shown, props);
        let mut face = match fonts::recode(family, shown) {
            Some(mapped) => {
                shown = mapped;
                self.env
                    .resolve(&Arc::from(fonts::SYMBOL_FACE), bold, italic)
            }
            None => self.env.resolve(family, bold, italic),
        };
        let line_face = face.face;
        if kind == Kind::Glyph && !self.env.covers(face.face, shown) {
            if let Some(found) = self.env.fonts.fallback(shown, bold, italic) {
                face = found;
            }
        }
        if props.small_caps && !props.caps && c.is_lowercase() {
            size *= 0.8;
        }
        let mut rise = props.position;
        if props.vertical != VerticalAlign::Baseline {
            let metrics = &self.env.fonts.face(line_face).metrics;
            let em = f64::from(metrics.units_per_em);
            let small = size * 0.58;
            rise += match props.vertical {
                VerticalAlign::Superscript => f64::from(metrics.ascent) / em * (size - small),
                _ => -f64::from(metrics.descent) / em * (size - small),
            };
            size = small;
        }
        self.atoms.push(Atom {
            kind,
            c: if kind == Kind::Glyph { shown } else { c },
            anchor,
            props,
            face,
            line_face,
            size,
            rise,
            advance: 0.0,
            leader: Leader::None,
            object: None,
        });
    }

    fn push_object(&mut self, drawing: &'d Drawing) {
        let props = &drawing.props;
        let (family, size, bold, italic) = slot(' ', props);
        let face = self.env.resolve(family, bold, italic);
        self.atoms.push(Atom {
            kind: Kind::Object,
            c: '\u{FFFC}',
            anchor: None,
            props,
            face,
            line_face: face.face,
            size,
            rise: props.position,
            advance: drawing.width,
            leader: Leader::None,
            object: Some(drawing),
        });
    }

    fn collect(&mut self, inlines: &'d [Inline], label: bool, fields: Fields) {
        if let (true, Some(l)) = (label, &self.paragraph.label) {
            for c in l.text.chars() {
                self.push(c, None, &l.props);
            }
            if l.suffix == LabelSuffix::Space {
                self.push(' ', None, &l.props);
            }
            for atom in &mut self.atoms {
                if atom.kind == Kind::Space {
                    atom.kind = Kind::Glyph;
                }
            }
            if l.suffix == LabelSuffix::Tab {
                self.push('\t', None, &l.props);
            }
            self.label = self.atoms.len();
        }
        for inline in inlines {
            match inline {
                Inline::Text(text) if !text.props.hidden => {
                    for (c, anchor) in text.text.chars().zip(&text.anchors) {
                        let c = if c == '\t' || c == '\n' { ' ' } else { c };
                        self.push(c, Some(*anchor), &text.props);
                    }
                }
                Inline::Char(c, props) if !props.hidden => self.push(*c, None, props),
                Inline::Tab(props) if !props.hidden => self.push('\t', None, props),
                Inline::Break(Break::Line) => {
                    let props = self.atoms.last().map_or(&self.paragraph.mark, |a| a.props);
                    self.push('\n', None, props);
                }
                Inline::Field(field, props) if !props.hidden => {
                    for c in fields.text(*field).chars() {
                        self.push(c, None, props);
                    }
                }
                Inline::Drawing(d) if d.float.is_none() && !d.props.hidden => self.push_object(d),
                Inline::Note(note, props) if !props.hidden => {
                    for c in note.label.chars() {
                        self.push(c, None, props);
                    }
                    self.notes.push((self.atoms.len(), note));
                }
                _ => {}
            }
        }
    }

    /// Advances of every character, shaped by runs of one face, size and
    /// formatting, as LibreOffice shapes a text portion.
    fn shape(&mut self) {
        let mut start = 0;
        while start < self.atoms.len() {
            let shaped = |a: &Atom<'_>| matches!(a.kind, Kind::Glyph | Kind::Space);
            if !shaped(&self.atoms[start]) {
                start += 1;
                continue;
            }
            let first = &self.atoms[start];
            let mut end = start + 1;
            while end < self.atoms.len() {
                let a = &self.atoms[end];
                if !shaped(a)
                    || a.face.face != first.face.face
                    || a.size != first.size
                    || !(std::ptr::eq(a.props, first.props) || a.props == first.props)
                {
                    break;
                }
                end += 1;
            }
            let text: String = self.atoms[start..end].iter().map(|a| a.c).collect();
            let props = first.props;
            let face = self.env.fonts.face(first.face.face);
            let scale = first.size / f64::from(face.metrics.units_per_em) * props.scale;
            let advances = face.advances(&text, props.kerning);
            let em = f64::from(face.metrics.units_per_em);
            for (atom, advance) in self.atoms[start..end].iter_mut().zip(advances) {
                // LibreOffice sets an East Asian space 0.5 em wide in any font.
                let advance = if atom.c == ' ' && props.east_asian_hint {
                    em / 2.0
                } else if wide(atom.c) && !self.env.covers(atom.face.face, atom.c) {
                    em
                } else {
                    advance
                };
                atom.advance = advance * scale + props.spacing;
            }
            start = end;
        }
    }

    /// A label aligned right ends at the first line's indent, one centred
    /// straddles it.
    fn align_label(&mut self) {
        let Some(label) = self.paragraph.label.as_ref().filter(|_| self.label > 0) else {
            return;
        };
        let width: f64 = self.atoms[..self.label]
            .iter()
            .filter(|a| a.kind != Kind::Tab)
            .map(|a| a.advance)
            .sum();
        self.label_shift = match label.alignment {
            Alignment::Right => width,
            Alignment::Center => width / 2.0,
            _ => 0.0,
        };
    }

    fn opportunities(&mut self) {
        let text: String = self.atoms.iter().map(|a| a.c).collect();
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        self.allowed = vec![false; self.atoms.len() + 1];
        for (offset, _) in linebreaks(&text) {
            let index = starts.partition_point(|s| *s < offset);
            self.allowed[index] = index >= self.label;
        }
    }

    fn left(&self, first: bool) -> f64 {
        let indent = &self.paragraph.props.indent;
        let left = match first {
            true => indent.left + indent.first_line - self.label_shift,
            false => indent.left,
        };
        self.run.get().map_or(left, |(from, _)| left.max(from))
    }

    fn right(&self) -> f64 {
        let right = self.width - self.paragraph.props.indent.right;
        self.run.get().map_or(right, |(_, to)| right.min(to))
    }

    /// Where the line from `top` runs past the bands, and how far down it
    /// goes to find room.
    fn band(&self, top: f64, height: f64, first: bool) -> (Option<(f64, f64)>, f64) {
        if self.bands.is_empty() {
            return (None, 0.0);
        }
        self.run.set(None);
        let (left, right) = (self.left(first), self.right());
        let mut at = top;
        for _ in 0..8 + self.bands.len() {
            match float::beside(self.bands, at, height, left, right) {
                Ok(run) => return (Some(run), at - top),
                Err(below) => at = below.max(at + EPSILON),
            }
        }
        (Some((left, right)), at - top)
    }

    /// Width of what follows the tab at `i` up to the next tab or break,
    /// trailing spaces left out, and of its part before a decimal mark.
    fn segment(&self, i: usize) -> (f64, f64) {
        let mut width = 0.0;
        let mut trailing = 0.0;
        let mut decimal = None;
        for a in &self.atoms[i + 1..] {
            if matches!(a.kind, Kind::Tab | Kind::Break) {
                break;
            }
            if decimal.is_none() && (a.c == '.' || a.c == ',') {
                decimal = Some(width + trailing);
            }
            if a.kind == Kind::Space {
                trailing += a.advance;
            } else {
                width += trailing + a.advance;
                trailing = 0.0;
            }
        }
        (width, decimal.unwrap_or(width))
    }

    /// Advance of the tab at `i` from `x`, and the leader it draws, as
    /// LibreOffice's `NewTabPortion` finds its stop: the next set stop, the
    /// list's among them, else a default one; the hanging indent first when
    /// the tab starts left of it, unless the stop found is the list's.
    fn tab(&self, i: usize, x: f64) -> (f64, Leader) {
        let props = &self.paragraph.props;
        let list = self.paragraph.label.as_ref().and_then(|l| l.tab_stop);
        let found = props
            .tabs
            .iter()
            .filter(|t| t.kind != TabKind::Bar)
            .map(|t| (t.position, t.kind, t.leader))
            .chain(list.map(|p| (p, TabKind::Left, Leader::None)))
            .filter(|(position, ..)| *position > x + EPSILON)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let hanging = props.indent.left;
        let inside = x + EPSILON < hanging
            && !found.is_some_and(|(p, ..)| list.is_some_and(|l| (p - l).abs() < EPSILON));
        let (stop, kind, leader) = match found {
            Some((position, ..)) if inside && position > hanging + EPSILON => {
                (hanging, TabKind::Left, Leader::None)
            }
            Some((position, kind, leader)) if kind != TabKind::Left && position > self.right() => {
                (self.right(), TabKind::Right, leader)
            }
            Some(stop) => stop,
            None if inside => (hanging, TabKind::Left, Leader::None),
            None => {
                let step = self.env.settings.default_tab_stop;
                let mut next = ((x + EPSILON) / step).floor() * step + step;
                if next <= x + EPSILON {
                    next += step;
                }
                (next, TabKind::Left, Leader::None)
            }
        };
        let (width, before_decimal) = self.segment(i);
        let target = match kind {
            TabKind::Right => stop - width,
            TabKind::Center => stop - width / 2.0,
            TabKind::Decimal => stop - before_decimal,
            TabKind::Left | TabKind::Bar => stop,
        };
        ((target - x).max(0.0), leader)
    }

    /// Cut the atoms in lines, greedily, at the last opportunity that fits.
    fn fill(&mut self, from: usize, first_line: bool) -> Vec<Span> {
        let n = self.atoms.len();
        let mut spans: Vec<Span> = Vec::new();
        let mut start = from;
        let mut top = 0.0;
        let mut height = self.line_box(&Span::new(from, from)).height;
        loop {
            let first = first_line && spans.is_empty();
            let (run, skip) = self.band(top, height, first);
            self.run.set(run);
            let right = self.right();
            let mut x = self.left(first);
            let mut end = n;
            let mut i = start;
            while i < n {
                match self.atoms[i].kind {
                    Kind::Break => {
                        end = i + 1;
                        break;
                    }
                    Kind::Tab => {
                        let (mut advance, leader) = self.tab(i, x);
                        let trailing = self.env.settings.tab_over_margin
                            && self.atoms[i + 1..]
                                .iter()
                                .take_while(|a| a.kind != Kind::Break)
                                .all(|a| matches!(a.kind, Kind::Tab | Kind::Space));
                        if trailing && x + advance > right {
                            advance = (right - x).max(0.0);
                        }
                        self.atoms[i].advance = advance;
                        self.atoms[i].leader = leader;
                    }
                    _ => {}
                }
                let atom = &self.atoms[i];
                if i > start && x + atom.advance > right + EPSILON {
                    // LibreOffice breaks at a space that overflows whatever
                    // follows it, and asks the break rules only inside words.
                    let blank = atom.kind == Kind::Space;
                    let at = if blank {
                        let next = (i..n).find(|&k| self.atoms[k].kind != Kind::Space);
                        Some(match next {
                            Some(k) if self.atoms[k].kind == Kind::Break => k + 1,
                            Some(k) => k,
                            None => n,
                        })
                    } else {
                        self.boundary(start, i)
                    };
                    end = at.unwrap_or(i);
                    if at.is_some_and(|e| e < n) {
                        end = self.squeeze(start, end, i, first).unwrap_or(end);
                    }
                    break;
                }
                x += atom.advance;
                i += 1;
            }
            let mut span = Span::new(start, end);
            if let Some((left, right)) = run {
                (span.left, span.right, span.skip) = (left, right, skip);
                height = self.line_box(&span).height;
                top += skip + height;
            }
            spans.push(span);
            self.run.set(None);
            let broken = end > start && self.atoms[end - 1].kind == Kind::Break;
            if end >= n {
                if broken {
                    spans.push(Span::new(n, n));
                }
                return spans;
            }
            start = end;
        }
    }

    /// Where a line from `start` whose atom `cut` does not fit breaks, as
    /// LibreOffice's break iterator answers: it keeps a path whole by
    /// breaking at the space before a slash rather than after the slash.
    fn boundary(&self, start: usize, cut: usize) -> Option<usize> {
        let slash = |b: usize| self.atoms[b - 1].c == '/';
        if self.allowed[cut] && !slash(cut) {
            return Some(cut);
        }
        let found = (start + 1..cut).rev().find(|&b| self.allowed[b])?;
        if !slash(found) {
            return Some(found);
        }
        let blank = |c: char| c.is_whitespace() && !matches!(c, '\u{A0}' | '\u{2007}' | '\u{202F}');
        let mut pos = found - 1;
        while pos > 0 && found - pos < 66 {
            pos -= 1;
            if blank(self.atoms[pos].c) {
                return (pos + 1 > start).then_some(pos + 1);
            }
        }
        Some(found)
    }

    fn same_portion(&self, a: usize, b: usize) -> bool {
        let (a, b) = (&self.atoms[a], &self.atoms[b]);
        matches!(a.kind, Kind::Glyph | Kind::Space)
            && a.face.face == b.face.face
            && a.size == b.size
            && (std::ptr::eq(a.props, b.props) || a.props == b.props)
    }

    /// Where a line broken inside the portion `from..stop`, which starts at
    /// `x`, ends when its right edge is `limit`: `stop` when all of it fits.
    fn reach(&self, from: usize, stop: usize, mut x: f64, limit: f64) -> Option<usize> {
        for i in from..stop {
            let atom = &self.atoms[i];
            if i > from && x + atom.advance > limit + EPSILON {
                return if atom.kind == Kind::Space {
                    (i..stop).find(|&k| self.atoms[k].kind != Kind::Space)
                } else {
                    self.boundary(from, i)
                };
            }
            x += atom.advance;
        }
        Some(stop)
    }

    /// The end a justified line overflowing at `over` takes instead of `end`
    /// when its spaces may shrink to three quarters, as LibreOffice decides
    /// for a Word 2013 file: more words when shrinking the spaces to fit them
    /// spoils the line less than stretching them without.
    fn squeeze(&self, start: usize, end: usize, over: usize, first: bool) -> Option<usize> {
        let atoms = &self.atoms;
        if !self.env.settings.justify_shrinks
            || !matches!(
                self.paragraph.props.alignment,
                Alignment::Justify | Alignment::Distribute
            )
            || atoms[start..over].iter().any(|a| a.kind == Kind::Tab)
        {
            return None;
        }
        let mut from = over;
        while from > start && self.same_portion(from - 1, over) {
            from -= 1;
        }
        if end <= from {
            return None;
        }
        let mut stop = over;
        while stop < atoms.len() && self.same_portion(stop, over) {
            stop += 1;
        }
        let x = self.left(first) + atoms[start..from].iter().map(|a| a.advance).sum::<f64>();
        // LibreOffice decides in twips, widths truncated.
        let room = ((self.right() - x) * 20.0).round();
        let content = |e: usize| {
            let mut c = e;
            while c > start && atoms[c - 1].kind == Kind::Space {
                c -= 1;
            }
            c
        };
        let width = |e: usize| {
            let points: f64 = atoms[from..content(e).max(from)]
                .iter()
                .map(|a| a.advance)
                .sum();
            (points * 20.0).floor()
        };
        let spaces = |e: usize| {
            let line = &atoms[start..content(e)];
            let lead = line.iter().take_while(|a| a.kind == Kind::Space).count();
            line[lead..]
                .iter()
                .filter(|a| a.kind == Kind::Space)
                .count() as f64
        };
        let a = &atoms[over];
        let face = self.env.fonts.face(a.face.face);
        let unit = a.size / f64::from(face.metrics.units_per_em) * a.props.scale;
        let space =
            ((face.advances(" ", false)[0] * unit + a.props.spacing) * 200.0).round() / 10.0;
        let leading = self.env.settings.external_leading;
        let height = (font_box(&face.metrics, a.size, leading).height * 20.0).round();
        let widened = |count: f64, spacing: f64| {
            let limit = room + (count * space * (1.0 - spacing / 100.0)).trunc();
            let reached = self.reach(from, stop, x, x + limit / 20.0)?;
            // The rest of a paragraph that fits squeezed is taken only when it
            // is long enough to skip LibreOffice's quick fit test.
            let taken = reached < stop
                || (stop == atoms.len() && 2.0 * limit <= (stop - from) as f64 * height);
            taken.then_some(reached)
        };

        let real = spaces(end);
        if real == 0.0 {
            return None;
        }
        let normal = (room - width(end) + real * space) / real;
        let hyphen = atoms[content(end) - 1].kind == Kind::Hyphen;
        let mut best = widened(real + if hyphen { 0.0 } else { 1.0 }, 75.0)?;
        if width(best) <= width(end) {
            return None;
        }
        let last = if best == atoms.len() {
            best - 1
        } else {
            content(best)
        };
        if let Some(blank) = (end..last).find(|&k| atoms[k].kind == Kind::Space) {
            let span = (last - end) as f64;
            let count = spaces(best);
            let weighted =
                |taken: f64| (75.0 * taken / span + 100.0 * (span - taken) / span).trunc();
            let mut taken = (blank - end) as f64;
            let mut third = widened(count - 1.0, weighted(taken));
            if third.is_some_and(|t| spaces(t) == count) {
                taken -= 1.0;
                third = widened(count - 1.0, weighted(taken));
            }
            if let Some(t) = third.filter(|&t| width(t) > width(end)) {
                best = t;
            }
        }
        let count = spaces(best);
        let taken = width(best);
        if taken <= room {
            return None;
        }
        let shrunk = (room - taken + count * space) / count;
        let stretched = 1.0 + (normal / space - 1.0) / 1.7;
        (stretched >= space / shrunk).then_some(best)
    }

    /// The box of the line a span makes, its line spacing applied.
    fn line_box(&self, span: &Span) -> LineBox {
        let rule = self.paragraph.props.spacing.line;
        let (text, blanks, has_text) = self.natural_box(span);
        let objects = self.atoms[span.start..span.end]
            .iter()
            .filter_map(|a| {
                a.object.map(|d| {
                    float::object_box(float::height(self.env, d, d.width, self.fields), a.rise)
                })
            })
            .reduce(LineBox::union);
        let grid = self.grid.filter(|_| !self.paragraph.props.off_grid);
        let spacing = |natural, rule| match grid {
            Some(pitch) => gridded(natural, rule, pitch),
            None => spaced(natural, rule),
        };
        match objects {
            None => {
                let mut line = spacing(text, rule);
                if let (LineSpacing::Proportional(percent), None) = (rule, grid) {
                    if percent > 100.0 && blanks.height > text.height {
                        let extra = twips(blanks.height) * (percent / 100.0 - 1.0);
                        line.height = (twips(text.height) + extra).floor() / 20.0;
                    }
                }
                line
            }
            Some(objects) => float::line_box(text, has_text, objects, rule, spacing),
        }
    }

    /// The box of a span's text without its blanks and tabs, which do not
    /// raise a Word line in LibreOffice, the paragraph mark's when it has no
    /// other; the box with them but the spaces a wrap leaves at its end,
    /// whose height still counts in a proportional spacing; and whether it
    /// has any but blanks, which a line holding an object ignores.
    fn natural_box(&self, span: &Span) -> (LineBox, LineBox, bool) {
        let atoms = &self.atoms[span.start..span.end];
        let wrapped = span.end < self.atoms.len();
        let kept = atoms.len()
            - atoms
                .iter()
                .rev()
                .take_while(|a| wrapped && a.kind == Kind::Space)
                .count();
        let mut bounds: Option<LineBox> = None;
        let mut blanks: Option<LineBox> = None;
        let mut found = false;
        for (k, atom) in atoms.iter().enumerate() {
            if atom.kind == Kind::Object {
                continue;
            }
            found |= !matches!(
                atom.kind,
                Kind::Space | Kind::Tab | Kind::Break | Kind::Hyphen
            );
            let face = self.env.fonts.face(atom.line_face);
            let mut b = font_box(&face.metrics, atom.size, self.env.settings.external_leading);
            if atom.rise != 0.0 {
                b = LineBox {
                    ascent: b.ascent + atom.rise.max(0.0),
                    height: b.height + atom.rise.abs(),
                };
            }
            if k < kept {
                blanks = Some(blanks.map_or(b, |u| u.union(b)));
            }
            if !matches!(atom.kind, Kind::Space | Kind::Tab) {
                bounds = Some(bounds.map_or(b, |u| u.union(b)));
            }
        }
        let bounds = bounds.unwrap_or_else(|| {
            let mark = &self.paragraph.mark;
            let (family, size, bold, italic) = slot(' ', mark);
            let face = self.env.resolve(family, bold, italic);
            font_box(
                &self.env.fonts.face(face.face).metrics,
                size,
                self.env.settings.external_leading,
            )
        });
        let blanks = blanks.map_or(bounds, |b| b.union(bounds));
        (bounds, blanks, found)
    }

    fn finish(&self, span: &Span, first: bool, last: bool) -> Line {
        self.run.set(Some((span.left, span.right)));
        let atoms = &self.atoms[span.start..span.end];
        let mut content = atoms.len();
        while content > 0 && matches!(atoms[content - 1].kind, Kind::Space | Kind::Break) {
            content -= 1;
        }
        let hyphenated = !last && content > 0 && atoms[content - 1].kind == Kind::Hyphen;
        let hyphen = if hyphenated {
            let a = &atoms[content - 1];
            let face = self.env.fonts.face(a.face.face);
            face.advances("-", false)[0] * a.size / f64::from(face.metrics.units_per_em)
        } else {
            0.0
        };
        let natural: f64 = atoms[..content]
            .iter()
            .filter(|a| a.kind != Kind::Hyphen)
            .map(|a| a.advance)
            .sum::<f64>()
            + hyphen;
        let left = self.left(first);
        let room = self.right() - left - natural;
        let broken = atoms.last().is_some_and(|a| a.kind == Kind::Break);
        let (shift, stretch) = match self.paragraph.props.alignment {
            Alignment::Left => (0.0, 0.0),
            Alignment::Center => ((room / 2.0).max(0.0), 0.0),
            Alignment::Right => (room.max(0.0), 0.0),
            Alignment::Justify if !last || broken => (0.0, room),
            Alignment::Justify => (0.0, room.min(0.0)),
            Alignment::Distribute => (0.0, room),
        };
        let from = atoms[..content]
            .iter()
            .rposition(|a| a.kind == Kind::Tab)
            .map_or(0, |t| t + 1);
        let spaces = atoms[from..content]
            .iter()
            .filter(|a| a.kind == Kind::Space)
            .count();
        let extra = if spaces > 0 {
            stretch / spaces as f64
        } else {
            0.0
        };

        let mut items = Vec::new();
        let mut decorations = Vec::new();
        let mut run: Option<Glyphs> = None;
        let mut x = left + shift;
        for (k, atom) in atoms.iter().enumerate() {
            let mut advance = atom.advance;
            if atom.kind == Kind::Space && k >= from && k < content {
                advance += extra;
            }
            let (c, advance) = match atom.kind {
                Kind::Hyphen if hyphenated && k + 1 == content => ('-', hyphen),
                Kind::Hyphen => continue,
                _ => (atom.c, advance),
            };
            match atom.kind {
                Kind::Tab => {
                    close(&mut run, &mut items);
                    self.leader(atom, x, advance, &mut items);
                }
                Kind::Break => close(&mut run, &mut items),
                Kind::Object => {
                    close(&mut run, &mut items);
                    if let Some(d) = atom.object {
                        let h = float::height(self.env, d, d.width, self.fields);
                        let y = -atom.rise - h;
                        let (w, t) = (d.width, d.transform);
                        float::draw(self.env, &d.shape, x, y, w, h, t, self.fields, &mut items);
                    }
                }
                _ => {
                    let color = self.ink(atom.props);
                    let joins = run.as_ref().is_some_and(|g| {
                        g.face == atom.face.face
                            && g.size == atom.size
                            && g.y == -atom.rise
                            && g.color == color
                            && g.synthetic_bold == atom.face.synthetic_bold
                            && g.synthetic_italic == atom.face.synthetic_italic
                            && g.kerning == atom.props.kerning
                            && g.letter_spacing == atom.props.spacing
                    });
                    if !joins {
                        close(&mut run, &mut items);
                        run = Some(Glyphs {
                            x,
                            y: -atom.rise,
                            face: atom.face.face,
                            size: atom.size,
                            color,
                            text: String::new(),
                            advances: Vec::new(),
                            anchors: Vec::new(),
                            synthetic_bold: atom.face.synthetic_bold,
                            synthetic_italic: atom.face.synthetic_italic,
                            kerning: atom.props.kerning,
                            letter_spacing: atom.props.spacing,
                        });
                    }
                    let glyphs = run.as_mut().expect("a run was opened");
                    glyphs.text.push(c);
                    glyphs.advances.push(advance);
                    glyphs.anchors.push(atom.anchor);
                }
            }
            if !matches!(atom.kind, Kind::Break | Kind::Object) {
                decorations.push((atom, x, advance, k < content));
            }
            x += advance;
        }
        close(&mut run, &mut items);
        let mut backgrounds = Vec::new();
        self.decorate(&decorations, &mut backgrounds, &mut items);
        backgrounds.extend(items);

        let mut bounds = self.line_box(span);
        let grid = self.grid.filter(|_| !self.paragraph.props.off_grid);
        let hang = match (self.paragraph.props.spacing.line, grid) {
            (LineSpacing::Proportional(percent), None)
                if !first
                    && percent > 100.0
                    && self.atoms[span.start..span.end]
                        .iter()
                        .all(|a| a.object.is_none()) =>
            {
                (bounds.height - self.natural_box(span).0.height).max(0.0)
            }
            _ => 0.0,
        };
        bounds.ascent += span.skip;
        bounds.height += span.skip;
        self.run.set(None);
        let notes = self
            .notes
            .iter()
            .filter(|(at, _)| {
                let owner = at.saturating_sub(1);
                owner >= span.start && (owner < span.end || last)
            })
            .map(|(_, note)| Arc::clone(note))
            .collect();
        Line {
            items: backgrounds,
            bounds,
            skip: span.skip,
            hang,
            notes,
            end: span.end,
        }
    }

    /// The colour of a run's text, an automatic one white over a dark fill,
    /// which a highlight is not.
    fn ink(&self, props: &RunProps) -> Color {
        props.color.unwrap_or_else(|| {
            let under = (props.shading)
                .or(self.paragraph.props.shading)
                .or(self.env.backdrop.get());
            if under.is_some_and(Color::is_dark) {
                Color::WHITE
            } else {
                Color::BLACK
            }
        })
    }

    fn leader(&self, atom: &Atom<'_>, x: f64, advance: f64, items: &mut Vec<Item>) {
        let c = match atom.leader {
            Leader::None => return,
            Leader::Dot => '.',
            Leader::Hyphen => '-',
            Leader::Underscore => '_',
            Leader::MiddleDot => '\u{B7}',
        };
        let face = self.env.fonts.face(atom.face.face);
        let unit = face.advances(&c.to_string(), false)[0] * atom.size
            / f64::from(face.metrics.units_per_em);
        if unit <= 0.0 {
            return;
        }
        // LibreOffice fills from the tab's start, one more underscore to close the gap.
        let count = (advance / unit).floor() as usize + usize::from(c == '_');
        if count == 0 {
            return;
        }
        items.push(Item::Glyphs(Glyphs {
            x,
            y: -atom.rise,
            face: atom.face.face,
            size: atom.size,
            color: self.ink(atom.props),
            text: std::iter::repeat_n(c, count).collect(),
            advances: vec![unit; count],
            anchors: vec![None; count],
            synthetic_bold: atom.face.synthetic_bold,
            synthetic_italic: atom.face.synthetic_italic,
            kerning: false,
            letter_spacing: 0.0,
        }));
    }

    /// Underlines, strike-throughs, highlights and run shading, each drawn
    /// once over the atoms that share it.
    fn decorate(
        &self,
        atoms: &[(&Atom<'_>, f64, f64, bool)],
        backgrounds: &mut Vec<Item>,
        items: &mut Vec<Item>,
    ) {
        let mut k = 0;
        while k < atoms.len() {
            let (atom, x0, _, _) = atoms[k];
            let props = atom.props;
            let mut end = k + 1;
            while end < atoms.len()
                && atoms[end].0.props == props
                && atoms[end].0.face.face == atom.face.face
                && atoms[end].0.size == atom.size
            {
                end += 1;
            }
            let x1 = atoms[end - 1].1 + atoms[end - 1].2;
            let face = self.env.fonts.face(atom.face.face);
            let em = f64::from(face.metrics.units_per_em);
            let per = atom.size / em;
            let color = self.ink(props);
            let top = -atom.rise - f64::from(face.metrics.ascent) * per;
            let height = f64::from(face.metrics.ascent + face.metrics.descent) * per;
            if let Some(fill) = props.highlight.or(props.shading) {
                backgrounds.push(Item::Rect {
                    x: x0,
                    y: top,
                    width: x1 - x0,
                    height,
                    color: fill,
                });
            }
            let content_end = atoms[k..end]
                .iter()
                .rposition(|a| a.3)
                .map(|i| atoms[k + i].1 + atoms[k + i].2);
            if let (Some(x_end), true) = (content_end, props.underline != Underline::None) {
                let thickness = f64::from(face.metrics.underline_thickness) * per;
                let y = -atom.rise - f64::from(face.metrics.underline_position) * per;
                let mut segments = Vec::new();
                if props.underline == Underline::Words {
                    let mut open: Option<f64> = None;
                    for &(a, x, w, _) in &atoms[k..end] {
                        let blank = matches!(a.kind, Kind::Space | Kind::Tab);
                        match (blank, open) {
                            (false, None) => open = Some(x),
                            (true, Some(s)) => {
                                segments.push((s, x));
                                open = None;
                            }
                            _ => {}
                        }
                        if !blank && x + w >= x_end - EPSILON {
                            if let Some(s) = open.take() {
                                segments.push((s, x + w));
                            }
                        }
                    }
                } else {
                    segments.push((x0, x_end));
                }
                let (width, offsets): (f64, &[f64]) = match props.underline {
                    Underline::Double => (thickness, &[0.0, 2.0]),
                    Underline::Thick => (thickness * 2.0, &[0.0]),
                    _ => (thickness, &[0.0]),
                };
                for (s, e) in segments {
                    for offset in offsets {
                        let y = y + offset * thickness;
                        items.push(Item::Line {
                            from: [s, y],
                            to: [e, y],
                            width,
                            color,
                        });
                    }
                }
            }
            if let (Some(x_end), true) = (content_end, props.strike != Strike::None) {
                let width = f64::from(face.metrics.strikeout_size) * per;
                let y = -atom.rise - f64::from(face.metrics.strikeout_position) * per;
                let offsets: &[f64] = if props.strike == Strike::Double {
                    &[-1.0, 1.0]
                } else {
                    &[0.0]
                };
                for offset in offsets {
                    let y = y + offset * width;
                    items.push(Item::Line {
                        from: [x0, y],
                        to: [x_end, y],
                        width,
                        color,
                    });
                }
            }
            k = end;
        }
    }
}

fn close(run: &mut Option<Glyphs>, items: &mut Vec<Item>) {
    if let Some(glyphs) = run.take() {
        items.push(Item::Glyphs(glyphs));
    }
}
