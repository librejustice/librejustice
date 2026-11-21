//! Composants de la page decision.

pub mod decision_body;
pub mod decision_citantes;
pub mod decision_commentaires;
pub mod decision_conclusions;
pub mod decision_header;
pub mod decision_layout;
pub mod decision_meta;
pub mod decision_parties;
pub mod decision_provenance;
pub mod decision_references;
pub mod decision_similar;
pub mod decision_skeleton;
pub mod decision_texts;
pub mod decision_toc;
pub mod selection;
pub mod toc_spy_dom;

pub use decision_body::{use_search_hits, DecisionBody};
pub use decision_citantes::DecisionCitantes;
pub use decision_commentaires::DecisionCommentaires;
pub use decision_conclusions::DecisionConclusions;
pub use decision_header::{DecisionActions, DecisionHeader};
pub use decision_layout::DecisionLayout;
pub use decision_meta::DecisionMeta;
pub use decision_parties::DecisionParties;
pub use decision_provenance::DecisionProvenance;
pub use decision_references::DecisionReferences;
pub use decision_similar::DecisionSimilar;
pub use decision_skeleton::DecisionSkeleton;
pub use decision_texts::DecisionTextStatus;
pub use decision_toc::DecisionToc;
