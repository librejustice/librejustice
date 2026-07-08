//! Réparation des faux merges judilibre (#29 / ADR 0100 §5).
//!
//! La dédup historique (clé 0098 `identity_key` sans `location`) a fusionné des
//! décisions **distinctes** partageant un RG. `fuse_cluster` a conservé les
//! `decision_sources` des membres supprimés (re-pointés vers le canonique) : une
//! décision faussement fusionnée porte donc ≥2 provenances actives dont les
//! `canonical_ref` (ADR 0100) **divergent**. Le `full_text`/chunks/embeddings des
//! membres supprimés sont perdus → il faut **re-fetch** + ré-ingérer.
//!
//! Cette passe :
//! 1. recalcule `canonical_ref` par provenance (`Decision::from_source_fields`,
//!    full_text vide — les discriminants vivent dans les champs structurés) ;
//! 2. **planifie** le scinde (logique pure, testée) : groupe par `canonical_ref`,
//!    désigne un groupe **gardé** (clé minimale, déterministe) qui réutilise la
//!    ligne canonique, et les groupes divergents à reconstituer ; tout `None` ⇒
//!    **ambigu**, jamais scindé (spécificité 100 %, #12) ;
//! 3. en `--execute` (write, gaté) : crée une décision séparée par groupe
//!    divergent (re-pointe ses provenances), puis **re-fetch chaque groupe — gardé
//!    inclus** — et ré-ingère (re-chunk + re-embed ciblé vLLM local). Re-fetcher le
//!    groupe gardé rend l'élection sans effet sur la correction : chaque décision
//!    récupère son **propre** contenu depuis la source, jamais celui d'un voisin
//!    (la ligne `decisions` ne porte ni `source_uid` ni `location` permettant de
//!    certifier à qui appartient son contenu — on ne devine donc pas, on re-fetch).
//!
//! `--dry-run` (défaut) : émet le PLAN exact sans **aucune** écriture.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use lj_core::decision::Decision;
use lj_llm::backend::AnyEmbedder;
use lj_sources::judilibre::JudilibreClient;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;

use super::embed::build_vllm_strict;
use super::generate_public_id;
use super::opendata::refetch_into;
use crate::config::Settings;

/// Une provenance d'un cluster faux-merge : son `source_uid` (porte l'ObjectId
/// Judilibre pour le re-fetch), sa `canonical_ref` recalculée (`None` =
/// non-routée / discriminants manquants) et son `juridiction_type` (pour
/// l'INSERT de la décision scindée — réécrit ensuite par le re-fetch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub source_uid: String,
    pub canonical_ref: Option<String>,
    pub juridiction_type: Option<String>,
}

/// Un groupe homogène (même `canonical_ref`) d'un cluster : sa `canonical_ref`, le
/// `juridiction_type` de sa 1ʳᵉ provenance et ses `source_uid` à re-fetch. Pour un
/// groupe divergent, ils sont déplacés vers une nouvelle décision ; pour le groupe
/// gardé, ils restent sur la ligne canonique (re-fetchée en place).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitGroup {
    pub canonical_ref: String,
    pub juridiction_type: Option<String>,
    pub source_uids: Vec<String>,
}

/// Plan de réparation d'une décision multi-provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResplitPlan {
    /// Toutes les provenances portent la **même** `canonical_ref` → fusion
    /// légitime (Cass abrégé+intégrale…) : rien à scinder.
    Legit,
    /// ≥1 provenance sans `canonical_ref` (`None`) → on ne peut pas certifier la
    /// séparation : **jamais scindée** (#12, spécificité 100 %).
    Ambiguous,
    /// Cluster déjà réparé (≤1 provenance) → SKIP idempotent (un re-run ne
    /// retombe pas dessus, car le cluster n'est plus multi-provenance).
    AlreadySplit,
    /// Faux merge : `keep` = groupe **gardé** (réutilise la ligne canonique, clé
    /// minimale pour le déterminisme) ; `splits` = les groupes divergents à
    /// reconstituer (ordre déterministe par `canonical_ref`). Tous les groupes,
    /// gardé inclus, sont re-fetchés à l'exécution.
    FalseMerge {
        keep: SplitGroup,
        splits: Vec<SplitGroup>,
    },
}

/// Planifie la réparation d'une décision faux-merge à partir de ses provenances.
/// Fonction **pure** (rule #8 : logique métier à cas limites). Groupe par
/// `canonical_ref` (BTreeMap → ordre déterministe), désigne le groupe de clé
/// **minimale** comme gardé (réutilise la ligne canonique). Le contenu de chaque
/// groupe — gardé inclus — étant re-fetché à l'exécution, ce choix n'influe que sur
/// quelle ligne `decisions` est réutilisée, jamais sur la correction du contenu.
///
/// - 0 ou 1 provenance → `AlreadySplit` (rien à faire / déjà scindé).
/// - une provenance sans clé (`None`) → `Ambiguous` (jamais scindée).
/// - une seule `canonical_ref` distincte → `Legit`.
/// - ≥2 clés distinctes → `FalseMerge { keep, splits }`.
pub(crate) fn plan_resplit(provenances: &[Provenance]) -> ResplitPlan {
    if provenances.len() <= 1 {
        return ResplitPlan::AlreadySplit;
    }
    if provenances.iter().any(|p| p.canonical_ref.is_none()) {
        return ResplitPlan::Ambiguous;
    }

    // Groupe les provenances par canonical_ref (déterministe : BTreeMap trié).
    // `jur_type` retient le type de la 1ʳᵉ provenance du groupe.
    let mut by_ref: BTreeMap<String, (Option<String>, Vec<String>)> = BTreeMap::new();
    for p in provenances {
        let key = p.canonical_ref.clone().expect("none écarté ci-dessus");
        let entry = by_ref
            .entry(key)
            .or_insert((p.juridiction_type.clone(), vec![]));
        entry.1.push(p.source_uid.clone());
    }
    if by_ref.len() == 1 {
        return ResplitPlan::Legit;
    }

    // Clé minimale = groupe gardé (réutilise la ligne canonique) ; le reste scinde.
    let mut groups =
        by_ref.into_iter().map(
            |(canonical_ref, (juridiction_type, source_uids))| SplitGroup {
                canonical_ref,
                juridiction_type,
                source_uids,
            },
        );
    let keep = groups
        .next()
        .expect(">=2 groupes (len==1 écarté ci-dessus)");
    let splits: Vec<SplitGroup> = groups.collect();
    ResplitPlan::FalseMerge { keep, splits }
}

/// `canonical_ref` (ADR 0100) d'une provenance judilibre, recalculée depuis
/// `(source_fields, source_uid)` avec full_text vide. `None` si non-routée ou si
/// les briques fiables manquent.
pub(super) fn provenance_canonical_ref(source_fields: &Value, source_uid: &str) -> Option<String> {
    let decision = Decision::from_source_fields("", source_fields, source_uid);
    lj_extract::extract::routed(&decision).ok()?;
    lj_extract::identity::decision_canonical_ref(&decision)
}

/// `juridiction_type` d'une provenance (pour l'INSERT de la décision scindée).
pub(super) fn provenance_juridiction_type(
    source_fields: &Value,
    source_uid: &str,
) -> Option<String> {
    Decision::from_source_fields("", source_fields, source_uid).juridiction_type
}

/// Agrégats du run.
#[derive(Default)]
struct Stats {
    clusters_seen: u64,
    legit: u64,
    ambiguous: u64,
    already_split: u64,
    repairable_clusters: u64,
    decisions_to_reconstruct: u64,
    source_uids_to_refetch: u64,
    refetch_failed: u64,
    split_decisions_created: u64,
}

/// Réparation des faux merges judilibre (#29 / ADR 0100 §5).
///
/// `dry_run` (défaut côté CLI) : émet le PLAN exact, **aucune écriture** (ni DB,
/// ni réseau, ni GPU) — `client` est ignoré.
/// `execute = !dry_run` : crée les décisions scindées, re-fetch via `client` +
/// ré-ingère leur contenu (re-chunk + re-embed ciblé vLLM local, jamais
/// Cloudflare). SAVEPOINT par cluster, reprise idempotente. **À NE PAS lancer
/// pendant l'ingest JADE** (écritures concurrentes + contention GPU vLLM).
pub async fn resplit_false_merges(
    client: Option<&JudilibreClient>,
    dry_run: bool,
    audit_sample: usize,
    limit: Option<usize>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Embedder seulement en mode execute (le dry-run ne touche ni réseau ni GPU).
    // Re-embed ciblé via `build_vllm_strict` : vLLM local uniquement, **jamais**
    // Cloudflare (coût ; ADR 0100 §5) — erreur franche si vLLM injoignable, pas de
    // repli payant (règle projet, #12).
    let embedder = if dry_run {
        None
    } else {
        if client.is_none() {
            return Err(anyhow!("mode --execute requiert un client Judilibre"));
        }
        Some(build_vllm_strict(&settings).await?)
    };

    let mut last_id: i64 = 0;
    let mut stats = Stats::default();
    let mut examples: Vec<String> = Vec::new();
    const BATCH: i64 = 2000;

    'batches: loop {
        let rows = repo
            .fetch_judilibre_multiprovenance_batch(last_id, BATCH)
            .await?;
        let Some(&max_id) = rows.iter().map(|(id, ..)| id).max() else {
            break;
        };

        // Regroupe les provenances par décision (rows triées par decision_id).
        let mut by_decision: BTreeMap<i64, Vec<Provenance>> = BTreeMap::new();
        for (id, source_uid, _payload_format, source_fields) in &rows {
            by_decision.entry(*id).or_default().push(Provenance {
                source_uid: source_uid.clone(),
                canonical_ref: provenance_canonical_ref(source_fields, source_uid),
                juridiction_type: provenance_juridiction_type(source_fields, source_uid),
            });
        }

        for (canonical_id, provenances) in by_decision {
            stats.clusters_seen += 1;
            match plan_resplit(&provenances) {
                ResplitPlan::Legit => stats.legit += 1,
                ResplitPlan::Ambiguous => stats.ambiguous += 1,
                ResplitPlan::AlreadySplit => stats.already_split += 1,
                ResplitPlan::FalseMerge { keep, splits } => {
                    stats.repairable_clusters += 1;
                    stats.decisions_to_reconstruct += splits.len() as u64;
                    // Tous les groupes sont re-fetchés (gardé inclus) → l'élection
                    // n'écrase jamais : chaque décision récupère son propre contenu.
                    stats.source_uids_to_refetch += keep.source_uids.len() as u64
                        + splits
                            .iter()
                            .map(|g| g.source_uids.len() as u64)
                            .sum::<u64>();

                    if examples.len() < audit_sample {
                        let split_refs: Vec<String> = splits
                            .iter()
                            .map(|g| format!("{} [{}]", g.canonical_ref, g.source_uids.join(",")))
                            .collect();
                        examples.push(format!(
                            "  decision {canonical_id} | garde: {} [{}] | scinde: {}",
                            keep.canonical_ref,
                            keep.source_uids.join(","),
                            split_refs.join("  ¦  ")
                        ));
                    }

                    if !dry_run {
                        let client = client.expect("client en mode execute");
                        let embedder = embedder.as_ref().expect("embedder en mode execute");
                        execute_resplit(
                            &conn,
                            canonical_id,
                            &keep,
                            &splits,
                            client,
                            embedder,
                            &mut stats,
                        )
                        .await?;
                        // Rollout prudent : stop après N clusters réparés (write).
                        if limit.is_some_and(|l| stats.repairable_clusters as usize >= l) {
                            tracing::info!(
                                limit = ?limit,
                                repaired = stats.repairable_clusters,
                                "resplit-false-merges : borne --limit atteinte, arrêt"
                            );
                            break 'batches;
                        }
                    }
                }
            }
        }

        last_id = max_id;
        tracing::info!(
            clusters = stats.clusters_seen,
            repairable = stats.repairable_clusters,
            last_id,
            "resplit-false-merges progress"
        );
    }

    let mode = if dry_run {
        "DRY-RUN (read-only)"
    } else {
        "EXECUTE (write)"
    };
    println!("\n=== resplit-false-merges [{mode}] (#29 / ADR 0100 §5) ===");
    println!(
        "clusters multi-provenances vus           : {}",
        stats.clusters_seen
    );
    println!("  fusion légitime (clé unique)           : {}", stats.legit);
    println!(
        "  ambigu (≥1 clé None, gardé)            : {}",
        stats.ambiguous
    );
    println!(
        "  déjà scindé (≤1 provenance, SKIP)      : {}",
        stats.already_split
    );
    println!(
        "  FAUX MERGE réparable                   : {}",
        stats.repairable_clusters
    );
    println!(
        "  → décisions à reconstituer             : {}",
        stats.decisions_to_reconstruct
    );
    println!(
        "  → source_uids à re-fetch               : {}",
        stats.source_uids_to_refetch
    );
    if !dry_run {
        println!(
            "  décisions scindées créées              : {}",
            stats.split_decisions_created
        );
        println!(
            "  re-fetch échoués (groupe sauté)        : {}",
            stats.refetch_failed
        );
    }
    println!("\nÉchantillon de plan (canonical_id | garde | scinde) :");
    for line in &examples {
        println!("{line}");
    }
    Ok(())
}

/// Exécute le scinde d'un cluster (write) **dans sa propre transaction** : crée
/// chaque décision divergente, re-pointe ses provenances, re-fetch + ré-ingère son
/// contenu (re-chunk + re-embed ciblé), **puis re-fetch le groupe gardé en place**
/// sur la ligne canonique (l'élection devient sans effet sur la correction). Tout
/// le cluster commit ou rollback en bloc — un échec re-fetch d'un groupe (décision
/// disparue côté Judilibre) ne casse PAS le cluster : groupe scindé laissé vide /
/// gardé laissé tel quel (compté en `refetch_failed`). Le compteur
/// `split_decisions_created` est validé **après** le COMMIT (jamais avant un
/// rollback potentiel).
async fn execute_resplit(
    conn: &Connection,
    canonical_id: i64,
    keep: &SplitGroup,
    splits: &[SplitGroup],
    client: &JudilibreClient,
    embedder: &AnyEmbedder,
    stats: &mut Stats,
) -> Result<()> {
    let repo = DecisionRepository::new(conn);
    conn.batch_execute("BEGIN").await?;
    let outcome: Result<(u64, u64)> = async {
        let mut created = 0u64;
        let mut failed = 0u64;
        for group in splits {
            // ObjectIds Judilibre = part après `judilibre/`.
            let object_ids: Vec<String> = group
                .source_uids
                .iter()
                .filter_map(|u| u.strip_prefix("judilibre/").map(str::to_string))
                .collect();
            if object_ids.is_empty() {
                tracing::warn!(?group, "groupe sans ObjectId judilibre, skip");
                continue;
            }

            // Décision squelette : identité (canonical_ref) + juridiction_type du
            // groupe posés ici ; le contenu (full_text/chunks/embeddings) et l'ECLI
            // sont écrits par le re-fetch (write_canonical_content, autorité
            // triviale de la nouvelle décision).
            let jur_type = group.juridiction_type.as_deref().unwrap_or("tj");
            let public_id = generate_public_id();
            let new_id = repo
                .create_split_decision(
                    canonical_id,
                    &group.source_uids,
                    jur_type,
                    &public_id,
                    &group.canonical_ref,
                )
                .await?;
            created += 1;

            // Re-fetch + ré-ingest ciblé : le source_uid pointe désormais new_id →
            // le contenu y atterrit (drain_batch mode All). Échec (décision
            // disparue côté Judilibre) → groupe conservé vide, pas de rollback.
            match refetch_into(conn, client, embedder, &object_ids).await {
                Ok(()) => {}
                Err(e) => {
                    failed += 1;
                    tracing::warn!(new_id, ?object_ids, error = %e, "re-fetch échoué, groupe conservé vide");
                }
            }
        }

        // Groupe gardé : ses source_uids pointent déjà vers `canonical_id` → le
        // re-fetch ré-ingère son contenu **en place** (full_text/chunks/embeddings
        // + identité réécrits par l'update). On ne suppose donc PAS que le canonique
        // sert déjà ce contenu — on le récupère depuis la source, ce qui neutralise
        // toute mauvaise élection. Échec (décision disparue côté Judilibre) →
        // contenu existant conservé, compté `refetch_failed` (audit manuel : le
        // cluster devient mono-identité et ne sera pas re-tenté).
        let keep_object_ids: Vec<String> = keep
            .source_uids
            .iter()
            .filter_map(|u| u.strip_prefix("judilibre/").map(str::to_string))
            .collect();
        if keep_object_ids.is_empty() {
            tracing::warn!(canonical_id, ?keep, "groupe gardé sans ObjectId judilibre");
        } else if let Err(e) = refetch_into(conn, client, embedder, &keep_object_ids).await {
            failed += 1;
            tracing::warn!(canonical_id, ?keep_object_ids, error = %e, "re-fetch groupe gardé échoué, contenu conservé");
        }

        Ok((created, failed))
    }
    .await;

    match outcome {
        Ok((created, failed)) => {
            conn.batch_execute("COMMIT").await?;
            stats.split_decisions_created += created;
            stats.refetch_failed += failed;
            Ok(())
        }
        Err(e) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            tracing::error!(canonical_id, error = %e, "cluster rollback");
            Err(e).context("execute_resplit cluster")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(uid: &str, key: Option<&str>) -> Provenance {
        Provenance {
            source_uid: uid.to_string(),
            canonical_ref: key.map(str::to_string),
            juridiction_type: Some("tj".to_string()),
        }
    }

    #[test]
    fn two_groups_keep_is_min_key() {
        // Groupe gardé = clé minimale (déterministe) : tj75011 garde, tj80021 scinde.
        // L'élection n'écrase rien : les deux groupes sont re-fetchés à l'exécution.
        let plan = plan_resplit(&[
            p("judilibre/a", Some("tj|tj80021|26/00051|2026-01-20")),
            p("judilibre/b", Some("tj|tj75011|26/00051|2026-01-20")),
        ]);
        assert_eq!(
            plan,
            ResplitPlan::FalseMerge {
                keep: SplitGroup {
                    canonical_ref: "tj|tj75011|26/00051|2026-01-20".to_string(),
                    juridiction_type: Some("tj".to_string()),
                    source_uids: vec!["judilibre/b".to_string()],
                },
                splits: vec![SplitGroup {
                    canonical_ref: "tj|tj80021|26/00051|2026-01-20".to_string(),
                    juridiction_type: Some("tj".to_string()),
                    source_uids: vec!["judilibre/a".to_string()],
                }],
            }
        );
    }

    #[test]
    fn three_provenances_two_groups() {
        // 2 provenances partagent une clé, 1 diverge → 1 seul split. Clé minimale
        // (tj75011) gardée, l'autre (tj80021, 2 provenances) scindée.
        let plan = plan_resplit(&[
            p("judilibre/a", Some("tj|tj80021|26/00051|2026-01-20")),
            p("judilibre/b", Some("tj|tj80021|26/00051|2026-01-20")),
            p("judilibre/c", Some("tj|tj75011|26/00051|2026-01-20")),
        ]);
        let ResplitPlan::FalseMerge { keep, splits } = plan else {
            panic!("attendu FalseMerge");
        };
        assert_eq!(keep.canonical_ref, "tj|tj75011|26/00051|2026-01-20");
        assert_eq!(keep.source_uids, vec!["judilibre/c".to_string()]);
        assert_eq!(splits.len(), 1);
        assert_eq!(
            splits[0].source_uids,
            vec!["judilibre/a".to_string(), "judilibre/b".to_string()]
        );
    }

    #[test]
    fn three_distinct_groups() {
        // 3 clés distinctes → garde la minimale (nancy), scinde 2 (ordre déterministe).
        let plan = plan_resplit(&[
            p("judilibre/a", Some("ca|ca paris|21 00002|2021-03-01")),
            p("judilibre/b", Some("ca|ca nancy|21 00002|2021-03-01")),
            p("judilibre/c", Some("ca|ca riom|21 00002|2021-03-01")),
        ]);
        let ResplitPlan::FalseMerge { keep, splits } = plan else {
            panic!("attendu FalseMerge");
        };
        assert_eq!(keep.canonical_ref, "ca|ca nancy|21 00002|2021-03-01");
        assert_eq!(keep.source_uids, vec!["judilibre/b".to_string()]);
        assert_eq!(splits.len(), 2);
        // Tri par canonical_ref : paris avant riom.
        assert_eq!(splits[0].canonical_ref, "ca|ca paris|21 00002|2021-03-01");
        assert_eq!(splits[1].canonical_ref, "ca|ca riom|21 00002|2021-03-01");
    }

    #[test]
    fn ambiguous_none_never_split() {
        // Spécificité 100 % : une provenance sans clé → jamais scindée.
        assert_eq!(
            plan_resplit(&[
                p("judilibre/a", Some("tj|tj80021|26/00051|2026-01-20")),
                p("judilibre/b", None),
            ]),
            ResplitPlan::Ambiguous
        );
        assert_eq!(
            plan_resplit(&[p("judilibre/a", None), p("judilibre/b", None)]),
            ResplitPlan::Ambiguous
        );
    }

    #[test]
    fn all_same_key_is_legit() {
        // Cass abrégé + intégrale : même clé → fusion légitime, pas de scinde.
        assert_eq!(
            plan_resplit(&[
                p("judilibre/a", Some("cc|611|2020-01-15")),
                p("judilibre/b", Some("cc|611|2020-01-15")),
            ]),
            ResplitPlan::Legit
        );
    }

    #[test]
    fn already_split_is_idempotent() {
        // Cluster déjà réparé (≤1 provenance restante) → SKIP : un re-run ne
        // re-scinde pas (réversibilité / idempotence, ADR 0100 §5).
        assert_eq!(
            plan_resplit(&[p("judilibre/a", Some("tj|tj80021|26/00051|2026-01-20"))]),
            ResplitPlan::AlreadySplit
        );
        assert_eq!(plan_resplit(&[]), ResplitPlan::AlreadySplit);
    }
}
