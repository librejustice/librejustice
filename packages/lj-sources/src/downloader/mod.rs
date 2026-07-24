//! Downloaders (port de `sources/downloader.py` opendata ZIP +
//! `sources/judilibre/downloader.py` sync incrémental).
//!
//! Persiste un [`Manifest`] JSON sur disque pour la reprise idempotente. Les
//! deux sources ont des formats de manifeste distincts (opendata : `entries`
//! par `{jur}/{yyyymm}` ; Judilibre : `jurisdictions` + watermark/cursor
//! `/transactionalhistory`). [`Manifest`] porte les deux formes ; `load`
//! détecte le format présent sur disque et `save` réécrit la forme courante.
//!
//! Différences assumées vs Python (cf. `unresolved`) :
//! - pas de barre de progression `tqdm` ;
//! - opendata : I/O HTTP synchrone via `reqwest::blocking` (le Python utilise
//!   `requests`), pas de parallélisme ;
//! - judilibre : `sync_judilibre` est mono-thread (le Python parallélise le
//!   bootstrap mensuel via `ThreadPoolExecutor`) — logique identique, ordre
//!   séquentiel ; le verrou `fcntl.flock` n'est pas reporté.

mod calendar;
mod compact;
pub(crate) mod http;
mod judilibre;
mod manifest;
mod opendata;
mod sha256;

pub use compact::compact_archives;
pub use judilibre::sync_judilibre;
pub use manifest::{Entry, JurState, Manifest, MonthState};
pub use opendata::sync_opendata;

pub(crate) use http::{get_to_file_retrying, get_with_body_retrying};
