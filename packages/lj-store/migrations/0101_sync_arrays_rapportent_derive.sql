-- Migration 0101 — les fonctions de sync des arrays de facettes rapportent la
-- dérive (ADR 0147).
--
-- Même recompute que 0098 (agrégat depuis `legal_citation`, garde
-- `IS DISTINCT FROM`), mais RETURNS bigint = nombre de lignes corrigées.
-- Consommé par le filet hebdomadaire `resync-legal-arrays` post-passe
-- intégrale : en régime nominal l'écrivain (write → sync atomiques) laisse
-- zéro dérive — un compte non nul EST le signal d'un bug d'écrivain (règle
-- #12 : signaler, pas réparer en silence).

DROP FUNCTION _sync_decisions_legal_instruments_for(bigint[]);
DROP FUNCTION _sync_chunks_legal_instruments_for(bigint[]);

CREATE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
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
$$ LANGUAGE sql;

CREATE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.ref_text_uid ORDER BY lc.ref_text_uid)
                FILTER (WHERE lc.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT lc.ref_text_uid || '|' || lc.ref_num_key
                      ORDER BY lc.ref_text_uid || '|' || lc.ref_num_key)
                FILTER (WHERE lc.ref_num_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN legal_citation lc ON lc.decision_id = t.id
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
$$ LANGUAGE sql;
