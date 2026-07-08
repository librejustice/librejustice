//! Repository de décisions chunk-aware (Postgres + pgvector).
//! Port de `repository.py`. Aucun SQL ailleurs (règle #2).
//!
//! Méthodes clés : [`DecisionRepository::upsert`] (INSERT/UPDATE métadonnées),
//! `replace_chunks` (DELETE + bulk INSERT chunks), `replace_citations`
//! (`legal_citation` à plat, module `citations`), `replace_source_payload`,
//! `delete`. Le pipeline appelle ces méthodes dans une transaction unique.
//!
//! Découpé par domaine en sous-modules (blocs `impl DecisionRepository`
//! répartis) ; ce `mod.rs` détient le struct, le constructeur et les
//! réexports publics.

mod bench;
mod cases;
mod chunks;
mod citations;
mod decisions;
mod identity;
mod links;
mod referential;
mod support;
mod types;

use crate::db::Connection;

pub use bench::ManualFields;
pub use cases::CaseCitationWriteItem;
pub use citations::CitationWriteItem;
pub use links::DecisionLinkWriteItem;
pub use support::source_from_source_uid;
pub use types::{
    ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, BulkDecisionWrite, CaseCitationRow,
    ChunkWrite, CitationOccurrenceRow, CitingDecisionRow, DecisionLinkRow, ExistingDecisionState,
    ExtractedFields, FacetCount, FacetValueRow, GtDoc, JurisdictionRow, LawCodeSummaryRow,
    LawVersionRow, LegalArticleRow, LegalTextCatalogRow, LegalTextMeta, LegalTextRow,
    MissingSummaryRow, SitemapRow, TocArticleRow, UpsertResult, UpsertStatus,
};

pub use support::REEXTRACTABLE_FIELDS;

/// Accès CRUD typé aux tables chunk-aware (cf. ADR 0014). Détient une connexion
/// checkout du pool ; le pipeline wrappe ses appels dans une transaction.
pub struct DecisionRepository<'a> {
    pub conn: &'a Connection,
}

impl<'a> DecisionRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}
