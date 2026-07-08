//! Helpers partagés des extracteurs (port FIDÈLE de `extract/common.py`).
//!
//! PUR : aucun I/O au runtime. Le chemin `crate::extract::common::X` reste
//! stable : `mod.rs` réexporte chaque helper depuis son sous-module thématique.
//!
//! ## Note sur les regex
//!
//! `regex` (crate) ne supporte ni lookahead `(?=…)` ni lookbehind `(?<=…)`.
//! Le Python `re` en abuse. Stratégie de portage :
//! - lookbehind/lookahead sur des classes simples (`(?<=\d)…(?=\d)`) → réécrits
//!   en groupes capturants + replacement (`$1-$2`) : strictement équivalent ;
//! - lookahead de borne en fin de motif (`…(?=\s*[,;.])`) → capture gloutonne
//!   contrôlée + coupe programmatique (`normalize_instrument`) ;
//! - négations de lookahead sur l'identifiant d'article (`_RE_ARTICLE_CORE`) →
//!   parcours caractère par caractère reproduisant la sémantique.
//!
//! Les grosses regex de CITATION (`_RE_ART_CITATION`, `_RE_ARTICLES_CITATION`,
//! `_RE_VU_ARTICLE`, `_RE_CHAINED_ARTICLE_CITATION`) ne sont *pas* portées ici :
//! elles ne sont pas appelées par `common.py` (elles vivent dans
//! `opendata.py` / `judilibre.py`, hors périmètre de cet agent) et dépendent
//! de lookahead non triviaux. Voir `unresolved` du rapport.

mod articles;
mod counsel;
mod dates;
pub(crate) use dates::parse_french_date;
mod instruments;
/// Signaux structurés d'une clé de citation (ADR 0144) — fonction pure de la
/// chaîne, substrat des règles du résolveur (DatedAct/EuNum/ForeignCode…).
pub mod key_signals;
// Primitives texte promues dans `lj-core` (partagées avec `aliases` côté serve,
// ADR 0123) ; re-exportées ici pour que `super::text::X` des sous-modules résolve.
pub(crate) use lj_core::text;

pub use articles::normalize_article;
pub use instruments::{is_unresolvable_instrument, normalize_instrument};

pub(crate) use instruments::FOREIGN_NATIONALITY_STEMS;

pub(crate) use counsel::{dedupe_prefix_variants, unique_nonempty};
pub(crate) use dates::{clean_date_iso, clean_docket_numbers, extract_textual_audience_date};
pub(crate) use text::{fold, normalize_spaces};
