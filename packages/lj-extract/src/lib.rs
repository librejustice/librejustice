//! `lj-extract` — étage INGEST de l'extraction légale : recognizer de citations
//! (DFA à `dfa_size_limit(64 Mo)`), normaliseurs
//! (`normalize_instrument`/`_article`), parsers de sources Légifrance/JORF/KALI,
//! identité canonique.
//!
//! PUR comme `lj-core` (aucune I/O, aucun client réseau) ; il dépend de `lj-core`
//! (primitives texte partagées, modèle `Decision`, `error`, `schema`) et de
//! `lj-dtos`. Tiré par l'ingest et les bancs offline, jamais par le serveur.

pub mod articles;
pub mod case_link;
pub mod cedh;
pub mod chrono;
pub mod compiled;
pub mod data;
pub mod doctrine;
pub mod domain;
pub mod eponyme;
pub mod extract;
pub mod facets;
pub mod foreign;
pub mod formation;
pub mod gazetteer;
pub mod identity;
pub mod instrument_key;
pub mod instrument_mint;
pub mod joinder;
pub mod jorf;
pub mod kali;
pub mod legi;
pub mod link;
pub mod ordinal;
pub mod parties;
pub mod registry;
pub mod scan;
pub mod seat_link;
