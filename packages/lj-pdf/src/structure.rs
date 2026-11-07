use std::collections::{BTreeMap, BTreeSet};

use crate::objects::{ObjectRef, PdfDictionary, PdfFile, PdfValue, StructureOwners};

const MAX_DEPTH: usize = 256;

/// The keys of a structure element that carry text standing for its content.
pub(crate) const MARKED_TEXT_KEYS: [&str; 3] = ["ActualText", "Alt", "E"];

/// For each marked-content identifier of a page's own content, the structure
/// elements that hold it, innermost first.
pub(crate) fn page_owners(file: &PdfFile, page: ObjectRef) -> BTreeMap<i64, Vec<ObjectRef>> {
    file.structure_owners
        .get_or_init(|| owners(file))
        .get(&page)
        .cloned()
        .unwrap_or_default()
}

/// One walk of the structure tree, for every page at once: walked per page, a
/// long tagged document re-reads the same tree for each of its pages.
fn owners(file: &PdfFile) -> StructureOwners {
    let mut owners = StructureOwners::new();
    let Some(root) = file
        .trailer
        .get("Root")
        .and_then(|root| file.resolve_dict(root).ok())
        .and_then(|catalog| catalog.get("StructTreeRoot"))
        .and_then(|tree| file.resolve_dict(tree).ok())
    else {
        return owners;
    };
    let mut walk = Walk {
        file,
        owners: &mut owners,
        visited: BTreeSet::new(),
        chain: Vec::new(),
    };
    if let Some(kids) = root.get("K") {
        walk.kids(kids, None, 0);
    }
    owners
}

/// For each marked identifier of the page, the innermost element holding it
/// that carries an `/ActualText`, with that text.
pub(crate) fn page_actual_texts(
    file: &PdfFile,
    page: ObjectRef,
) -> BTreeMap<i64, (ObjectRef, String)> {
    page_owners(file, page)
        .into_iter()
        .filter_map(|(mcid, chain)| {
            chain.iter().find_map(|element| {
                let dict = file.get_dictionary(*element).ok()?;
                match file.resolve(dict.get("ActualText")?).ok()? {
                    PdfValue::String(text) => {
                        Some((mcid, (*element, crate::text::text_string(&text.0))))
                    }
                    _ => None,
                }
            })
        })
        .collect()
}

struct Walk<'a> {
    file: &'a PdfFile,
    owners: &'a mut StructureOwners,
    visited: BTreeSet<ObjectRef>,
    chain: Vec<ObjectRef>,
}

impl Walk<'_> {
    fn kids(&mut self, kids: &PdfValue, page: Option<ObjectRef>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match kids {
            PdfValue::Array(items) => {
                for item in items {
                    self.kids(item, page, depth + 1);
                }
            }
            PdfValue::Integer(mcid) => self.record(*mcid, page),
            PdfValue::Reference(reference) => {
                if !self.visited.insert(*reference) {
                    return;
                }
                if let Ok(dict) = self.file.get_dictionary(*reference) {
                    self.node(Some(*reference), dict, page, depth);
                }
            }
            PdfValue::Dictionary(dict) => self.node(None, dict, page, depth),
            _ => {}
        }
    }

    fn node(
        &mut self,
        reference: Option<ObjectRef>,
        dict: &PdfDictionary,
        page: Option<ObjectRef>,
        depth: usize,
    ) {
        let page = match dict.get("Pg") {
            Some(PdfValue::Reference(own)) => Some(*own),
            _ => page,
        };
        match dict.get("Type").and_then(PdfValue::as_name) {
            Some("MCR") => {
                if dict.get("Stm").is_none()
                    && let Some(mcid) = dict.get("MCID").and_then(PdfValue::as_integer)
                {
                    self.record(mcid, page);
                }
            }
            Some("OBJR") => {}
            _ => {
                let Some(kids) = dict.get("K") else {
                    return;
                };
                if let Some(reference) = reference {
                    self.chain.push(reference);
                }
                self.kids(kids, page, depth + 1);
                if reference.is_some() {
                    self.chain.pop();
                }
            }
        }
    }

    fn record(&mut self, mcid: i64, page: Option<ObjectRef>) {
        if let Some(page) = page
            && !self.chain.is_empty()
        {
            let mut chain = self.chain.clone();
            chain.reverse();
            self.owners
                .entry(page)
                .or_default()
                .entry(mcid)
                .or_insert(chain);
        }
    }
}
