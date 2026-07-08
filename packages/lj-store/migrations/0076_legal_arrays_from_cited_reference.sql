-- Migration 0076 — arrays légaux dénormalisés alimentés par cited_reference
-- (ADR 0112 phase B, P5 cutover). Bascule la SOURCE des fast fields/GIN de filtre
-- (`decisions.legal_instruments` / `legal_article_composite` et leurs homonymes
-- chunk) du 3NF (legal_codes/legal_articles/decision_legal_references) vers le
-- modèle unifié (decision_citation → cited_reference).
--
-- Token de filtre/facette = `text_key` (instrument canonique) et
-- `text_key || '|' || article_key` (composite). `text_key` = la forme que posait
-- déjà le snap pour les instruments catalogués (ex. « Code de procédure civile »),
-- d'où 93,4 % d'arrays identiques au 3NF ; les écarts = queue non cataloguée à
-- titre long (le snap fusionnait par fréquence, P5 garde le titre normalisé). La
-- RÉSOLUTION (ref_text_uid/ref_num_key) ne touche PAS ces arrays : améliorer la
-- normalisation/catalogue ne ré-indexe pas le BM25 (seul un changement de citation
-- le fait, par décision, garde IS DISTINCT FROM).
--
-- DDL/fonctions seulement (rapide). Le RECOMPUTE de tout le corpus est porté hors
-- migration (CLI `resync-legal-arrays`, lots autocommit) pour ne pas tenir une
-- transaction géante ; la garde IS DISTINCT FROM ne réécrit que les ~6,6 % qui
-- changent. À lancer juste après le déploiement du binaire (dual-write actif).

-- ── 1. Fonctions de sync : agrègent depuis cited_reference ───────────────────

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
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
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
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

-- ── 2. Trigger functions : sync des DEUX grains depuis decision_citation ──────
-- (decision_citation n'a pas d'UPDATE — clé = PK (decision_id, cited_reference_id)
-- — donc INSERT + DELETE suffisent. On corrige au passage l'absence de sync
-- incrémentale de l'array décision sous le 3NF : ici les deux grains sont synchronisés.)

CREATE OR REPLACE FUNCTION sync_citation_arrays_ins()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM new_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION sync_citation_arrays_del()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM old_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

-- ── 3. Bascule des triggers : 3NF (dlr) → decision_citation ──────────────────
-- Le 3NF reste écrit (dual-write, rollback trivial) mais ne pilote plus les arrays.

DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_ins ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_del ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_upd ON decision_legal_references;

CREATE TRIGGER dc_sync_arrays_ins
    AFTER INSERT ON decision_citation
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_ins();

CREATE TRIGGER dc_sync_arrays_del
    AFTER DELETE ON decision_citation
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_del();
