//! Module SEO. Each page renders its meta through `leptos_meta` (`<Title>`/`<Meta>`);
//! the shell sets the site's.
//!
//! - `generic` : meta site/404, constantes canoniques.
//! - `decision` : titre/description/JSON-LD des pages decision (donnee depuis
//!   les DTO `lj-dtos`).
//! - `search` : meta de la page recherche.

pub mod decision;
pub mod generic;
pub mod law;
pub mod search;

pub use generic::{canonical_url, Canonical, CANONICAL_BASE, OG_IMAGE, SITE_JSONLD};
