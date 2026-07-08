-- Migration 0098 — facettes re-sourcées sur la relation du domaine (ADR 0145, M4).
--
-- Le token de filtre/facette passe de `text_key` / `text_key || '|' || article_key`
-- (0076, découplé de la résolution) à `ref_text_uid` / `ref_text_uid || '|' ||
-- ref_num_key`, agrégé depuis `legal_citation`. Une seule monnaie : filtres,
-- backlinks et overlay parlent en références catalogue ; les libellés
-- (uid → titre) sont résolus à la lecture via `legal_text`.
--
-- Assumé : le non-lié (`ref_text_uid` NULL) n'est plus facettable, et une
-- amélioration du linker réécrit les arrays des décisions changées (garde
-- `IS DISTINCT FROM`, marginal en régime établi).
--
-- DDL seulement : le RECOMPUTE corpus-entier est porté par la CLI
-- `resync-legal-arrays` (lots autocommit), à lancer après le déploiement.

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
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
    )
    UPDATE decisions d
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE d.id = agg.decision_id
      AND (
            d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
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
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;
