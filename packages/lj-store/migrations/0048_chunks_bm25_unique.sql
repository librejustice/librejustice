-- Migration 0048 — Index BM25 unique consolidé (ADR 0073).
--
-- ``chunks_bm25`` devient le SEUL index BM25 : il absorbe le titre
-- (``search_title`` dénormalisé sur le chunk 0 — un doc-titre par décision,
-- stats BM25 du champ non gonflées par le nombre de chunks) et le composite
-- ``instrument|article`` passe en fast field. Trois dettes soldées :
--
-- 1. La jambe titre filtrait sur ``decisions`` seul (aucune colonne legal_*) :
--    les filtres article/instrument FUYAIENT par cette jambe RRF. Servie par
--    ``chunks_bm25``, elle hérite du filtre chunk complet, fast fields compris.
-- 2. ``legal_article_composite`` en fast field ``pdb.literal`` : pushdown
--    in-index (le post-filtre heap de 0047 coûtait 1-3 s / ~930k buffers).
-- 3. ``legal_article_labels`` (colonne + fast field + GIN, plus lu depuis
--    0047) : supprimé.
--
-- Stopwords : ``body`` et ``search_title`` partagent la même config
-- (``French`` + ``["a", "à"]``, absents de la liste tantivy builtin). Mesuré
-- sur ce ParadeDB avant migration : le filtre stopwords PRÉSERVE les trous de
-- positions (``le ministère de la justice`` → ``ministère(1) … justice(4)``)
-- et ``paradedb.parse`` analyse les phrases avec le tokenizer du champ → une
-- phrase avec stopwords matche exactement comme avant ; en OR les stopwords
-- sont ignorés (plus de pollution BM25) ; stopword seul → 0 résultat sans
-- erreur. Cf. ADR 0073.
--
-- ⚠️ Fenêtre de maintenance : ``@@@`` est indisponible du DROP au CREATE
-- (lexical/hybrid en échec, ~30-60 min). Les DROP précèdent le backfill :
-- l'UPDATE de ~3 M de lignes tourne sans maintenance BM25/GIN (leçon de 0047,
-- où le GIN créé avant le backfill l'avait étiré à 75 min).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. Colonne titre sur les chunks (portée par le chunk 0 uniquement)
-- =====================================================================

ALTER TABLE decision_chunks ADD COLUMN IF NOT EXISTS search_title TEXT;

-- =====================================================================
-- 2. Triggers : titre dans la sync décisions → chunks ; labels retiré
--    de la sync des références légales
-- =====================================================================

-- ``search_title`` (GENERATED STORED sur decisions) se recalcule quand ses
-- sources changent (formation, docket, juridiction, date…) ; la garde WHEN
-- du trigger le compare directement — couvre des sources (formation_or_chamber,
-- docket_numbers) absentes des colonnes meta synchronisées.
CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW
WHEN (
       OLD.jurisdiction_level IS DISTINCT FROM NEW.jurisdiction_level
    OR OLD.instance_level     IS DISTINCT FROM NEW.instance_level
    OR OLD.main_outcome       IS DISTINCT FROM NEW.main_outcome
    OR OLD.special_procedure  IS DISTINCT FROM NEW.special_procedure
    OR OLD.jurisdiction_name  IS DISTINCT FROM NEW.jurisdiction_name
    OR OLD.publication_codes  IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title       IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

-- Sync des références légales (helper de 0038/0047) : instruments + composite,
-- sans ``legal_article_labels``.
CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
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
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

-- =====================================================================
-- 3. Coupure : drop des index BM25 + dette ``legal_article_labels``
-- =====================================================================

DROP INDEX IF EXISTS chunks_bm25;
DROP INDEX IF EXISTS decisions_title_bm25;
DROP INDEX IF EXISTS idx_chunks_legal_article_labels;
ALTER TABLE decision_chunks DROP COLUMN IF EXISTS legal_article_labels;

-- =====================================================================
-- 4. Backfill du titre sur les chunks 0 (~3 M lignes, hors index BM25/GIN)
-- =====================================================================

UPDATE decision_chunks c
SET search_title = d.search_title
FROM decisions d
WHERE d.id = c.decision_id
  AND c.chunk_index = 0
  AND c.search_title IS DISTINCT FROM d.search_title;

-- =====================================================================
-- 5. ``chunks_bm25`` v2 : body + titre + composite en fast field
-- =====================================================================

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "body":         {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

-- =====================================================================
-- 6. Stats planner à jour (leçon post-0047 : backfill massif sans ANALYZE
--    = plans heap catastrophiques + statement_timeout API)
-- =====================================================================

ANALYZE decision_chunks;
