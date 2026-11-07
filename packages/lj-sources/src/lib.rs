//! `lj-sources` — I/O sources.
//!
//! Client Judilibre (reqwest), downloader incrémental + ZIP opendata, lecteur
//! d'archives ZIP. Aucune logique de parsing métier ici : les bytes/JSON bruts
//! sont remis aux parsers de `lj-core`.

pub mod adlc;
pub mod ariane;
pub mod bofip;
pub mod bulletins;
pub mod ce_list;
pub mod cedh;
pub mod circulaires;
pub mod cjue;
pub mod cnda;
pub mod comeu;
pub mod curia;
pub mod ddd;
pub mod dila;
pub mod docx;
pub mod downloader;
pub mod error;
pub mod gazette;
pub mod hal;
pub mod html_strip;
pub mod judilibre;
pub mod juricaf;
pub mod legifrance;
pub mod oai;
pub mod pdf;
pub mod piste;
pub mod raa;
pub mod registries;
pub mod service_public;
pub mod state_paths;
pub mod tar_reader;
pub mod wiki;
pub mod zip_reader;
