//! `lj-core`: the decision model shared by the server and the ingest.
//!
//! No I/O at runtime: the `Decision` model, its dates, docket keys, chambers
//! and publication, and the text primitives it rests on (recasing, cleaning,
//! titles). Static data is embedded through `include_str!`. The source parsers
//! live in `lj-parse`, the referential of the legal texts in `lj-norm`, the
//! search text analysis in `lj-textsearch`, the stamped versions in
//! `lj-versions`.

pub mod address;
pub mod annexes;
pub mod case_names;
pub mod chamber;
pub mod citation;
pub mod dates;
pub mod decision;
pub mod docket_key;
pub mod error;
pub mod formation;
pub mod forme_juridique;
pub mod identity;
pub mod lettered_par;
pub mod normalizer;
pub mod official_url;
pub mod outremer;
pub mod publication;
pub mod text;
pub mod titles;
pub mod truecase;
