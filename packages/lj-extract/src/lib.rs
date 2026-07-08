//! `lj-extract` — étage INGEST de l'extraction légale (ADR 0123).
//!
//! Sorti de `lj-core` pour que le binaire serveur (`lj-server`) ne compile plus
//! cette pile lourde qu'il n'exécute jamais : recognizer de citations (DFA à
//! `dfa_size_limit(64 Mo)`), normaliseurs (`normalize_instrument`/`_article`),
//! parsers de sources Légifrance/JORF/KALI, identité canonique.
//!
//! PUR comme `lj-core` (aucune I/O, aucun client réseau) ; il dépend de `lj-core`
//! (primitives texte partagées, modèle `Decision`, `error`, `schema`) et de
//! `lj-dtos`. Tiré uniquement par `lj-ingest` et `lj-bench`.

pub mod articles;
pub mod chrono;
pub mod compiled;
pub mod data;
pub mod domain;
pub mod extract;
pub mod facets;
pub mod formation;
pub mod gazetteer;
pub mod identity;
pub mod instrument_key;
pub mod jorf;
pub mod kali;
pub mod legi;
pub mod link;
pub mod scan;
