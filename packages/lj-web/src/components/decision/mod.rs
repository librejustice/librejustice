//! Composants de la page decision (STUBS). A remplir par l'agent de la tranche
//! Decision : port de `apps/web/src/components/decision/*`. Les stubs compilent
//! et rendent un placeholder minimal ; la signature des `#[component]` peut etre
//! ajustee a l'implementation (les noms de fichiers/modules, eux, sont figes).

pub mod decision_body;
pub mod decision_commentaires;
pub mod decision_header;
pub mod decision_layout;
pub mod decision_meta;
pub mod decision_parties;
pub mod decision_provenance;
pub mod decision_similar;
pub mod decision_skeleton;
pub mod decision_toc;

pub use decision_body::DecisionBody;
pub use decision_commentaires::DecisionCommentaires;
pub use decision_header::DecisionHeader;
pub use decision_layout::DecisionLayout;
pub use decision_meta::DecisionMeta;
pub use decision_parties::DecisionParties;
pub use decision_provenance::DecisionProvenance;
pub use decision_similar::DecisionSimilar;
pub use decision_skeleton::DecisionSkeleton;
pub use decision_toc::DecisionToc;
