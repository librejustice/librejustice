//! Analyse **read-only** des faux merges judilibre (#29 / ADR 0100).
//!
//! La dédup historique a fusionné des décisions distinctes partageant un RG sans
//! `location` (cause des faux merges, ADR 0100 §2). `fuse_cluster` re-pointe les
//! `decision_sources` des doublons vers le canonique puis supprime les lignes
//! `decisions` doublons : une décision faussement fusionnée porte donc ≥2
//! `decision_sources` actifs, chacun gardant son `source_fields`.
//!
//! Cette passe **ne touche rien** : elle recalcule `canonical_ref` (ADR 0100) par
//! provenance depuis `(source_fields, source_uid)` — full_text vide, les
//! discriminants (type, location, RG, date) vivent dans les champs structurés —
//! et classe chaque décision multi-provenance. Objectif : mesurer l'ampleur réelle
//! avant tout re-split (écriture prod, étape distincte gatée). Penché **100 %
//! spécificité** : une provenance sans clé exploitable (`None`) rend le verdict
//! `Ambiguous` (jamais scindée).

use anyhow::{anyhow, Result};
use std::collections::BTreeSet;

use lj_core::decision::Decision;
use lj_store::repository::DecisionRepository;
use serde_json::Value;

use crate::config::Settings;

/// Verdict d'une décision multi-provenance.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Toutes les provenances portent la **même** `canonical_ref` → fusion
    /// légitime (ex. Cass abrégé Bulletin + intégrale partagent la clé).
    Legit,
    /// ≥2 `canonical_ref` distinctes, toutes renseignées → décisions différentes
    /// faussement fusionnées. `groups` = nombre de décisions distinctes à
    /// reconstituer.
    FalseMerge { groups: usize },
    /// Au moins une provenance sans clé exploitable (`None`) → on ne peut pas
    /// certifier la séparation. Conservateur : jamais scindée (#12, spécificité).
    Ambiguous,
}

/// Classe une décision d'après les `canonical_ref` (ADR 0100) recalculés de ses
/// provenances. Penché 100 % spécificité : tout `None` ⇒ `Ambiguous`.
fn classify(refs: &[Option<String>]) -> Verdict {
    if refs.iter().any(Option::is_none) {
        return Verdict::Ambiguous;
    }
    let distinct: BTreeSet<&String> = refs.iter().flatten().collect();
    match distinct.len() {
        0 => Verdict::Ambiguous, // provenances vides : ne devrait pas arriver
        1 => Verdict::Legit,
        n => Verdict::FalseMerge { groups: n },
    }
}

/// `canonical_ref` (ADR 0100) d'une provenance judilibre, reconstruite depuis
/// `(source_fields, source_uid)` avec full_text vide (les discriminants d'identité
/// sont dans les champs structurés ; le full_text du membre supprimé est perdu et
/// inutile ici). `None` si la juridiction n'est pas routée ou si les briques
/// fiables manquent.
fn provenance_canonical_ref(source_fields: &Value, source_uid: &str) -> Option<String> {
    let decision = Decision::from_source_fields("", source_fields, source_uid);
    lj_extract::extract::routed(&decision).ok()?;
    lj_extract::identity::decision_canonical_ref(&decision)
}

/// Compteurs agrégés de l'analyse.
#[derive(Default)]
struct Stats {
    decisions: u64,
    legit: u64,
    ambiguous: u64,
    false_merges: u64,
    extra_decisions: u64, // décisions à reconstituer = Σ(groups − 1) sur les faux merges
}

/// Analyse read-only des faux merges judilibre. N'écrit rien.
pub async fn analyze_false_merges() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Agrégats sur ~3M lignes par batch : on lève la borne API (statement_timeout).
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let mut last_id: i64 = 0;
    let mut stats = Stats::default();
    let mut examples: Vec<(i64, Vec<String>)> = Vec::new();
    const BATCH: i64 = 2000;

    loop {
        let rows = repo
            .fetch_judilibre_multiprovenance_batch(last_id, BATCH)
            .await?;
        let Some(&max_id) = rows.iter().map(|(id, ..)| id).max() else {
            break;
        };

        // Regroupe les provenances par décision (rows triées par decision_id).
        let mut current: Option<i64> = None;
        let mut refs: Vec<Option<String>> = Vec::new();
        let flush = |id: i64,
                     refs: &[Option<String>],
                     stats: &mut Stats,
                     examples: &mut Vec<(i64, Vec<String>)>| {
            stats.decisions += 1;
            match classify(refs) {
                Verdict::Legit => stats.legit += 1,
                Verdict::Ambiguous => stats.ambiguous += 1,
                Verdict::FalseMerge { groups } => {
                    stats.false_merges += 1;
                    stats.extra_decisions += (groups - 1) as u64;
                    if examples.len() < 25 {
                        let distinct: BTreeSet<String> = refs.iter().flatten().cloned().collect();
                        examples.push((id, distinct.into_iter().collect()));
                    }
                }
            }
        };

        for (id, source_uid, _payload_format, source_fields) in &rows {
            if current != Some(*id) {
                if let Some(prev) = current {
                    flush(prev, &refs, &mut stats, &mut examples);
                }
                current = Some(*id);
                refs.clear();
            }
            refs.push(provenance_canonical_ref(source_fields, source_uid));
        }
        if let Some(prev) = current {
            flush(prev, &refs, &mut stats, &mut examples);
        }

        last_id = max_id;
        tracing::info!(
            decisions = stats.decisions,
            false_merges = stats.false_merges,
            last_id,
            "analyze-false-merges progress"
        );
    }

    println!("\n=== analyze-false-merges (read-only, #29 / ADR 0100) ===");
    println!(
        "décisions multi-provenances tout-judilibre : {}",
        stats.decisions
    );
    println!(
        "  fusion légitime (canonical_ref unique)   : {}",
        stats.legit
    );
    println!(
        "  ambiguë (≥1 provenance sans clé, gardée) : {}",
        stats.ambiguous
    );
    println!(
        "  FAUX MERGE (clés divergentes, à scinder) : {}",
        stats.false_merges
    );
    println!(
        "  → décisions à reconstituer (Σ groupes−1) : {}",
        stats.extra_decisions
    );
    println!("\nÉchantillon de faux merges (decision_id | canonical_ref distincts) :");
    for (id, refs) in &examples {
        println!("  {id} | {}", refs.join("  ¦  "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    #[test]
    fn all_same_key_is_legit() {
        // Cass abrégé + intégrale : même clé → fusion légitime.
        assert_eq!(
            classify(&[s("cc|611|2020-01-15"), s("cc|611|2020-01-15")]),
            Verdict::Legit
        );
    }

    #[test]
    fn distinct_keys_is_false_merge() {
        // Même RG, tribunaux différents (location) → décisions distinctes.
        let v = classify(&[
            s("tj|tj80021|26/00051|2026-01-20"),
            s("tj|tj75011|26/00051|2026-01-20"),
        ]);
        assert_eq!(v, Verdict::FalseMerge { groups: 2 });
    }

    #[test]
    fn three_provenances_two_groups() {
        let v = classify(&[
            s("tj|tj80021|26/00051|2026-01-20"),
            s("tj|tj80021|26/00051|2026-01-20"),
            s("tj|tj75011|26/00051|2026-01-20"),
        ]);
        assert_eq!(v, Verdict::FalseMerge { groups: 2 });
    }

    #[test]
    fn any_none_is_ambiguous_never_split() {
        // Spécificité 100 % : une provenance sans clé → jamais scindée.
        assert_eq!(
            classify(&[s("tj|tj80021|26/00051|2026-01-20"), None]),
            Verdict::Ambiguous
        );
        assert_eq!(classify(&[None, None]), Verdict::Ambiguous);
    }
}
