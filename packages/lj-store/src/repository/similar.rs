//! A decision's similar decisions, in two panels: the notable decisions on the
//! same question of law (`principe`) and the decisions on similar facts
//! (`espece`). The legs read the citation graph and the chunk embeddings.

use std::collections::HashSet;

use super::DecisionRepository;
use crate::error::Result;

/// Widening factor of the inner ANN window.
const SIMILAR_INNER_LIMIT_FACTOR: i64 = 24;

/// Overfetch of the espece panel: absorbs the exclusions before the cut to `limit`.
const ESPECE_OVERFETCH_FACTOR: i64 = 3;

/// vchordrq probes of the KNN leg prefiltered on notable decisions.
const PRINCIPE_KNN_PROBES: u32 = 100;

/// Publication codes of a notable decision: MAJEURE and IMPORTANTE.
const NOTABLE_CODES: [&str; 8] = ["r", "A", "b", "l", "c", "B", "C+", "R"];

/// IDF leg: most article terms kept from the source decision.
const IDF_MAX_RARE_TERMS: i64 = 8;

/// IDF leg: past this df, an article is cited too widely to sign the question of law.
const IDF_RARE_DF_CAP: i64 = 100_000;

/// Past this size, a CC rejection citing art. 1014 also answers real grounds.
const GATE_1014_MAX_CHARS: i32 = 3_500;

/// `lj_cit_terms` term of art. 1014 of the code of civil procedure.
const CPC_1014_TERM: &str = "LEGITEXT000006070716|1014";

/// The source decision's metadata the panels' abstention reads.
pub struct SimilarSourceMeta {
    pub jurisdiction_type: String,
    pub solution_uid: Option<String>,
    pub text_chars: i32,
    pub cites_1014: bool,
}

/// The principe panel stays empty: the source settles no question of law, a
/// withdrawal, a no-need-to-rule, or a short CC rejection under art. 1014 CPC.
pub fn similar_principe_gated(meta: &SimilarSourceMeta) -> bool {
    match meta.solution_uid.as_deref() {
        Some("solution:DESISTEMENT") | Some("solution:NON_LIEU_A_STATUER") => true,
        Some("solution:REJET") => {
            meta.jurisdiction_type == "CC"
                && meta.cites_1014
                && meta.text_chars < GATE_1014_MAX_CHARS
        }
        _ => false,
    }
}

/// The espece panel stays empty for a withdrawal: its neighbours are serial
/// formal acknowledgements of no use. A no-need-to-rule keeps its panel.
pub fn similar_espece_gated(meta: &SimilarSourceMeta) -> bool {
    meta.solution_uid.as_deref() == Some("solution:DESISTEMENT")
}

/// The raw legs of both panels, before assembly.
pub struct SimilarLegs {
    /// Resolved targets of the outgoing citations, notable.
    pub direct: Vec<i64>,
    /// Notable decisions citing the same rare articles, IDF rank.
    pub idf: Vec<i64>,
    /// The case's `decision_links` chronology, left out of the espece panel.
    pub chronology: Vec<i64>,
    /// KNN prefiltered on notable decisions.
    pub principe_knn: Vec<i64>,
    /// Generic KNN, overfetched (espece panel, before its exclusions).
    pub espece_knn: Vec<i64>,
}

/// The citation graph's legs of the principe panel, and the case chronology.
pub struct SimilarGraph {
    pub direct: Vec<i64>,
    pub idf: Vec<i64>,
    pub chronology: Vec<i64>,
}

/// Assembles the principe panel: direct citations first, then the hits IDF and
/// KNN agree on, then IDF and KNN interleaved.
pub fn assemble_principe(direct: &[i64], idf: &[i64], knn: &[i64], limit: usize) -> Vec<i64> {
    let knn_set: HashSet<i64> = knn.iter().copied().collect();
    let accord = idf.iter().copied().filter(|id| knn_set.contains(id));

    let mut interleaved = Vec::with_capacity(idf.len() + knn.len());
    let (mut it_idf, mut it_knn) = (idf.iter().copied(), knn.iter().copied());
    loop {
        let (a, b) = (it_idf.next(), it_knn.next());
        if a.is_none() && b.is_none() {
            break;
        }
        interleaved.extend(a);
        interleaved.extend(b);
    }

    let mut seen = HashSet::new();
    direct
        .iter()
        .copied()
        .chain(accord)
        .chain(interleaved)
        .filter(|id| seen.insert(*id))
        .take(limit)
        .collect()
}

/// The two panels of a decision, in display order, internal ids.
pub struct SimilarPanels {
    pub principe: Vec<i64>,
    pub espece: Vec<i64>,
    /// The principe panel abstained under [`similar_principe_gated`].
    pub abstained: bool,
}

impl SimilarPanels {
    /// The panels of `limit` decisions each: the espece panel leaves out the
    /// principe hits and the case chronology.
    pub fn assemble(meta: &SimilarSourceMeta, legs: SimilarLegs, limit: usize) -> Self {
        let abstained = similar_principe_gated(meta);
        let principe = if abstained {
            Vec::new()
        } else {
            assemble_principe(&legs.direct, &legs.idf, &legs.principe_knn, limit)
        };
        let excluded: HashSet<i64> = principe
            .iter()
            .chain(legs.chronology.iter())
            .copied()
            .collect();
        let espece = if similar_espece_gated(meta) {
            Vec::new()
        } else {
            legs.espece_knn
                .into_iter()
                .filter(|id| !excluded.contains(id))
                .take(limit)
                .collect()
        };
        Self {
            principe,
            espece,
            abstained,
        }
    }
}

/// The first and last chunk embeddings of `$1`, the KNN legs' query vectors.
const SOURCE_EMBEDDINGS: &str = "
    WITH source AS (
      SELECT embedding
      FROM (
        SELECT
          c.embedding,
          row_number() OVER (ORDER BY c.chunk_index) AS rn_first,
          row_number() OVER (ORDER BY c.chunk_index DESC) AS rn_last
        FROM decision_chunks c
        WHERE c.decision_id = $1
          AND c.embedding IS NOT NULL
      ) ordered
      WHERE rn_first = 1 OR rn_last = 1
    )";

impl DecisionRepository<'_> {
    /// The decision at `public_id` and the metadata its panels' abstention reads.
    #[tracing::instrument(name = "db.similar_source_meta", skip(self), fields(db.system = "postgresql"))]
    pub async fn similar_source_meta(
        &self,
        public_id: &str,
    ) -> Result<Option<(i64, SimilarSourceMeta)>> {
        let row = self
            .conn
            .query_opt(
                &format!(
                    "SELECT d.id, d.jurisdiction_type, d.solution_uid,
                            coalesce(length({text}), 0),
                            EXISTS (
                              SELECT 1 FROM legal_citation lc
                              WHERE lc.decision_id = d.id
                                AND public.lj_cit_terms(lc.spans) @> ARRAY[$2::text]
                            )
                     FROM decisions d WHERE d.public_id = $1 AND d.deleted_at IS NULL",
                    text = super::pick::text("p.full_text", "d")
                ),
                &[&public_id, &CPC_1014_TERM],
            )
            .await?;
        Ok(row.map(|r| {
            (
                r.get(0),
                SimilarSourceMeta {
                    jurisdiction_type: r.get(1),
                    solution_uid: r.get(2),
                    text_chars: r.get(3),
                    cites_1014: r.get(4),
                },
            )
        }))
    }

    /// The espece leg: generic top-k ANN over the edge chunks' embeddings (first
    /// and last, max-pooled per decision), overfetched to absorb the exclusions,
    /// under `probes`.
    #[tracing::instrument(name = "db.similar_espece_knn", skip(self), fields(db.system = "postgresql"))]
    pub async fn similar_espece_knn(
        &self,
        source_id: i64,
        limit: usize,
        probes: u32,
    ) -> Result<Vec<i64>> {
        let outer_limit = (limit as i64) * ESPECE_OVERFETCH_FACTOR;
        let inner_limit = (limit as i64) * SIMILAR_INNER_LIMIT_FACTOR;
        let query = format!(
            "{SOURCE_EMBEDDINGS},
            neighbors AS MATERIALIZED (
              SELECT
                cand.decision_id,
                1 - cand.distance AS score
              FROM source src
              JOIN LATERAL (
                SELECT
                  c.decision_id,
                  c.embedding <=> src.embedding AS distance
                FROM decision_chunks c
                WHERE c.embedding IS NOT NULL
                  AND c.decision_id <> $1
                ORDER BY distance
                LIMIT {inner_limit}
              ) cand ON TRUE
            )
            SELECT n.decision_id
            FROM neighbors n
            GROUP BY n.decision_id
            ORDER BY MAX(n.score) DESC, n.decision_id
            LIMIT {outer_limit}"
        );
        self.similar_knn(&query, source_id, probes).await
    }

    /// The principe KNN leg: the same ANN, prefiltered on notable decisions
    /// inside the scan; the join filter keeps vchordrq walking in distance order
    /// until the LIMIT fills.
    #[tracing::instrument(name = "db.similar_principe_knn", skip(self), fields(db.system = "postgresql"))]
    pub async fn similar_principe_knn(&self, source_id: i64, limit: usize) -> Result<Vec<i64>> {
        let limit = limit as i64;
        let query = format!(
            "{SOURCE_EMBEDDINGS},
            neighbors AS MATERIALIZED (
              SELECT
                cand.decision_id,
                1 - cand.distance AS score
              FROM source src
              JOIN LATERAL (
                SELECT
                  c.decision_id,
                  c.embedding <=> src.embedding AS distance
                FROM decision_chunks c
                JOIN decisions dn ON dn.id = c.decision_id
                WHERE c.embedding IS NOT NULL
                  AND c.decision_id <> $1
                  AND dn.publication_codes && '{{{notable}}}'::text[]
                ORDER BY distance
                LIMIT {limit}
              ) cand ON TRUE
            )
            SELECT n.decision_id
            FROM neighbors n
            GROUP BY n.decision_id
            ORDER BY MAX(n.score) DESC, n.decision_id
            LIMIT {limit}",
            notable = NOTABLE_CODES.join(","),
        );
        self.similar_knn(&query, source_id, PRINCIPE_KNN_PROBES)
            .await
    }

    /// An ANN query (`$1` the source decision, one `decision_id` column) under
    /// `SET LOCAL vchordrq.probes`.
    async fn similar_knn(&self, query: &str, source_id: i64, probes: u32) -> Result<Vec<i64>> {
        self.conn
            .batch_execute(&format!("BEGIN; SET LOCAL vchordrq.probes = {probes}"))
            .await?;
        let rows = self.conn.query(query, &[&source_id]).await;
        match &rows {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        Ok(rows?.iter().map(|r| r.get(0)).collect())
    }

    /// The citation graph's legs of the principe panel, and the case chronology
    /// the espece panel leaves out.
    #[tracing::instrument(name = "db.similar_graph", skip(self), fields(db.system = "postgresql"))]
    pub async fn similar_graph(&self, source_id: i64, limit: usize) -> Result<SimilarGraph> {
        let limit = limit as i64;
        let notable: Vec<&str> = NOTABLE_CODES.to_vec();
        let majeure = vec!["r", "A"];

        let direct_rows = self
            .conn
            .query(
                "
                SELECT t.id
                FROM case_citation cc
                JOIN decisions t ON t.id = cc.target_decision_id
                WHERE cc.decision_id = $1
                  AND t.publication_codes && $2
                GROUP BY t.id
                ORDER BY bool_or(t.publication_codes && $3) DESC,
                         MAX(t.date_lecture) DESC NULLS LAST, t.id
                LIMIT $4
                ",
                &[&source_id, &notable, &majeure, &limit],
            )
            .await?;

        // Shared rare articles: the source's article terms (`text_uid|num_key`), kept
        // under the rarity cap, inverted by the GIN `legal_citation_terms_idx` onto the
        // notable decisions, scored Σ ln(N/df) with N the planner's estimate.
        // Codes inlined, never bound: `idx_decisions_notable` is partial, and from a
        // parameter the planner cannot prove the implication on a generic plan.
        let notable_sql = NOTABLE_CODES.map(|code| format!("'{code}'")).join(", "); // sql-splice-ok: constant codes
        let idf_query = format!(
            "
            WITH terms AS (
              SELECT DISTINCT t
              FROM legal_citation lc, unnest(public.lj_cit_terms(lc.spans)) AS t
              WHERE lc.decision_id = $1 AND position('|' in t) > 0
            ),
            rare AS (
              SELECT t.t, s.decision_count
              FROM terms t
              JOIN citing_decision_counts s ON s.cited_term = t.t
              WHERE s.decision_count < {IDF_RARE_DF_CAP}
              ORDER BY s.decision_count ASC
              LIMIT {IDF_MAX_RARE_TERMS}
            )
            SELECT lc2.decision_id
            FROM rare r
            JOIN legal_citation lc2 ON public.lj_cit_terms(lc2.spans) @> ARRAY[r.t]
            JOIN decisions d2 ON d2.id = lc2.decision_id
            WHERE d2.publication_codes && ARRAY[{notable_sql}]::text[]
              AND lc2.decision_id <> $1
            GROUP BY lc2.decision_id
            ORDER BY sum(ln(
                         (SELECT greatest(reltuples, 1)::float8 FROM pg_class
                          WHERE oid = 'decisions'::regclass)
                         / r.decision_count::float8
                     )) DESC,
                     lc2.decision_id
            LIMIT $2
            ",
        );
        let idf_rows = self.conn.query(&idf_query, &[&source_id, &limit]).await?;

        let chrono_rows = self
            .conn
            .query(
                "
                WITH RECURSIVE chain(id, depth) AS (
                    SELECT $1::bigint, 0
                    UNION
                    SELECT CASE WHEN dl.decision_id = c.id THEN dl.target_decision_id
                                ELSE dl.decision_id END,
                           c.depth + 1
                    FROM chain c
                    JOIN decision_links dl
                      ON dl.target_decision_id IS NOT NULL
                     AND (dl.decision_id = c.id OR dl.target_decision_id = c.id)
                    WHERE c.depth < 8
                )
                SELECT DISTINCT id FROM chain WHERE id <> $1
                ",
                &[&source_id],
            )
            .await?;

        Ok(SimilarGraph {
            direct: direct_rows.iter().map(|r| r.get(0)).collect(),
            idf: idf_rows.iter().map(|r| r.get(0)).collect(),
            chronology: chrono_rows.iter().map(|r| r.get(0)).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_principe_direct_then_accord_then_interleaved() {
        let out = assemble_principe(&[1], &[2, 3, 4], &[3, 5, 6], 6);
        assert_eq!(out, vec![1, 3, 2, 5, 4, 6]);
    }

    #[test]
    fn assemble_principe_caps_and_dedups_direct() {
        let out = assemble_principe(&[2], &[2, 3], &[4, 5], 3);
        assert_eq!(out, vec![2, 4, 3]);
    }

    #[test]
    fn assemble_principe_empty_legs() {
        assert_eq!(assemble_principe(&[], &[], &[7, 8], 4), vec![7, 8]);
        assert!(assemble_principe(&[], &[], &[], 4).is_empty());
    }

    fn gate_meta(
        jt: &str,
        solution: Option<&str>,
        chars: i32,
        cites_1014: bool,
    ) -> SimilarSourceMeta {
        SimilarSourceMeta {
            jurisdiction_type: jt.to_string(),
            solution_uid: solution.map(str::to_string),
            text_chars: chars,
            cites_1014,
        }
    }

    #[test]
    fn gate_withdrawal_cuts_both_panels() {
        let m = gate_meta("TA", Some("solution:DESISTEMENT"), 2_500, false);
        assert!(similar_principe_gated(&m));
        assert!(similar_espece_gated(&m));
    }

    #[test]
    fn gate_no_need_to_rule_cuts_principe_only() {
        let m = gate_meta("TA", Some("solution:NON_LIEU_A_STATUER"), 2_500, false);
        assert!(similar_principe_gated(&m));
        assert!(!similar_espece_gated(&m));
    }

    #[test]
    fn gate_short_cc_1014_rejection_only() {
        assert!(similar_principe_gated(&gate_meta(
            "CC",
            Some("solution:REJET"),
            2_400,
            true
        )));
        assert!(!similar_principe_gated(&gate_meta(
            "CC",
            Some("solution:REJET"),
            8_000,
            true
        )));
        assert!(!similar_principe_gated(&gate_meta(
            "TA",
            Some("solution:REJET"),
            2_000,
            false
        )));
        assert!(!similar_principe_gated(&gate_meta(
            "CE", None, 2_000, false
        )));
    }

    #[test]
    fn panels_leave_principe_and_chronology_out_of_espece() {
        let legs = SimilarLegs {
            direct: vec![10],
            idf: vec![11],
            chronology: vec![20],
            principe_knn: vec![12],
            espece_knn: vec![10, 20, 30, 31, 32, 33, 34],
        };
        let p = SimilarPanels::assemble(&gate_meta("CE", None, 9_000, false), legs, 4);
        assert_eq!(p.principe, vec![10, 11, 12]);
        assert_eq!(p.espece, vec![30, 31, 32, 33]);
        assert!(!p.abstained);
    }
}
