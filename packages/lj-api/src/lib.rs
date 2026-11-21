//! `lj-api` — serveur API mono-serveur.
//!
//! Axum + tokio-postgres/deadpool + pgvector. Endpoint MCP. Auth OAuth +
//! JWT. Télémétrie tracing + OTel. Les DTOs viennent de `lj-dtos` (source de
//! vérité partagée avec le front).

pub mod account;
pub mod accounts;
pub mod admission;
pub mod api_keys;
pub mod auth;
pub mod cache;
pub mod cache_metrics;
pub mod chronology;
pub mod config;
pub mod contribution;
pub mod decisions;
mod disposable;
pub mod embedder;
pub mod entities;
pub mod error;
pub mod host_metrics;
pub mod inpi;
pub mod journals;
pub mod journals_bodacc;
mod journals_gazette;
pub mod journals_jo;
pub mod journals_joafe;
pub mod journals_raa;
pub mod jurisdiction_hubs;
pub mod jurisdictions;
pub mod legi;
pub mod locale;
pub mod mail;
pub mod mcp;
pub mod mcp_presenters;
pub mod me;
pub mod norm_hubs;
pub mod notices;
mod notices_gazette;
pub mod oauth;
pub mod oidc_auth;
pub mod oidc_keys;
pub mod open_source;
pub mod page_gates;
pub mod pdf_export;
pub mod pg_metrics;
pub mod ratelimit;
pub mod reads;
pub mod redact;
pub mod referential;
pub mod registre;
pub mod rerank;
pub mod robots;
pub mod routes;
pub mod search;
pub mod session;
pub mod signals;
pub mod sitemap;
pub mod snippets;
pub mod state;
pub mod stats;
pub mod support;
pub mod telemetry;
pub mod titles;
pub mod typed_cache;
pub mod usage;

#[cfg(test)]
mod dependency_boundary {
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
