//! Repository de décisions chunk-aware (Postgres + pgvector).
//! Aucun SQL ailleurs.
//!
//! Méthodes clés : [`DecisionRepository::upsert`] (INSERT/UPDATE métadonnées),
//! `replace_chunks` (DELETE + bulk INSERT chunks), `replace_citations`
//! (`legal_citation` à plat, module `citations`), `delete`. Le pipeline appelle
//! ces méthodes dans une transaction unique.
//!
//! Découpé par domaine en sous-modules (blocs `impl DecisionRepository`
//! répartis) ; ce `mod.rs` détient le struct, le constructeur et les
//! réexports publics.

mod alias;
mod annonces;
mod annonces_invariants;
mod assertions;
mod bench;
mod case_names;
mod cases;
mod cedh_application;
mod chunks;
mod citations;
mod cluster;
mod conclusions;
mod corpus_counts;
mod court_summaries;
mod decision_text;
mod decisions;
mod disposable_domains;
mod entities;
mod expiry;
mod gazette;
mod identity;
mod instrument_move;
mod journal;
mod journal_move;
mod jurisdiction_hubs;
mod language_texts;
mod legal_links;
mod legal_toc;
mod links;
mod llm_keys;
pub use llm_keys::{Credential, NewCredential};
mod norm_hubs;
mod norm_passages;
mod norm_visit;
mod parliament;
mod parties;
pub mod pick;
mod quantum;
mod raa;
mod referential;
mod rekey;
mod renvois;
mod scopes;
mod similar;
mod submissions;
mod suggest;
mod summary_batch;
mod support;
mod text_rewrite;
mod types;
mod usage_terms;
mod withdrawal;

use lj_pg::db::Connection;

pub use alias::{AliasCandidat, AliasRevue};
pub use annonces::{
    AnnonceAEnrichir, AnnonceAJumeler, AnnonceAxe, AnnonceCanalOuvert, AnnonceCanalRow,
    AnnonceChamps, AnnonceDecrite, AnnonceEntiteRow, AnnonceFiltre, AnnonceOrdre, AnnonceOrpheline,
    AnnonceRow, AnnonceSourceWriteItem, AnnonceWriteItem, AnnonceWriteStats, ContenuPose,
    DetailPose, EntityAnnonceRow, FileEnrichissement, ResultatPose, SondeLiens, TableSize,
    TrouCouverture,
};
pub use assertions::{
    covering_states, AssertionControl, AssertionRow, AssertionWrite, BareTextCandidate,
    CompileCandidate, CompileStateWrite, CoverageRuleWrite, CoverageWrite, ExpressionWrite,
    PlaceCorpusRow, VersionForCompile, VersionKeyRow,
};
pub use bench::{
    Courts, DocTexts, EmbedInput, EmbedScope, EmbedText, ManualFields, ManualUpsert,
    NormBenchArticle, StoredSummary, SummaryContext,
};
pub use case_names::{DayHead, NamedCaseRow};
pub use cases::{CaseCitationWriteItem, ChronoCoverageRow};
pub use cedh_application::{
    CedhApplicationRow, CommunicatedStep, HeldCedhRuling, PendingCedhApplication,
};
pub use citations::CitationWriteItem;
pub use conclusions::{ConclusionsPage, ConclusionsRow, ConclusionsWrite};
pub use decision_text::{DecisionTextDrift, DecisionTextRead, RepairText, StoredText};
pub use decisions::canonical_jurisdiction_code;
pub use entities::{
    AffiliationEdge, CnbEntityState, CnbEntityUpdate, EntityHistoryWriteItem, EntityWriteItem,
};
pub use gazette::{BodaccNoticeRow, GazetteFacets, GazetteFilter, GazetteQuery, JoafeNoticeRow};
pub use identity::{
    JoinderPair, JudilibreProvenance, ProvenanceToResolve, WebNoteHead, SOURCE_TWIN_CONTAINMENT,
};
pub use instrument_move::{TextRenameCounts, TextVersionMerge};
pub use journal::{
    Edition, IssuePlacement, IssueRef, JoActRow, JorfPiece, JournalIssueRow, NoticePublicationRow,
    PieceSource, PublicationFacts, PublicationRow, TextPublicationRow, TreatyCutCandidate,
};
pub use journal_move::{JournalMoveCandidate, JournalMoveCounts};
pub use language_texts::{ArticleLanguageText, ArticleLanguageWrite};
pub use legal_toc::TocEdges;
pub use links::{CedhCandidateRow, CedhVisit, DecisionLinkWriteItem, PendingCedhLink};
pub use norm_passages::{NormPassageInput, NormPassageWrite};
pub use norm_visit::{NormVisit, StoredArticleVersion};
pub use parliament::ParliamentDocumentRow;
pub use parties::DecisionActorWriteItem;
pub use quantum::{QuantumObservationRow, QuantumWriteItem};
pub use raa::{RaaHitRow, RaaListedRow, RaaNotice, RaaQueued, RaaRead, RaaScanned};
pub use referential::{merge_article_facets, ArticleVersions, TransitRecueil};
pub use rekey::StarConflict;
pub use renvois::{
    CaseNameMatch, ElectedCaseName, NamedDecision, Renvoi, RenvoiNomme, TitleInputs,
};
pub use scopes::{ArticleScopeRow, CompteurArticleRow};
pub use similar::{
    assemble_principe, similar_espece_gated, similar_principe_gated, SimilarGraph, SimilarLegs,
    SimilarPanels, SimilarSourceMeta,
};
pub use submissions::PendingSubmission;
pub use suggest::SUGGEST_FST_KEY;
pub use summary_batch::SummaryBatchRow;
pub use support::source_from_source_uid;
pub use text_rewrite::{ArticleTextDrift, ArticleTextRead, ArticleTextWrite};
pub use types::{
    ArticleBrowseFilters, ArticleCandidate, ArticleCitationSpanRow, ArticleDesignation,
    ArticleFacetMember, ArticleLegHit, ArticleNeighborRow, ArticleRankHit, ArticleRrf,
    ArticleSearchRow, ArticleSearchStats, ArticleTitleMode, BodyWrite, BulkDecisionWrite,
    CaseCitationRow, ChunkWrite, CitationOccurrenceRow, CitingDecisionRow, CitingText,
    CoCitedArticleRow, CollageRow, DecisionLinkRow, DecisionPartyReadRow, DecisionPartyRow,
    DisplayInput, EntityContentieuxCounts, EntityDecisionRow, EntityDenominationReadRow,
    EntityDirectoryRow, EntityHeaderRow, EntityJurisdictionCountRow, EntityYearCountRow, EuTwin,
    ExistingDecisionState, ExtractedFields, FacetCount, FacetValueRow, Fraicheur, GtDoc,
    HubDecisionRow, Identity, InstrumentCandidate, InstrumentResolution, JurisdictionHubPages,
    JurisdictionHubRow, JurisdictionRow, LawCodeSummaryRow, LawVersionRow, LegalArticleRow,
    LegalLinkOwner, LegalLinkRow, LegalTextCatalogRow, LegalTextMeta, LegalTextRow,
    MissingSummaryRow, NamedText, NormHubPages, NormTextRow, PassageInputs, PickedText,
    PlaceFilter, PlaceReading, PlacedElsewhere, ReferentialRef, ResolvedLegalLink, SectionArticle,
    SectionArticles, SectionCitingTextRow, SeededJurisdiction, SitemapArticle, SitemapDecision,
    SitemapRow, SlugSourceRow, SummaryStamp, TextCaseCitationRow, TextLegalCitationRow,
    TextListRow, TocArticleRow, TocEdgeRow, TocOwner, TocReadingRow, TocTreeRow, TreatyTitle,
    UpsertResult, UpsertStatus,
};

pub use support::REEXTRACTABLE_FIELDS;

/// Accès CRUD typé aux tables chunk-aware. Détient une connexion checkout du
/// pool ; le pipeline wrappe ses appels dans une transaction.
pub struct DecisionRepository<'a> {
    pub conn: &'a Connection,
}

impl<'a> DecisionRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}
