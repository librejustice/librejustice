-- Annule uniquement la migration 0016 :
-- supprime search_title de decision_chunks, les triggers et fonctions associés,
-- et reconstruit chunks_bm25 dans son état d'origine (body seul).
-- La colonne decisions.search_title et l'index decisions_title_bm25 (0015) sont conservés.

DROP TRIGGER IF EXISTS trg_decisions_propagate_search_title ON decisions;
DROP TRIGGER IF EXISTS trg_chunks_inherit_search_title ON decision_chunks;
DROP FUNCTION IF EXISTS _sync_chunk_search_title_from_decision();
DROP FUNCTION IF EXISTS _set_chunk_search_title();

ALTER TABLE decision_chunks DROP COLUMN IF EXISTS search_title;

DROP INDEX IF EXISTS chunks_bm25;
CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (id, body, juridiction_type)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
