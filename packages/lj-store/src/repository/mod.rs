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
mod entities;
mod identity;
mod jurisdiction_hubs;
mod legal_links;
mod legal_toc;
mod links;
mod llm_keys;
mod norm_hubs;
mod parties;
mod referential;
mod rekey;
mod suggest;
mod support;
mod types;
mod usage_terms;

use crate::db::Connection;

pub use bench::ManualFields;
pub use cases::CaseCitationWriteItem;
pub use citations::CitationWriteItem;
pub use entities::{EntityHistoryWriteItem, EntityWriteItem};
pub use links::DecisionLinkWriteItem;
pub use parties::DecisionPartyWriteItem;
pub use suggest::SUGGEST_FST_KEY;
pub use support::source_from_source_uid;
pub use types::{
    ArticleCitationSpanRow, ArticleNeighborRow, ArticleRankHit, ArticleRrf, ArticleSearchRow,
    ArticleSearchStats, ArticleTitleMode, BulkDecisionWrite, CaseCitationRow, ChunkWrite,
    CitationOccurrenceRow, CitingDecisionRow, CoCitedArticleRow, DecisionLinkRow,
    DecisionPartyReadRow, DecisionPartyRow, EntityContentieuxCounts, EntityCounselRow,
    EntityDecisionRow, EntityDenominationReadRow, EntityDirectoryRow, EntityHeaderRow,
    EntityJurisdictionCountRow, EntityYearCountRow, ExistingDecisionState, ExtractedFields,
    FacetCount, FacetValueRow, GtDoc, HubDecisionRow, JurisdictionHubRow, JurisdictionRow,
    LawCodeSummaryRow, LawVersionRow, LegalArticleRow, LegalLinkOwner, LegalLinkRow,
    LegalTextCatalogRow, LegalTextMeta, LegalTextRow, MissingSummaryRow, NormTextRow,
    ResolvedLegalLink, SitemapRow, SlugSourceRow, TextCaseCitationRow, TextLegalCitationRow,
    TocArticleRow, TocEdgeRow, TocOwner, TocReadingRow, TocTreeRow, UpsertResult, UpsertStatus,
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
