//! Downloaders : opendata ZIP + sync incrémental Judilibre.
//!
//! Persiste un [`Manifest`] JSON sur disque pour la reprise idempotente. Les
//! deux sources ont des formats de manifeste distincts (opendata : `entries`
//! par `{jur}/{yyyymm}` ; Judilibre : `jurisdictions` + watermark/cursor
//! `/transactionalhistory`). [`Manifest`] porte les deux formes ; `load`
//! détecte le format présent sur disque et `save` réécrit la forme courante.
//!
//! - opendata : I/O HTTP synchrone via `reqwest::blocking`, pas de
//!   parallélisme ;
//! - judilibre : `sync_judilibre` lit l'historique page à page et les
//!   décisions d'une page en parallèle.

mod calendar;
mod compact;
pub(crate) mod http;
mod judilibre;
mod manifest;
mod opendata;
mod sha256;

pub use compact::compact_archives;
pub use judilibre::{
    court_total, list_slices, month_slices, parse_decision_line, read_jsonl_gz_lines,
    reconcile_fetch, reconcile_tombstone, slice_totals, sync_judilibre, year_slices, Fetched,
    Slice,
};
pub use manifest::{Entry, JurState, Manifest, MonthState};
pub use opendata::sync_opendata;

pub(crate) use http::{get_to_file_retrying, get_with_body_retrying};
