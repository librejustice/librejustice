//! `lj-sources` — I/O sources (port de `librejustice-core/sources`).
//!
//! Client Judilibre (reqwest), downloader incrémental + ZIP opendata, lecteur
//! d'archives ZIP. Aucune logique de parsing métier ici : les bytes/JSON bruts
//! sont remis aux parsers de `lj-core`.

pub mod cedh;
pub mod cjue;
pub mod cnda;
pub mod dila;
pub mod docx;
pub mod downloader;
pub mod error;
pub mod fetch;
pub mod html_strip;
pub mod judilibre;
pub mod legifrance;
pub mod pdf;
pub mod piste;
pub mod tar_reader;
pub mod zip_reader;
