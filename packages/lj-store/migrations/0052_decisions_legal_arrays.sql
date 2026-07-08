-- Migration 0052 — Arrays légaux dénormalisés au grain décision (ADR 0084).
--
-- ``legal_instruments`` / ``legal_article_composite`` (données par-décision,
-- aujourd'hui répliquées sur chaque chunk pour le filtre ``chunks_bm25``)
-- montent sur ``decisions`` : elles alimentent les fast fields de
-- ``decisions_bm25`` (jambes BM25 body + titre au grain décision, migration
-- 0053). Les colonnes chunk homonymes restent en place — la jambe ANN filtre
-- toujours au scan chunk (VectorChord exige le filtre local, pas de join).
-- Résolution canonique COALESCE identique à la sync chunk (ADR 0079, 0049).
--
-- Le backfill est set-based et tourne AVANT le build de ``decisions_bm25``
-- (0053) : l'UPDATE massif n'a aucune maintenance d'index BM25 à porter
-- (leçon 0047/0048). ⚠️ N'appliquer qu'une fois le backfill ``full_text``
-- terminé (étape 1 ADR 0084) : l'UPDATE touche ``decisions`` et entrerait
-- sinon en concurrence de verrous avec la boucle de backfill.

-- =====================================================================
-- 1. Colonnes arrays légaux au grain décision
-- =====================================================================

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS legal_instruments       TEXT[],
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

-- =====================================================================
-- 2. Sync incrémentale au grain décision (réutilisée par
--    consolidate-legal-refs / reextract-fields)
-- =====================================================================

-- Même agrégation que ``_sync_chunks_legal_instruments_for`` (0048/0049),
-- mais une seule ligne par décision (pas de fanout chunk). COALESCE canonique.
CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name)
                      ORDER BY COALESCE(canon.name, lc.name))
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name) || '|' || la.label
                      ORDER BY COALESCE(canon.name, lc.name) || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        LEFT JOIN legal_codes    canon          ON canon.id = lc.canonical_id
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

-- =====================================================================
-- 3. Backfill par lots d'id (mémoire bornée par lot, ~3 M lignes)
-- =====================================================================

-- Un backfill set-based d'un seul tenant (CTE agrégeant les ~3 M décisions
-- jointes à decision_legal_references) matérialise un hash-agg + tri par groupe
-- qui dépasse la RAM du serveur (24 Go) → OOM/swap saturé. On borne la mémoire
-- en appelant la sync par fenêtres d'id : chaque appel n'agrège que les refs de
-- sa fenêtre. Même pattern « keyset par lot » que les backfills lj-ingest
-- (backfill_text_fields). Le tout reste dans la transaction de migration
-- (atomique) — seule la mémoire PAR STATEMENT est bornée, pas la transaction.
DO $$
DECLARE
    b     bigint := 0;
    maxid bigint;
    step  bigint := 20000;
BEGIN
    SELECT max(id) INTO maxid FROM decisions;
    WHILE maxid IS NOT NULL AND b < maxid LOOP
        PERFORM _sync_decisions_legal_instruments_for(
            ARRAY(SELECT id FROM decisions WHERE id > b AND id <= b + step)
        );
        b := b + step;
    END LOOP;
END $$;

ANALYZE decisions;
