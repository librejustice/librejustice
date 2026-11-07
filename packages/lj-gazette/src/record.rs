//! A notice as a shape, typed cells and a rest: the shape is the tree without its values, `name@attr$(child%3,child&7)`;
//! a value goes to the first column naming its path if the codec gives its exact text back, else to the rest, joined by U+001F, which XML 1.0 cannot carry.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::tree::{Body, Node};
use crate::Error;

const SEP: char = '\u{1f}';
const META: &[char] = &['@', '$', '%', '&', '(', ')', ','];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Text,
    Date,
    /// Up to nine digits, an `integer`.
    Int,
    /// Up to eighteen digits, a `bigint`.
    Long,
    Siren,
    Dict(i16),
    Address,
}

pub struct Column {
    pub name: &'static str,
    pub codec: Codec,
    pub paths: &'static [&'static str],
}

pub struct Spec {
    pub columns: &'static [Column],
    index: OnceLock<Index>,
}

/// Exact paths, then patterns whose `*` segment matches any one segment; a path several columns
/// name fills them in column order, one value each.
struct Index {
    exact: HashMap<&'static str, Vec<usize>>,
    patterns: Vec<(Vec<&'static str>, usize)>,
}

fn matches(pattern: &[&str], path: &str) -> bool {
    let mut segs = path.split('/');
    pattern
        .iter()
        .all(|p| segs.next().is_some_and(|s| *p == "*" || *p == s))
        && segs.next().is_none()
}

impl Spec {
    pub const fn new(columns: &'static [Column]) -> Self {
        Self {
            columns,
            index: OnceLock::new(),
        }
    }

    fn index(&self) -> &Index {
        self.index.get_or_init(|| {
            let mut exact: HashMap<&'static str, Vec<usize>> = HashMap::new();
            let mut patterns = Vec::new();
            for (i, c) in self.columns.iter().enumerate() {
                for p in c.paths {
                    if p.split('/').any(|s| s == "*") {
                        patterns.push((p.split('/').collect(), i));
                    } else {
                        exact.entry(*p).or_default().push(i);
                    }
                }
            }
            Index { exact, patterns }
        })
    }

    fn columns_of<'s>(&'s self, path: &'s str) -> impl Iterator<Item = usize> + 's {
        let index = self.index();
        let exact = index.exact.get(path).map(Vec::as_slice).unwrap_or_default();
        exact.iter().copied().chain(
            index
                .patterns
                .iter()
                .filter(move |(p, _)| matches(p, path))
                .map(|(_, c)| *c),
        )
    }

    pub fn column(&self, name: &str) -> usize {
        self.columns
            .iter()
            .position(|c| c.name == name)
            .unwrap_or_else(|| panic!("no column {name}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Text(String),
    /// Days since 1970-01-01.
    Date(i32),
    Int(i64),
    Siren(u32),
    Dict(String),
    Address(Box<Record>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub shape: String,
    pub cells: Vec<Option<Cell>>,
    pub rest: Option<String>,
}

impl Record {
    pub fn text(&self, col: usize) -> Option<&str> {
        match self.cells[col].as_ref()? {
            Cell::Text(s) | Cell::Dict(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(&self, col: usize) -> Option<i64> {
        match self.cells[col].as_ref()? {
            Cell::Int(i) => Some(*i),
            _ => None,
        }
    }
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn parse_date(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        b[r].iter().try_fold(0u32, |acc, &c| {
            c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
        })
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    let days = days_from_civil(i64::from(y), m, d);
    (civil_from_days(days) == (i64::from(y), m, d)).then(|| i32::try_from(days).ok())?
}

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// `5 décembre 2007` or `1er mars 2008`, spaced by U+00A0 (variants 3, 4) or a space (5, 6),
/// the odd one writing the first of the month `1`.
fn parse_long_date(s: &str) -> Option<(i32, u8)> {
    [('\u{a0}', 3u8), (' ', 5u8)]
        .into_iter()
        .find_map(|(sep, base)| {
            let mut parts = s.split(sep);
            let (day, month, year) = (parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some() {
                return None;
            }
            let d: u32 = match day {
                "1er" => 1,
                _ if (1..=2).contains(&day.len())
                    && !day.starts_with('0')
                    && day.bytes().all(|c| c.is_ascii_digit()) =>
                {
                    day.parse().ok()?
                }
                _ => return None,
            };
            let m = MONTHS.iter().position(|&name| name == month)? + 1;
            parse_date(&format!("{year}-{m:02}-{d:02}"))
                .map(|days| (days, base + u8::from(day == "1")))
        })
}

/// `yyyy-mm-dd`, or the `dd/mm/yyyy`, `d/mm/yyyy`, `yyyy/mm/dd` and long French forms of older
/// files, with the variant that writes it back.
pub fn parse_any_date(s: &str) -> Option<(i32, u8)> {
    let b = s.as_bytes();
    if b.len() == 10 && b[2] == b'/' && b[5] == b'/' {
        return parse_date(&format!("{}-{}-{}", &s[6..10], &s[3..5], &s[0..2])).map(|d| (d, 1));
    }
    if b.len() == 10 && b[4] == b'/' && b[7] == b'/' {
        return parse_date(&format!("{}-{}-{}", &s[0..4], &s[5..7], &s[8..10])).map(|d| (d, 2));
    }
    if b.len() == 9 && b[1] == b'/' && b[4] == b'/' && b[0] != b'0' {
        return parse_date(&format!("{}-{}-0{}", &s[5..9], &s[2..4], &s[0..1])).map(|d| (d, 7));
    }
    parse_date(s).map(|d| (d, 0)).or_else(|| parse_long_date(s))
}

pub fn format_date(days: i32) -> String {
    let (y, m, d) = civil_from_days(i64::from(days));
    format!("{y:04}-{m:02}-{d:02}")
}

fn encode(codec: Codec, v: &str) -> Option<(Cell, u8)> {
    match codec {
        Codec::Text => Some((Cell::Text(v.to_string()), 0)),
        Codec::Dict(_) => Some((Cell::Dict(v.to_string()), 0)),
        Codec::Date => parse_any_date(v).map(|(d, variant)| (Cell::Date(d), variant)),
        Codec::Int | Codec::Long => {
            let max = if codec == Codec::Int { 9 } else { 18 };
            if v.is_empty() || v.len() > max || !v.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let padded = v.len() > 1 && v.starts_with('0');
            Some((
                Cell::Int(v.parse().ok()?),
                if padded { v.len() as u8 } else { 0 },
            ))
        }
        Codec::Siren => {
            let (digits, variant) = if v.len() == 9 {
                (v.to_string(), 1)
            } else {
                [(' ', 0), ('\u{a0}', 2)]
                    .into_iter()
                    .find_map(|(sep, variant)| {
                        let groups: Vec<&str> = v.split(sep).collect();
                        (groups.len() == 3 && groups.iter().all(|g| g.len() == 3))
                            .then(|| (groups.concat(), variant))
                    })?
            };
            if !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some((Cell::Siren(digits.parse().ok()?), variant))
        }
        Codec::Address => None,
    }
}

fn render(cell: &Cell, variant: u8) -> Result<String, Error> {
    Ok(match (cell, variant) {
        (Cell::Text(s) | Cell::Dict(s), 0) => s.clone(),
        (Cell::Date(d), 0) => format_date(*d),
        (Cell::Date(d), 1) => {
            let (y, m, dd) = civil_from_days(i64::from(*d));
            format!("{dd:02}/{m:02}/{y:04}")
        }
        (Cell::Date(d), 2) => {
            let (y, m, dd) = civil_from_days(i64::from(*d));
            format!("{y:04}/{m:02}/{dd:02}")
        }
        (Cell::Date(d), 7) => {
            let (y, m, dd) = civil_from_days(i64::from(*d));
            format!("{dd}/{m:02}/{y:04}")
        }
        (Cell::Date(d), 3..=6) => {
            let (y, m, dd) = civil_from_days(i64::from(*d));
            let sep = if variant < 5 { '\u{a0}' } else { ' ' };
            let day = if dd == 1 && variant % 2 == 1 {
                "1er".to_string()
            } else {
                dd.to_string()
            };
            format!("{day}{sep}{}{sep}{y:04}", MONTHS[m as usize - 1])
        }
        (Cell::Int(i), 0) => i.to_string(),
        (Cell::Int(i), width) => format!("{i:0width$}", width = usize::from(width)),
        (Cell::Siren(s), 0) => {
            let d = format!("{s:09}");
            format!("{} {} {}", &d[0..3], &d[3..6], &d[6..9])
        }
        (Cell::Siren(s), 1) => format!("{s:09}"),
        (Cell::Siren(s), 2) => {
            let d = format!("{s:09}");
            format!("{}\u{a0}{}\u{a0}{}", &d[0..3], &d[3..6], &d[6..9])
        }
        _ => {
            return Err(Error::new(format!(
                "cell {cell:?} has no variant {variant}"
            )))
        }
    })
}

struct Decomposer<'a> {
    spec: &'a Spec,
    named: bool,
    shape: String,
    cells: Vec<Option<Cell>>,
    rest: Vec<&'a str>,
    path: String,
}

impl<'a> Decomposer<'a> {
    fn free_column(&self) -> Option<usize> {
        self.spec
            .columns_of(&self.path)
            .find(|&c| self.cells[c].is_none())
    }

    fn slot(&mut self, value: &'a str) -> Result<(), Error> {
        if let Some(col) = self.free_column() {
            if let Some((cell, variant)) = encode(self.spec.columns[col].codec, value) {
                self.cells[col] = Some(cell);
                let _ = write!(self.shape, "%{col}");
                if variant != 0 {
                    let _ = write!(self.shape, ".{variant}");
                }
                return Ok(());
            }
        }
        if value.contains(SEP) {
            return Err(Error::new(format!("value at {} carries U+001F", self.path)));
        }
        self.shape.push('$');
        self.rest.push(value);
        Ok(())
    }

    fn element(&mut self, node: &'a Node, root: bool) -> Result<(), Error> {
        if node.name.contains(META) {
            return Err(Error::new(format!(
                "element name {} carries a shape character",
                node.name
            )));
        }
        let mark = self.path.len();
        if root && self.named {
            self.shape.push_str(&node.name);
        }
        if !root {
            if !self.path.is_empty() {
                self.path.push('/');
            }
            self.path.push_str(&node.name);
            self.shape.push_str(&node.name);
            if let Some(col) = self.free_column() {
                if self.spec.columns[col].codec == Codec::Address {
                    let sub = decompose_with(node, &crate::address::SPEC, false)?;
                    self.cells[col] = Some(Cell::Address(Box::new(sub)));
                    let _ = write!(self.shape, "&{col}");
                    self.path.truncate(mark);
                    return Ok(());
                }
            }
        }
        for (k, v) in &node.attrs {
            if k.contains(META) {
                return Err(Error::new(format!(
                    "attribute name {k} carries a shape character"
                )));
            }
            let m = self.path.len();
            if !self.path.is_empty() {
                self.path.push('/');
            }
            self.path.push('@');
            self.path.push_str(k);
            self.shape.push('@');
            self.shape.push_str(k);
            self.slot(v)?;
            self.path.truncate(m);
        }
        match &node.body {
            Body::Empty => {}
            Body::Text(t) => self.slot(t)?,
            Body::Children(children) => {
                self.shape.push('(');
                for (i, c) in children.iter().enumerate() {
                    if i > 0 {
                        self.shape.push(',');
                    }
                    self.element(c, false)?;
                }
                self.shape.push(')');
            }
        }
        self.path.truncate(mark);
        Ok(())
    }
}

pub fn decompose(node: &Node, spec: &Spec) -> Result<Record, Error> {
    decompose_with(node, spec, true)
}

/// An address leaves its element's name out of the shape, so one row serves whatever element holds it.
fn decompose_with(node: &Node, spec: &Spec, named: bool) -> Result<Record, Error> {
    let mut d = Decomposer {
        spec,
        named,
        shape: String::new(),
        cells: vec![None; spec.columns.len()],
        rest: Vec::new(),
        path: String::new(),
    };
    d.element(node, true)?;
    let rest = (!d.rest.is_empty()).then(|| d.rest.join("\u{1f}"));
    Ok(Record {
        shape: d.shape,
        cells: d.cells,
        rest,
    })
}

struct Recomposer<'a> {
    rec: &'a Record,
    shape: &'a [u8],
    pos: usize,
    rest: Option<std::str::Split<'a, char>>,
}

impl Recomposer<'_> {
    fn peek(&self) -> Option<u8> {
        self.shape.get(self.pos).copied()
    }

    fn name(&mut self) -> Result<String, Error> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if META.contains(&char::from(c)) {
                break;
            }
            self.pos += 1;
        }
        std::str::from_utf8(&self.shape[start..self.pos])
            .map(str::to_string)
            .map_err(|_| Error::new("shape name is not UTF-8"))
    }

    fn number(&mut self) -> Result<usize, Error> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.shape[start..self.pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::new(format!("shape: number expected at {start}")))
    }

    fn value(&mut self) -> Result<String, Error> {
        match self.peek() {
            Some(b'$') => {
                self.pos += 1;
                self.rest
                    .as_mut()
                    .and_then(Iterator::next)
                    .map(str::to_string)
                    .ok_or_else(|| Error::new("rest exhausted"))
            }
            Some(b'%') => {
                self.pos += 1;
                let col = self.number()?;
                let variant = if self.peek() == Some(b'.') {
                    self.pos += 1;
                    u8::try_from(self.number()?).map_err(|_| Error::new("variant out of range"))?
                } else {
                    0
                };
                let cell = self
                    .rec
                    .cells
                    .get(col)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| Error::new(format!("shape names empty column {col}")))?;
                render(cell, variant)
            }
            _ => Err(Error::new(format!("shape: slot expected at {}", self.pos))),
        }
    }

    fn element(&mut self, name: String) -> Result<Node, Error> {
        if self.peek() == Some(b'&') {
            self.pos += 1;
            let col = self.number()?;
            let Some(Cell::Address(sub)) = self.rec.cells.get(col).and_then(Option::as_ref) else {
                return Err(Error::new(format!(
                    "shape names address column {col} without an address"
                )));
            };
            let mut node = recompose(sub)?;
            node.name = name;
            return Ok(node);
        }
        let mut attrs = Vec::new();
        while self.peek() == Some(b'@') {
            self.pos += 1;
            let k = self.name()?;
            let v = self.value()?;
            attrs.push((k, v));
        }
        let body = match self.peek() {
            Some(b'$' | b'%') => Body::Text(self.value()?),
            Some(b'(') => {
                self.pos += 1;
                let mut children = Vec::new();
                loop {
                    let n = self.name()?;
                    children.push(self.element(n)?);
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b')') => {
                            self.pos += 1;
                            break;
                        }
                        _ => {
                            return Err(Error::new(format!(
                                "shape: , or ) expected at {}",
                                self.pos
                            )))
                        }
                    }
                }
                Body::Children(children)
            }
            _ => Body::Empty,
        };
        Ok(Node { name, attrs, body })
    }
}

pub fn recompose(rec: &Record) -> Result<Node, Error> {
    let mut r = Recomposer {
        rec,
        shape: rec.shape.as_bytes(),
        pos: 0,
        rest: rec.rest.as_deref().map(|s| s.split(SEP)),
    };
    let name = r.name()?;
    let node = r.element(name)?;
    if r.pos != r.shape.len() {
        return Err(Error::new(format!("shape: trailing input at {}", r.pos)));
    }
    if r.rest.as_mut().and_then(Iterator::next).is_some() {
        return Err(Error::new("rest values left over"));
    }
    Ok(node)
}

/// The path of each value a shape sends to the rest, in rest order: what a spec leaves uncolumned.
pub fn rest_paths(shape: &str) -> Result<Vec<String>, Error> {
    fn element(
        s: &[u8],
        pos: &mut usize,
        path: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> Result<(), Error> {
        let name = |pos: &mut usize| {
            let start = *pos;
            while *pos < s.len() && !META.contains(&char::from(s[*pos])) {
                *pos += 1;
            }
            String::from_utf8_lossy(&s[start..*pos]).into_owned()
        };
        let slot = |pos: &mut usize, at: String, out: &mut Vec<String>| -> Result<(), Error> {
            match s.get(*pos) {
                Some(b'$') => {
                    *pos += 1;
                    out.push(at);
                }
                Some(b'%') => {
                    *pos += 1;
                    while *pos < s.len() && (s[*pos].is_ascii_digit() || s[*pos] == b'.') {
                        *pos += 1;
                    }
                }
                _ => return Err(Error::new(format!("shape: slot expected at {pos}"))),
            }
            Ok(())
        };
        if s.get(*pos) == Some(&b'&') {
            *pos += 1;
            while *pos < s.len() && s[*pos].is_ascii_digit() {
                *pos += 1;
            }
            return Ok(());
        }
        while s.get(*pos) == Some(&b'@') {
            *pos += 1;
            let k = name(pos);
            let mut at = path.join("/");
            if !at.is_empty() {
                at.push('/');
            }
            at.push('@');
            at.push_str(&k);
            slot(pos, at, out)?;
        }
        match s.get(*pos) {
            Some(b'$' | b'%') => slot(pos, path.join("/"), out)?,
            Some(b'(') => {
                *pos += 1;
                loop {
                    let n = name(pos);
                    path.push(n);
                    element(s, pos, path, out)?;
                    path.pop();
                    match s.get(*pos) {
                        Some(b',') => *pos += 1,
                        Some(b')') => {
                            *pos += 1;
                            break;
                        }
                        _ => return Err(Error::new(format!("shape: , or ) expected at {pos}"))),
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let s = shape.as_bytes();
    let mut pos = 0;
    while pos < s.len() && !META.contains(&char::from(s[pos])) {
        pos += 1;
    }
    let mut out = Vec::new();
    element(s, &mut pos, &mut Vec::new(), &mut out)?;
    Ok(out)
}
