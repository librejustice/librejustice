//! Whole-corpus counters (`corpus_counts`), decisions and legal texts alike.
//! Single writer: `precompute`. Both citation axes come from the index rather
//! than a `GROUP BY` over the arrays; counting at read time is too slow, hence
//! a snapshot. The `seat` dimension carries the whole room axis, one row per
//! unfolded path, its parent being the path minus its last segment: the tree
//! has as many levels as the corpus has.

use super::DecisionRepository;
use crate::error::Result;

/// Everything after the `art_ids` CTE, which alone needs the validity window
/// spliced in. A single statement, so a single snapshot, where the `<>` guard
/// only rewrites the counts that changed.
const REBUILD_TAIL: &str = r#"
    , cit AS (
        SELECT paradedb.aggregate('decision_text_bm25',
                 paradedb.boolean(must => ARRAY[paradedb.term('counted', true),
                                                paradedb.term('kind', 'decision')],
                                  must_not => ARRAY[paradedb.exists('deleted_at')]),
                 '{"inst": {"terms": {"field": "legal_instruments", "size": 30}},
                   "art": {"terms": {"field": "legal_article_composite", "size": 300}}}'::json,
                 bucket_limit => 1000000)::jsonb AS j
    ), cit_inst AS (
        SELECT b->>'key' AS uid, (b->>'doc_count')::bigint AS n
        FROM cit, jsonb_array_elements(j->'inst'->'buckets') b
    ), cit_art AS (
        SELECT split_part(b->>'key', '|', 1) AS uid,
               split_part(b->>'key', '|', 2) AS num_key,
               (b->>'doc_count')::bigint AS n
        FROM cit, jsonb_array_elements(j->'art'->'buckets') b
    ), fresh AS (
            SELECT 'total'::text AS dimension, ''::text AS value,
                   NULL::text AS parent, count(*) AS n
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision'
        UNION ALL
            SELECT 'jurisdiction_type', jurisdiction_type, NULL, count(*)
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision'
            GROUP BY jurisdiction_type
        UNION ALL
            SELECT 'jurisdiction_code', jurisdiction_code, jurisdiction_type, count(*)
            FROM decisions
            WHERE deleted_at IS NULL AND kind = 'decision' AND jurisdiction_code IS NOT NULL
            GROUP BY jurisdiction_code, jurisdiction_type
        UNION ALL
            SELECT 'seat', p, nullif(regexp_replace(p, '\.[^.]+$', ''), p), count(*)
            FROM decisions d, unnest(d.seat_paths) p
            WHERE d.deleted_at IS NULL AND d.kind = 'decision'
            GROUP BY p
        UNION ALL
            SELECT 'formation', p, NULL, count(*)
            FROM decisions d, unnest(d.formation_paths) p
            WHERE d.deleted_at IS NULL AND d.kind = 'decision'
            GROUP BY p
        UNION ALL
            SELECT 'legal_domain', legal_domain_uid, NULL, count(*)
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision' AND legal_domain_uid IS NOT NULL
            GROUP BY legal_domain_uid
        UNION ALL
            SELECT 'solution', solution_uid, NULL, count(*)
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision' AND solution_uid IS NOT NULL
            GROUP BY solution_uid
        UNION ALL
            SELECT 'year', extract(year FROM date_lecture)::int::text, NULL, count(*)
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision' AND date_lecture IS NOT NULL
            GROUP BY 2
        UNION ALL
            SELECT 'publication_codes',
                   array_to_string(publication_codes, ','), NULL, count(*)
            FROM decisions WHERE deleted_at IS NULL AND kind = 'decision' AND publication_codes <> '{}'
            GROUP BY 2
        UNION ALL
            SELECT 'legal_text_total', '', NULL, count(*) FROM legal_text
        UNION ALL
            SELECT 'legal_article_total', '', NULL, count(*) FROM art_ids
        UNION ALL
            SELECT 'legal_instrument', uid, NULL, n FROM cit_inst
        UNION ALL
            SELECT 'legal_article', num_key, uid, n FROM cit_art
            WHERE uid IN (SELECT uid FROM cit_inst)
    ), stale AS (
        DELETE FROM corpus_counts c
        WHERE NOT EXISTS (
            SELECT 1 FROM fresh f
            WHERE f.dimension = c.dimension
              AND f.value = c.value
              AND f.parent IS NOT DISTINCT FROM c.parent)
    )
    INSERT INTO corpus_counts (dimension, value, parent, n)
    SELECT dimension, value, parent, n FROM fresh
    ON CONFLICT (dimension, value, parent) DO UPDATE
    SET n = excluded.n
    WHERE corpus_counts.n <> excluded.n
"#;

impl DecisionRepository<'_> {
    /// The three home-page totals, read in one go. Migration 0275 seeds the two
    /// legal dimensions, so no reader ever meets an empty one.
    #[tracing::instrument(name = "db.corpus_totals", skip(self), fields(db.system = "postgresql"))]
    pub async fn corpus_totals(&self) -> Result<(i64, i64, i64)> {
        let rows = self
            .conn
            .query(
                "SELECT dimension, n FROM corpus_counts
                  WHERE dimension IN ('total', 'legal_text_total', 'legal_article_total')
                    AND value = '' AND parent IS NULL",
                &[],
            )
            .await?;
        let get = |d: &str| {
            rows.iter()
                .find(|r| r.get::<_, &str>(0) == d)
                .map_or(0, |r| r.get::<_, i64>(1))
        };
        Ok((
            get("total"),
            get("legal_text_total"),
            get("legal_article_total"),
        ))
    }

    /// Full rebuild of the snapshot. Returns the number of upserted rows.
    #[tracing::instrument(name = "db.rebuild_corpus_counts", skip(self), fields(db.system = "postgresql"))]
    pub async fn rebuild_corpus_counts(&self) -> Result<u64> {
        // Distinct identities via a `GROUP BY` subquery: `count(DISTINCT …)`
        // sorts a composite ROW and spills to disk.
        let (valid_a, norm_a) = (
            super::referential::article_valid_now("a"),
            super::referential::article_is_norm("a"),
        );
        let n = self
            .conn
            .execute(
                &format!(
                    "WITH art_ids AS (
                        SELECT a.text_uid, a.num_key FROM legal_article a
                         WHERE {valid_a} AND {norm_a} AND a.num <> ''
                         GROUP BY 1, 2){REBUILD_TAIL}"
                ),
                &[],
            )
            .await?;
        Ok(n)
    }
}
