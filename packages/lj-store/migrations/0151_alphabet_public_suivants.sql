-- 0151 — alphabet public partout (audit du 2026-07-18). Le stock
-- `legal_citation.ref_num_key` est en clés PUBLIQUES (ADR 0209) : 18,1 M de
-- lignes `l…`, 14,7 M numériques ; la seule poche citable (« L. 3213-1 »)
-- était la couche gold v1000, repliée au chargement depuis ce commit
-- (gt_load → article_key). L'enveloppe citable `_suivants_family` (0149)
-- ne reconnaissait donc AUCUNE ligne live préfixée : l'expansion facettes
-- des « et suivants » était un no-op silencieux hors numérique. Une seule
-- forme, une seule fonction : les resync consomment `_suivants_family_keys`
-- directement.

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family_keys(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
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
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family_keys(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
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

DROP FUNCTION _suivants_family(text, text);
