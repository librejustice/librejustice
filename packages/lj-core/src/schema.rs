//! Taxonomie décisions vue depuis le noyau.
//!
//! La taxonomie partagée api ↔ web vit dans `lj-dtos` (ADR 0060), re-exportée
//! ici pour que `lj_core::schema::X` et les `crate::schema::X` internes
//! résolvent à l'identique. Depuis la fusion extraction→référentiels (ADR
//! 0148, v12), les scanners émettent directement les clés des référentiels
//! (`solution:*`-17, `voie:*`, `office:*`, `domaine:*`) — plus aucun
//! vocabulaire ancien-monde ici.

pub use lj_dtos::schema::*;
