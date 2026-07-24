//! `lj-api` — serveur API mono-serveur (port de `apps/api`).
//!
//! Axum + tokio-postgres/deadpool + pgvector. Endpoint MCP (rmcp). Auth OAuth +
//! JWT. Télémétrie tracing + OTel. Les DTOs viennent de `lj-dtos` (source de
//! vérité partagée avec le front).

pub mod auth;
pub mod bookmarks;
pub mod cache;
pub mod config;
pub mod decision_views;
pub mod decisions;
pub mod docx_export;
pub mod embedder;
pub mod entities;
pub mod error;
pub mod jurisdiction_hubs;
pub mod legi;
pub mod mcp;
pub mod mcp_presenters;
pub mod me;
pub mod norm_hubs;
pub mod oauth;
pub mod pdf_export;
pub mod pg_metrics;
pub mod redirect;
pub mod referential;
pub mod registre;
pub mod rerank;
pub mod routes;
pub mod search;
pub mod search_history;
pub mod signals;
pub mod sitemap;
pub mod snippets;
pub mod state;
pub mod stats;
pub mod telemetry;
pub mod titles;

#[cfg(test)]
mod dependency_boundary {
    /// Frontière serve : `lj-api` ne linke jamais le moteur d'extraction
    /// (`lj-extract`). Une primitive partagée extraction ↔ serve (ex.
    /// `fold_stable`) monte dans `lj-core`, jamais l'inverse.
    #[test]
    fn no_lj_extract_in_serve_path() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            !manifest.contains("lj-extract"),
            "lj-api ne doit pas dépendre de lj-extract : \
             promouvoir la primitive partagée dans lj-core"
        );
    }
}
