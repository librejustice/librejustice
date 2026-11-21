//! What a container's records give back against its source elements, and what the specs leave in the rest.

use std::collections::HashMap;

use crate::record::{recompose, rest_paths, Record};
use crate::rows::Parsed;
use crate::tree::Node;
use crate::{bodacc, joafe, Error, Source};

/// The header element, `None` for a one-notice JOAFE file, and the notice elements in document order.
pub fn originals(source: Source, raw: &[u8]) -> Result<(Option<Node>, Vec<Node>), Error> {
    let root = crate::rows::root(raw)?;
    Ok(match source {
        Source::Bodacc => {
            let (header, items) = bodacc::split(root)?;
            (Some(header), items)
        }
        Source::Joafe => {
            let unitary = root.name == "ANNONCE_REF";
            let (header, items) = joafe::split(root)?;
            (if unitary { None } else { Some(header) }, items)
        }
    })
}

#[derive(Default)]
pub struct Comparison {
    pub notices: usize,
    pub missing: Vec<i64>,
    pub differing: Vec<i64>,
    pub headers_differing: Vec<String>,
}

impl Comparison {
    pub fn add(&mut self, other: Comparison) {
        self.notices += other.notices;
        self.missing.extend(other.missing);
        self.differing.extend(other.differing);
        self.headers_differing.extend(other.headers_differing);
    }
}

pub fn compare(
    source: Source,
    raw: &[Vec<u8>],
    parsed: &Parsed,
    stored: &dyn Fn(i64) -> Option<Record>,
    header: Option<&Record>,
) -> Result<Comparison, Error> {
    let mut out = Comparison::default();
    for (raw, doc) in raw.iter().zip(&parsed.docs) {
        let (head, items) = originals(source, raw)?;
        let records = doc.records();
        if records.len() != items.len() {
            return Err(Error::new(format!(
                "{}: {} records for {} elements",
                parsed.origin,
                records.len(),
                items.len()
            )));
        }
        for ((id, _), node) in records.iter().zip(&items) {
            out.notices += 1;
            match stored(*id) {
                None => out.missing.push(*id),
                Some(rec) if recompose(&rec).ok().as_ref() != Some(node) => out.differing.push(*id),
                Some(_) => {}
            }
        }
        if let Some(head) = head {
            if header.and_then(|h| recompose(h).ok()) != Some(head) {
                out.headers_differing.push(parsed.origin.clone());
            }
        }
    }
    Ok(out)
}

#[derive(Default)]
pub struct Inspection {
    pub comparison: Comparison,
    /// Path to (values, bytes, a few values).
    pub rest: HashMap<String, (u64, u64, Vec<String>)>,
}

/// The codec's own round trip, without any store, and the rest by path.
pub fn inspect(source: Source, raw: &[Vec<u8>], parsed: &Parsed) -> Result<Inspection, Error> {
    let mut records: HashMap<i64, &Record> = HashMap::new();
    let mut rest: HashMap<String, (u64, u64, Vec<String>)> = HashMap::new();
    for doc in &parsed.docs {
        for (id, rec) in doc.records() {
            records.insert(id, rec);
            if let Some(values) = &rec.rest {
                for (path, v) in rest_paths(&rec.shape)?
                    .into_iter()
                    .zip(values.split('\u{1f}'))
                {
                    let e = rest.entry(path).or_default();
                    e.0 += 1;
                    e.1 += v.len() as u64;
                    if e.2.len() < 3 {
                        e.2.push(v.chars().take(60).collect());
                    }
                }
            }
        }
    }
    let header = parsed.docs.iter().find_map(|d| d.header());
    let comparison = compare(
        source,
        raw,
        parsed,
        &|id| records.get(&id).map(|r| (*r).clone()),
        header,
    )?;
    Ok(Inspection { comparison, rest })
}
