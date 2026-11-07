//! Records as table rows: a dictionary value or an address becomes a code, which never changes once stored.

use std::collections::HashMap;
use std::sync::LazyLock;

use xxhash_rust::xxh3::Xxh3;

use crate::record::{Cell, Codec, Record, Spec};
use crate::{address, bodacc, dict, joafe, officers, Error, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Small,
    Int,
    Long,
    Date,
    Text,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Small(i16),
    Int(i32),
    Long(i64),
    /// Days since 1970-01-01.
    Date(i32),
    Text(String),
    Bool(bool),
}

pub type Row = Vec<Value>;

pub struct Table {
    pub name: &'static str,
    pub columns: Vec<(&'static str, Type)>,
}

/// A spec column the row id gives back, and how.
type Derived = (&'static str, fn(i64) -> i64);

/// A record table: its own columns, ending with `shape`, the spec's columns but those the row id gives back, then `rest`.
pub struct Layout {
    pub table: Table,
    spec: &'static Spec,
    shape_kind: i16,
    own: usize,
    stored: Vec<usize>,
    derived: &'static [Derived],
}

fn type_of(codec: Codec) -> Type {
    match codec {
        Codec::Text => Type::Text,
        Codec::Date => Type::Date,
        Codec::Int | Codec::Siren | Codec::Address => Type::Int,
        Codec::Long => Type::Long,
        Codec::Dict(kind) if dict::wide(kind) => Type::Int,
        Codec::Dict(_) => Type::Small,
    }
}

fn layout(
    name: &'static str,
    own: &[(&'static str, Type)],
    spec: &'static Spec,
    shape_kind: i16,
    derived: &'static [Derived],
) -> Layout {
    let stored: Vec<usize> = (0..spec.columns.len())
        .filter(|&i| !derived.iter().any(|(d, _)| *d == spec.columns[i].name))
        .collect();
    let mut columns = own.to_vec();
    columns.extend(
        stored
            .iter()
            .map(|&i| (spec.columns[i].name, type_of(spec.columns[i].codec))),
    );
    columns.push(("rest", Type::Text));
    Layout {
        table: Table { name, columns },
        spec,
        shape_kind,
        own: own.len(),
        stored,
        derived,
    }
}

pub static BODACC: LazyLock<Layout> = LazyLock::new(|| {
    use Type::*;
    layout(
        "bodacc_notice",
        &[
            ("id", Long),
            ("date", Date),
            ("kind", Small),
            ("famille", Small),
            ("avis", Small),
            ("shape", Int),
        ],
        &bodacc::SPEC,
        dict::SHAPE,
        &[("num", |id| id % 1_000_000)],
    )
});

pub static JOAFE: LazyLock<Layout> = LazyLock::new(|| {
    use Type::*;
    layout(
        "joafe_notice",
        &[
            ("id", Long),
            ("date", Date),
            ("action", Small),
            ("rectificatif", Bool),
            ("shape", Int),
        ],
        &joafe::SPEC,
        dict::SHAPE,
        &[("id", |id| id), ("num", |id| id % 10_000)],
    )
});

pub static ADDRESS: LazyLock<Layout> = LazyLock::new(|| {
    use Type::*;
    layout(
        "gazette_address",
        &[("id", Int), ("key", Long), ("shape", Int)],
        &address::SPEC,
        dict::ADDRESS_SHAPE,
        &[],
    )
});

pub static OFFICER: LazyLock<Table> = LazyLock::new(|| Table {
    name: "bodacc_officer",
    columns: vec![
        ("id", Type::Long),
        ("role", Type::Int),
        ("name", Type::Text),
    ],
});

pub static DICT: LazyLock<Table> = LazyLock::new(|| Table {
    name: "gazette_dict",
    columns: vec![
        ("kind", Type::Small),
        ("code", Type::Int),
        ("value", Type::Text),
    ],
});

pub static FILE: LazyLock<Table> = LazyLock::new(|| {
    use Type::*;
    Table {
        name: "gazette_file",
        columns: vec![
            ("source", Small),
            ("key", Text),
            ("checksum", Long),
            ("notices", Int),
            ("header_shape", Int),
            ("header_rest", Text),
        ],
    }
});

impl Source {
    pub fn layout(self) -> &'static Layout {
        match self {
            Source::Bodacc => &BODACC,
            Source::Joafe => &JOAFE,
        }
    }

    /// The id and the columns a notice takes from rules over its document rather than from its record.
    pub fn classes(self) -> Table {
        use Type::*;
        Table {
            name: self.layout().table.name,
            columns: match self {
                Source::Bodacc => vec![
                    ("id", Long),
                    ("kind", Small),
                    ("famille", Small),
                    ("avis", Small),
                ],
                Source::Joafe => vec![("id", Long), ("action", Small), ("rectificatif", Bool)],
            },
        }
    }
}

/// Each notice of a container as a row of [`Source::classes`].
pub fn class_rows(parsed: &Parsed) -> Vec<Row> {
    let mut rows = Vec::new();
    for doc in &parsed.docs {
        match doc {
            Doc::Bodacc(f) => rows.extend(f.notices.iter().map(|n| {
                vec![
                    Value::Long(n.id),
                    Value::Small(f.kind as i16),
                    Value::Small(n.famille as i16),
                    Value::Small(n.avis as i16),
                ]
            })),
            Doc::Joafe(d) => rows.extend(d.notices.iter().map(|n| {
                vec![
                    Value::Long(n.id),
                    Value::Small(n.action as i16),
                    Value::Bool(n.rectificatif),
                ]
            })),
        }
    }
    rows
}

/// Stored as the address row's `key`: the bytes hashed never change.
pub fn address_key(rec: &Record) -> i64 {
    let mut h = Xxh3::new();
    h.update(rec.shape.as_bytes());
    for cell in &rec.cells {
        match cell {
            None => h.update(&[0]),
            Some(Cell::Text(s)) => {
                h.update(&[1]);
                h.update(s.as_bytes());
                h.update(&[0x1e]);
            }
            Some(other) => h.update(format!("\u{2}{other:?}\u{1e}").as_bytes()),
        }
    }
    match &rec.rest {
        None => h.update(&[0]),
        Some(r) => {
            h.update(&[1]);
            h.update(r.as_bytes());
        }
    }
    h.digest() as i64
}

#[derive(Default)]
pub struct Codes {
    codes: HashMap<i16, HashMap<String, i32>>,
    values: HashMap<i16, Vec<String>>,
    addresses: HashMap<i64, i32>,
    last_address: i32,
    fresh_dicts: Vec<Row>,
    fresh_addresses: Vec<Row>,
}

impl Codes {
    pub fn new(last_address: i32) -> Self {
        Self {
            last_address,
            ..Self::default()
        }
    }

    /// Read in code order: a kind's codes run from 1 without a gap.
    pub fn load(&mut self, kind: i16, code: i32, value: String) -> Result<(), Error> {
        let values = self.values.entry(kind).or_default();
        if usize::try_from(code).ok() != Some(values.len() + 1) {
            return Err(Error::new(format!(
                "dictionary {kind}: code {code} after {}",
                values.len()
            )));
        }
        self.codes
            .entry(kind)
            .or_default()
            .insert(value.clone(), code);
        values.push(value);
        Ok(())
    }

    pub fn code(&mut self, kind: i16, value: &str) -> Result<i32, Error> {
        if let Some(&code) = self.codes.get(&kind).and_then(|m| m.get(value)) {
            return Ok(code);
        }
        let values = self.values.entry(kind).or_default();
        let code = i32::try_from(values.len() + 1)
            .map_err(|_| Error::new(format!("dictionary {kind} is full")))?;
        if !dict::wide(kind) && code > i32::from(i16::MAX) {
            return Err(Error::new(format!(
                "dictionary {kind} outgrows a smallint at {value:?}"
            )));
        }
        values.push(value.to_string());
        self.codes
            .entry(kind)
            .or_default()
            .insert(value.to_string(), code);
        self.fresh_dicts.push(vec![
            Value::Small(kind),
            Value::Int(code),
            Value::Text(value.to_string()),
        ]);
        Ok(code)
    }

    pub fn value(&self, kind: i16, code: i32) -> Result<&str, Error> {
        usize::try_from(code - 1)
            .ok()
            .and_then(|i| self.values.get(&kind)?.get(i))
            .map(String::as_str)
            .ok_or_else(|| Error::new(format!("dictionary {kind} has no code {code}")))
    }

    pub fn dictionary_len(&self, kind: i16) -> usize {
        self.values.get(&kind).map_or(0, Vec::len)
    }

    pub fn address_count(&self) -> usize {
        self.addresses.len()
    }

    pub fn knows_address(&self, key: i64) -> bool {
        self.addresses.contains_key(&key)
    }

    pub fn learn_address(&mut self, key: i64, id: i32) {
        self.addresses.insert(key, id);
    }

    fn address(&mut self, rec: Record) -> Result<i32, Error> {
        let key = address_key(&rec);
        if let Some(&id) = self.addresses.get(&key) {
            return Ok(id);
        }
        self.last_address = self
            .last_address
            .checked_add(1)
            .ok_or_else(|| Error::new("address ids exhausted"))?;
        let id = self.last_address;
        self.addresses.insert(key, id);
        let mut row = vec![Value::Int(id), Value::Long(key)];
        self.record_values(&ADDRESS, rec, &mut row)?;
        self.fresh_addresses.push(row);
        Ok(id)
    }

    /// Appends `shape`, the stored cells and `rest`.
    fn record_values(&mut self, l: &Layout, rec: Record, row: &mut Row) -> Result<(), Error> {
        row.push(Value::Int(self.code(l.shape_kind, &rec.shape)?));
        let mut cells = rec.cells;
        for &col in &l.stored {
            let column = &l.spec.columns[col];
            row.push(match (cells[col].take(), column.codec) {
                (None, _) => Value::Null,
                (Some(Cell::Text(s)), Codec::Text) => Value::Text(s),
                (Some(Cell::Date(d)), Codec::Date) => Value::Date(d),
                (Some(Cell::Int(i)), Codec::Int) => {
                    Value::Int(i32::try_from(i).map_err(|_| {
                        Error::new(format!("{} {i} outgrows an integer", column.name))
                    })?)
                }
                (Some(Cell::Int(i)), Codec::Long) => Value::Long(i),
                (Some(Cell::Siren(s)), Codec::Siren) => Value::Int(s as i32),
                (Some(Cell::Dict(s)), Codec::Dict(kind)) => {
                    let code = self.code(kind, &s)?;
                    if dict::wide(kind) {
                        Value::Int(code)
                    } else {
                        Value::Small(code as i16)
                    }
                }
                (Some(Cell::Address(a)), Codec::Address) => Value::Int(self.address(*a)?),
                (Some(cell), codec) => {
                    return Err(Error::new(format!(
                        "{} holds {cell:?}, not {codec:?}",
                        column.name
                    )))
                }
            });
        }
        row.push(rec.rest.map_or(Value::Null, Value::Text));
        Ok(())
    }
}

fn check_derived(l: &Layout, rec: &Record, id: i64) -> Result<(), Error> {
    for (name, f) in l.derived {
        match &rec.cells[l.spec.column(name)] {
            None => {}
            Some(Cell::Int(v)) if *v == f(id) => {}
            Some(other) => {
                return Err(Error::new(format!(
                    "notice {id}: {name} {other:?} is not {}",
                    f(id)
                )))
            }
        }
    }
    Ok(())
}

/// One parution file, or for the 1997-2003 JOAFE a batch of one-notice files.
pub struct Parsed {
    pub origin: String,
    pub checksum: i64,
    pub docs: Vec<Doc>,
}

pub enum Doc {
    Bodacc(bodacc::File),
    Joafe(joafe::Doc),
}

impl Doc {
    pub fn notices(&self) -> usize {
        match self {
            Doc::Bodacc(f) => f.notices.len(),
            Doc::Joafe(d) => d.notices.len(),
        }
    }

    pub fn records(&self) -> Vec<(i64, &Record)> {
        match self {
            Doc::Bodacc(f) => f.notices.iter().map(|n| (n.id, &n.record)).collect(),
            Doc::Joafe(d) => d.notices.iter().map(|n| (n.id, &n.record)).collect(),
        }
    }

    pub fn header(&self) -> Option<&Record> {
        match self {
            Doc::Bodacc(f) => Some(&f.header),
            Doc::Joafe(d) => d.header.as_ref(),
        }
    }
}

pub fn address_keys(parsed: &Parsed) -> Vec<i64> {
    parsed
        .docs
        .iter()
        .flat_map(Doc::records)
        .flat_map(|(_, r)| r.cells.iter())
        .filter_map(|c| match c {
            Some(Cell::Address(a)) => Some(address_key(a)),
            _ => None,
        })
        .collect()
}

pub fn checksum(docs: &[Vec<u8>]) -> i64 {
    let mut h = Xxh3::new();
    for d in docs {
        h.update(&(d.len() as u64).to_le_bytes());
        h.update(d);
    }
    h.digest() as i64
}

pub fn parse_doc(source: Source, raw: &[u8]) -> Result<Doc, Error> {
    let root = crate::tree::parse(&crate::tree::decode(raw)?)?;
    Ok(match source {
        Source::Bodacc => Doc::Bodacc(bodacc::parse_file(root)?),
        Source::Joafe => Doc::Joafe(joafe::parse(root)?),
    })
}

pub fn parse_container(source: Source, origin: String, docs: &[Vec<u8>]) -> Result<Parsed, Error> {
    let parsed = docs
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            parse_doc(source, raw).map_err(|e| Error::new(format!("{origin} document {i}: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Parsed {
        checksum: checksum(docs),
        origin,
        docs: parsed,
    })
}

/// The rows of a batch of containers; a notice met twice keeps its last version.
#[derive(Default)]
pub struct Rows {
    pub files: Vec<Row>,
    pub notices: Vec<Row>,
    pub officers: Vec<Row>,
    pub ids: Vec<i64>,
    pub dicts: Vec<Row>,
    pub addresses: Vec<Row>,
    pub repeated: usize,
    seen: HashMap<i64, usize>,
    officers_of: Vec<Vec<Row>>,
}

impl Rows {
    fn push(&mut self, id: i64, row: Row, officers: Vec<Row>) {
        match self.seen.get(&id) {
            Some(&i) => {
                self.repeated += 1;
                self.notices[i] = row;
                self.officers_of[i] = officers;
            }
            None => {
                self.seen.insert(id, self.notices.len());
                self.ids.push(id);
                self.notices.push(row);
                self.officers_of.push(officers);
            }
        }
    }

    /// Takes the dictionary entries and addresses first met in the batch, which its rows name.
    pub fn finish(&mut self, codes: &mut Codes) {
        self.officers = self.officers_of.drain(..).flatten().collect();
        self.dicts = std::mem::take(&mut codes.fresh_dicts);
        self.addresses = std::mem::take(&mut codes.fresh_addresses);
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn officer_rows(codes: &mut Codes, id: i64, administration: &str) -> Result<Vec<Row>, Error> {
    officers::parse(administration)
        .into_iter()
        .take(100)
        .enumerate()
        .map(|(i, o)| {
            Ok(vec![
                Value::Long(id * 100 + i as i64),
                Value::Int(codes.code(dict::ROLE, &o.role)?),
                Value::Text(o.name),
            ])
        })
        .collect()
}

impl Codes {
    pub fn container(
        &mut self,
        source: Source,
        parsed: Parsed,
        rows: &mut Rows,
    ) -> Result<(), Error> {
        let notices: usize = parsed.docs.iter().map(Doc::notices).sum();
        let mut headers = Vec::new();
        for doc in parsed.docs {
            match doc {
                Doc::Bodacc(f) => {
                    headers.push(f.header);
                    let admin = BODACC.spec.column("administration");
                    for n in f.notices {
                        check_derived(&BODACC, &n.record, n.id)?;
                        let officers = match n.record.text(admin) {
                            Some(a) => officer_rows(self, n.id, a)?,
                            None => Vec::new(),
                        };
                        let mut row = Vec::with_capacity(BODACC.table.columns.len());
                        row.extend([Value::Long(n.id), Value::Date(f.date)]);
                        row.extend(
                            [f.kind as i16, n.famille as i16, n.avis as i16].map(Value::Small),
                        );
                        self.record_values(&BODACC, n.record, &mut row)?;
                        rows.push(n.id, row, officers);
                    }
                }
                Doc::Joafe(d) => {
                    headers.extend(d.header);
                    for n in d.notices {
                        check_derived(&JOAFE, &n.record, n.id)?;
                        let mut row = Vec::with_capacity(JOAFE.table.columns.len());
                        row.extend([
                            Value::Long(n.id),
                            Value::Date(d.date),
                            Value::Small(n.action as i16),
                            Value::Bool(n.rectificatif),
                        ]);
                        self.record_values(&JOAFE, n.record, &mut row)?;
                        rows.push(n.id, row, Vec::new());
                    }
                }
            }
        }
        if headers.len() > 1 {
            return Err(Error::new(format!(
                "{}: {} headers in one container",
                parsed.origin,
                headers.len()
            )));
        }
        let (shape, rest) = match headers.pop() {
            Some(h) => (
                Value::Int(self.code(dict::HEADER_SHAPE, &h.shape)?),
                h.rest.map_or(Value::Null, Value::Text),
            ),
            None => (Value::Null, Value::Null),
        };
        rows.files.push(vec![
            Value::Small(source as i16),
            Value::Text(parsed.origin),
            Value::Long(parsed.checksum),
            Value::Int(i32::try_from(notices).map_err(|_| Error::new("notice count"))?),
            shape,
            rest,
        ]);
        Ok(())
    }
}

fn cell(
    codes: &Codes,
    codec: Codec,
    v: &Value,
    addresses: &HashMap<i32, Record>,
) -> Result<Option<Cell>, Error> {
    Ok(Some(match (v, codec) {
        (Value::Null, _) => return Ok(None),
        (Value::Text(s), Codec::Text) => Cell::Text(s.clone()),
        (Value::Date(d), Codec::Date) => Cell::Date(*d),
        (Value::Int(i), Codec::Int) => Cell::Int(i64::from(*i)),
        (Value::Long(i), Codec::Long) => Cell::Int(*i),
        (Value::Int(s), Codec::Siren) => Cell::Siren(*s as u32),
        (Value::Small(c), Codec::Dict(kind)) => {
            Cell::Dict(codes.value(kind, i32::from(*c))?.to_string())
        }
        (Value::Int(c), Codec::Dict(kind)) => Cell::Dict(codes.value(kind, *c)?.to_string()),
        (Value::Int(a), Codec::Address) => Cell::Address(Box::new(
            addresses
                .get(a)
                .cloned()
                .ok_or_else(|| Error::new(format!("address {a} not loaded")))?,
        )),
        (v, codec) => return Err(Error::new(format!("stored {v:?} is not {codec:?}"))),
    }))
}

impl Layout {
    pub fn address_ids(&self, row: &Row) -> Vec<i32> {
        self.stored
            .iter()
            .enumerate()
            .filter(|(_, &col)| self.spec.columns[col].codec == Codec::Address)
            .filter_map(|(k, _)| match row[self.own + k] {
                Value::Int(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// The row's id and the record it gives back.
    pub fn record(
        &self,
        row: &Row,
        codes: &Codes,
        addresses: &HashMap<i32, Record>,
    ) -> Result<(i64, Record), Error> {
        let id = match row.first() {
            Some(Value::Long(id)) => *id,
            Some(Value::Int(id)) => i64::from(*id),
            other => return Err(Error::new(format!("row id {other:?}"))),
        };
        let Value::Int(shape) = row[self.own - 1] else {
            return Err(Error::new(format!(
                "row {id}: shape {:?}",
                row[self.own - 1]
            )));
        };
        let mut cells = vec![None; self.spec.columns.len()];
        for (k, &col) in self.stored.iter().enumerate() {
            cells[col] = cell(
                codes,
                self.spec.columns[col].codec,
                &row[self.own + k],
                addresses,
            )?;
        }
        for (name, f) in self.derived {
            cells[self.spec.column(name)] = Some(Cell::Int(f(id)));
        }
        let rest = match row.last() {
            Some(Value::Text(r)) => Some(r.clone()),
            Some(Value::Null) => None,
            other => return Err(Error::new(format!("row {id}: rest {other:?}"))),
        };
        Ok((
            id,
            Record {
                shape: codes.value(self.shape_kind, shape)?.to_string(),
                cells,
                rest,
            },
        ))
    }
}
