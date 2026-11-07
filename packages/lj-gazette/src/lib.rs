//! BODACC and JOAFE notices from the DILA XML bulk, decomposed into stored columns with nothing lost.

pub mod address;
pub mod bodacc;
pub mod census;
pub mod joafe;
pub mod officers;
pub mod record;
pub mod rows;
pub mod tree;

#[cfg(test)]
mod tests;

/// Dictionary kinds, the first key of a stored `(kind, code, value)`: never renumbered.
pub mod dict {
    pub const SHAPE: i16 = 1;
    pub const ADDRESS_SHAPE: i16 = 2;
    pub const HEADER_SHAPE: i16 = 3;
    pub const ROLE: i16 = 4;
    pub const DEPT: i16 = 10;
    pub const TRIBUNAL: i16 = 11;
    pub const GREFFE: i16 = 12;
    pub const FORME: i16 = 13;
    pub const JUG_FAMILLE: i16 = 14;
    pub const JUG_NATURE: i16 = 15;
    pub const CATEGORY: i16 = 16;
    pub const ORIGIN: i16 = 17;
    pub const QUALITE: i16 = 18;
    pub const MODIF: i16 = 19;
    pub const RADIATION: i16 = 20;
    pub const DEPOT_TYPE: i16 = 21;
    pub const DEPOT_DESC: i16 = 22;
    pub const CODE_RCS: i16 = 23;
    pub const DEVISE: i16 = 24;
    pub const JOAFE_TYPE: i16 = 30;
    pub const JOAFE_DEPT: i16 = 31;
    pub const JOAFE_THEME: i16 = 32;
    pub const JOAFE_LIEU: i16 = 33;

    /// Kinds coded as `integer`; the others are `smallint`.
    pub fn wide(kind: i16) -> bool {
        matches!(
            kind,
            SHAPE | ADDRESS_SHAPE | HEADER_SHAPE | ROLE | ORIGIN | MODIF
        )
    }
}

/// Stored as `gazette_file.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Bodacc = 1,
    Joafe = 2,
}

impl Source {
    pub fn slug(self) -> &'static str {
        match self {
            Source::Bodacc => "bodacc",
            Source::Joafe => "joafe",
        }
    }
}

/// A department as the official codes write it: the JOAFE also writes `1` for `01` and `20A` for `2A`.
pub fn canonical_dept(value: &str) -> std::borrow::Cow<'_, str> {
    match value {
        "20A" => "2A".into(),
        "20B" => "2B".into(),
        v if v.len() == 1 && v.as_bytes()[0].is_ascii_digit() => format!("0{v}").into(),
        v => v.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
