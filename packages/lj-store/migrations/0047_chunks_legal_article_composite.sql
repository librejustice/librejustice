-- Migration 0047 — Clé d'article composite ``instrument|article`` (ADR 0071).
-- Variante ZÉRO DOWNTIME (pas de rebuild ``chunks_bm25``).
--
-- Contexte : le filtre ``legalArticle`` portait des libellés d'articles NUS
-- (``legal_article_labels`` = ``["1240", "L. 132-1"]``, migration 0038). Or un
-- même numéro existe dans plusieurs codes → ``c.legal_article_labels && %s``
-- matche l'article dans N'IMPORTE quel code, et le front coche le même numéro
-- sous tous les codes. On ajoute une clé composite ``instrument|article``
-- (``["Code civil|1240", …]``) : le filtre devient « article X DU code Y ».
--
-- Le séparateur ``|`` n'apparaît ni dans un nom de ``legal_codes`` ni dans un
-- libellé de ``legal_articles`` (vérifié sur la prod).
--
-- ZÉRO DOWNTIME : on NE rebuild PAS ``chunks_bm25`` (ce qui imposait une fenêtre
-- de maintenance en 0038/0046). ``legal_article_composite`` n'est PAS un fast
-- field — il est filtré par post-filtre heap, narrowé en amont par le fast field
-- EXISTANT ``legal_instruments`` (un composite implique son instrument, donc le
-- push d'instrument est un sur-ensemble logiquement redondant ; cf. search.rs).
-- ``@@@`` reste servi par ``chunks_bm25`` intact → /recherche ne tombe jamais.
-- Seul le ``CREATE INDEX`` GIN prend un verrou SHARE (bloque les écritures
-- d'ingest quelques minutes, PAS les lectures). On garde ``legal_article_labels``
-- maintenu et cohérent (coût trivial : le join dlr×la×lc est déjà fait) ; sa
-- suppression + celle de son fast field sont différées à un futur rebuild
-- ``chunks_bm25`` (bundlé avec un autre changement d'index, hors fenêtre dédiée).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. Colonne composite + index GIN (verrou SHARE : lectures OK)
-- =====================================================================

ALTER TABLE decision_chunks
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

CREATE INDEX IF NOT EXISTS idx_chunks_legal_article_composite
    ON decision_chunks USING GIN (legal_article_composite);

-- =====================================================================
-- 2. Backfill : instrument|article (jointure 3NF) — online (MVCC)
-- =====================================================================

UPDATE decision_chunks c
SET legal_article_composite = comp_agg.composite
FROM (
    SELECT
        dlr.decision_id,
        ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                  ORDER BY lc.name || '|' || la.label)
            FILTER (WHERE la.label IS NOT NULL) AS composite
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    GROUP BY dlr.decision_id
) comp_agg
WHERE c.decision_id = comp_agg.decision_id
  AND c.legal_article_composite IS DISTINCT FROM comp_agg.composite;

-- =====================================================================
-- 3. Trigger STATEMENT-level : instruments + labels (cohérence) + composite
-- =====================================================================
--
-- Enrichit le helper de 0038 : ``legal_article_composite`` s'ajoute aux deux
-- agrégats existants. ``legal_article_labels`` reste maintenu (cohérent jusqu'à
-- sa suppression différée). Coût équivalent (le scan dlr×la×lc est déjà fait).

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
                FILTER (WHERE la.label IS NOT NULL) AS labels_arr,
            ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                      ORDER BY lc.name || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments        = agg.instruments_arr,
        legal_article_labels     = agg.labels_arr,
        legal_article_composite  = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_labels    IS DISTINCT FROM agg.labels_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;
