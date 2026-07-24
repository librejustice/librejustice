-- ADR 0247 — legal_citation : spans en jsonb au grain décision, inverse par
-- GIN d'expression. Élément de spans : tableau positionnel
-- [char_start, char_end, ref_text_uid|null, ref_num_key|null, suivants],
-- ordonné par char_start (offsets codepoints ADR 0143 inchangés).

-- Termes d'inversion : uid (usage d'un texte) et uid|num (usage d'un
-- article), distincts. Appels internes qualifiés public. : PG 18 construit
-- les index sous search_path restreint (pg_catalog, pg_temp).
CREATE FUNCTION lj_cit_terms(jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(DISTINCT term), '{}')
FROM (
    SELECT el->>2 AS term FROM jsonb_array_elements($1) AS el
    WHERE el->>2 IS NOT NULL
    UNION
    SELECT (el->>2) || '|' || (el->>3) FROM jsonb_array_elements($1) AS el
    WHERE el->>3 IS NOT NULL
) t
$fn$;

-- Build-aside + swap : la nouvelle table se construit à côté (lectures de
-- l'ancienne non bloquées pendant l'agrégat et le GIN), la bascule en fin de
-- transaction ne tient le verrou exclusif que quelques secondes. Prérequis
-- opérationnel : aucun écrivain de citations pendant la fenêtre (cron
-- stoppé) — une écriture entre l'agrégat et le COMMIT serait perdue.
CREATE TABLE legal_citation_blob (
    decision_id     bigint   PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    extract_version smallint NOT NULL,
    spans           jsonb    NOT NULL
);

INSERT INTO legal_citation_blob (decision_id, extract_version, spans)
SELECT decision_id,
       max(extract_version),
       jsonb_agg(jsonb_build_array(char_start, char_end, ref_text_uid,
                                   ref_num_key, suivants)
                 ORDER BY char_start)
FROM legal_citation
GROUP BY decision_id;

CREATE INDEX legal_citation_terms_idx
    ON legal_citation_blob USING gin (public.lj_cit_terms(spans));

DROP TABLE legal_citation;
ALTER TABLE legal_citation_blob RENAME TO legal_citation;
ALTER INDEX legal_citation_blob_pkey RENAME TO legal_citation_pkey;
ALTER TABLE legal_citation
    RENAME CONSTRAINT legal_citation_blob_decision_id_fkey
    TO legal_citation_decision_id_fkey;

-- Resync des arrays de facettes (détecteur de dérive) : même forme que la
-- 0151 (expansion « et suivants » via _suivants_family_keys, sorties
-- COLLATE "C"), lue depuis le blob.
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decisions d
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE d.id = agg.decision_id
          AND (
                d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decision_chunks c
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE c.decision_id = agg.decision_id
          AND (
                c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;
