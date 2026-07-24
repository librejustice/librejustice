//! Réconciliation des liens « pendants » (ADR 0240) : un unique passage qui
//! rejoue tous les résolveurs batch de fin de pass — chronologie (0161),
//! citations de jurisprudence décision→décision (0165) et texte→décision (0196),
//! acteurs (0182) — puis reconstruit l'annuaire. Partagé entre la queue de
//! `reextract-fields` et la commande `db reconcile` (rattrapage idempotent après
//! un ingest interrompu).

use anyhow::Result;
use lj_store::repository::DecisionRepository;

/// Rejoue les quatre résolveurs de liens pendants sur `repo` puis rafraîchit
/// l'annuaire (`relink_with` : parties + compteurs `entity`, ADR 0239).
/// Idempotent.
pub async fn reconcile_pending(repo: &DecisionRepository<'_>) -> Result<()> {
    // Chronologie (ADR 0161) : une décision (re)liée peut être la cible de liens
    // pendants plus anciens.
    let resolved = repo.resolve_pending_decision_links().await?;
    tracing::info!(resolved, "liens de chronologie pendants résolus");
    // Citations de jurisprudence décision→décision (ADR 0165).
    let resolved = repo.resolve_pending_case_citations().await?;
    tracing::info!(resolved, "citations de jurisprudence pendantes résolues");
    // Citations texte→jurisprudence (ADR 0196) : un corps de référentiel peut
    // citer une décision entre-temps ingérée.
    let resolved = repo.resolve_pending_text_case_citations().await?;
    tracing::info!(resolved, "citations texte→jurisprudence pendantes résolues");
    // Acteurs + annuaire (ADR 0182) : `decision_party` pendantes → référentiel
    // d'entités, puis refresh des compteurs annuaire d'`entity`.
    super::parties::relink_with(repo).await
}
