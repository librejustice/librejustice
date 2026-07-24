-- Migration 0120 — purge de l'ancien monde titres (ADR 0170 ét.7).
--
-- `formation_or_chamber` (chaîne aplatie greffe) et `jurisdiction_name`
-- (libellé texte libre) ne sont plus ni écrites ni lues : les axes
-- structurés (`chamber_position` + uids) et le référentiel `jurisdiction`
-- (label guéri via `jurisdiction_code`) les remplacent partout — titres
-- composés par `lj_core::titles`, affichage via les référentiels.
--
-- `decisions_bm25` portait `jurisdiction_name` en fast field (inutilisé par
-- les requêtes : les filtres passent par `jurisdiction_code` côté SQL). Le
-- swap est GARDÉ : en prod l'index a été reconstruit en amont par
-- `CREATE INDEX CONCURRENTLY` (zéro downtime, procédure ParadeDB) et le DO
-- ne fait rien ; sur un replay fresh (table vide) il reconstruit ici même.

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- 1. Trigger de dénorm chunks : `jurisdiction_name` sort de la projection.
CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger LANGUAGE plpgsql AS $fn$
BEGIN
    UPDATE decision_chunks SET
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$fn$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW WHEN (
       OLD.publication_codes IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture      IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title      IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

-- 2. Dénorm chunks : colonne + btree.
DROP INDEX IF EXISTS idx_chunks_jurisdiction_name;
ALTER TABLE decision_chunks DROP COLUMN IF EXISTS jurisdiction_name;

-- 3. decisions_bm25 sans `jurisdiction_name` (swap gardé, cf. en-tête).
DO $do$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_indexes
        WHERE indexname = 'decisions_bm25' AND indexdef LIKE '%jurisdiction_name%'
    ) THEN
        EXECUTE 'DROP INDEX decisions_bm25';
        EXECUTE $ix$
CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((juridiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }')
        $ix$;
    END IF;
END;
$do$;

-- 4. Colonnes decisions.
ALTER TABLE decisions
    DROP COLUMN IF EXISTS jurisdiction_name,
    DROP COLUMN IF EXISTS formation_or_chamber;

ANALYZE decisions;
ANALYZE decision_chunks;
