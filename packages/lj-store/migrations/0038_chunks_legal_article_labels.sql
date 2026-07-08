-- Migration 0038 — Dénormalisation ``legal_article_labels`` sur
-- ``decision_chunks`` (phase 5.4 du plan ``fuzz-search-2026-05``).
--
-- Contexte : avant cette migration, le filtre ``legalArticle`` côté
-- ``/search`` était implémenté via ``EXISTS (SELECT 1 FROM
-- decision_legal_references dlr JOIN legal_articles la … WHERE … AND
-- la.label ILIKE %s)``. Sur le fuzz pass 1 et 7, ce path mettait 10-60
-- secondes pour des filtres simples (50 articles × N décisions × ILIKE
-- non indexé). DoS trivial.
--
-- Solution structurelle : dénormaliser les **labels** d'articles cités
-- directement sur ``decision_chunks`` dans un TEXT[] flat, mirror exact
-- de ``legal_instruments`` (qui contient les **codes**, granularité
-- supérieure — cf. amend ADR 0033). On peut alors filtrer en
-- ``c.legal_article_labels && %s::text[]`` (GIN overlap, indexé) —
-- même path que les instruments.
--
-- ⚠️ Fenêtre de maintenance OBLIGATOIRE : ce script DROP + CREATE
-- ``chunks_bm25``. Pendant la durée du build (≈ plusieurs minutes selon
-- corpus + mémoire), /search avec mode lexical ou hybrid retourne soit
-- une erreur Postgres ("operator does not exist: bigint @@@ …"), soit
-- un seq scan timeout. À programmer en off-peak. Plan de rollback :
-- recréer l'index ``chunks_bm25`` sans ``legal_article_labels``
-- (revert du CREATE ci-dessous, retour à la migration 0026 §6).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. Colonne dénormalisée + index GIN
-- =====================================================================

ALTER TABLE decision_chunks
    ADD COLUMN IF NOT EXISTS legal_article_labels TEXT[];

CREATE INDEX IF NOT EXISTS idx_chunks_legal_article_labels
    ON decision_chunks USING GIN (legal_article_labels);

-- =====================================================================
-- 2. Backfill
-- =====================================================================

UPDATE decision_chunks c
SET legal_article_labels = labels_agg.labels
FROM (
    SELECT
        dlr.decision_id,
        ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
            FILTER (WHERE la.label IS NOT NULL) AS labels
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    GROUP BY dlr.decision_id
) labels_agg
WHERE c.decision_id = labels_agg.decision_id
  AND c.legal_article_labels IS DISTINCT FROM labels_agg.labels;

-- =====================================================================
-- 3. Trigger STATEMENT-level commun aux deux agrégats
-- =====================================================================
--
-- On enrichit le helper de 0029 : un seul UPDATE par batch propage
-- conjointement ``legal_instruments`` ET ``legal_article_labels``.
-- Coût équivalent à 0029 (le scan ``dlr × la × lc`` est de toute façon
-- fait pour ``legal_instruments``), gain : pas de second trigger à
-- chaîner.

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
                FILTER (WHERE la.label IS NOT NULL) AS labels_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments     = agg.instruments_arr,
        legal_article_labels  = agg.labels_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments    IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_labels IS DISTINCT FROM agg.labels_arr
      );
$$ LANGUAGE sql;

-- =====================================================================
-- 4. Rebuild ``chunks_bm25`` avec le nouveau fast field
-- =====================================================================
--
-- ParadeDB ne permet pas d'ajouter un fast field sur un index BM25
-- existant. Rebuild complet requis. ``chunks_vec`` n'est pas concerné.

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal_normalized),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    is_recueil,
    is_tables_lebon,
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
