-- Migration 0041 — Publication : codes bruts multi-valeur par ordre (ADR 0054).
--
-- Remplace les colonnes générées cassées ``is_recueil`` / ``is_tables_lebon``
-- (migration 0009, ``LIKE '%A%'`` sensible à la casse → 117 852 arrêts judiciaires
-- « Publié au bulletin » classés à tort « Inédit ») par un ``publication_codes
-- TEXT[]`` fidèle. La facette/portée se dérivent des codes en requête
-- (``= ANY`` / ``term_set``), plus aucune dérivation en base.
--
-- Harmonise aussi ``docket_numbers`` en ``NOT NULL DEFAULT '{}'`` (un array de
-- facette n'a pas de raison d'être NULL ; un NULL casse ``= ANY`` / overlap).
--
-- ⚠️ Fenêtre de maintenance OBLIGATOIRE : ce script DROP + CREATE ``chunks_bm25``
-- (≈ plusieurs minutes sur le corpus). Pendant le build, /search en mode lexical
-- ou hybrid échoue. À programmer off-peak. Rollback : recréer ``chunks_bm25`` avec
-- ``is_recueil`` / ``is_tables_lebon`` (état migration 0038).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

-- =====================================================================
-- 1. decisions : publication_codes TEXT[] (fidèle) + backfill
-- =====================================================================

ALTER TABLE decisions
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

-- Backfill depuis le scalaire historique (déjà tronqué au 1er code à l'ingestion ;
-- le multi-valeur ne se récupère qu'au ré-ingest, hors scope de cette migration).
UPDATE decisions
SET publication_codes = ARRAY[publication_code]
WHERE publication_code IS NOT NULL AND publication_code <> '';

-- =====================================================================
-- 2. Drop trigger + colonnes générées cassées + scalaire
-- =====================================================================
--
-- Le trigger ``decisions_sync_chunks_meta`` référence is_recueil/is_tables_lebon
-- (corps + clause WHEN) : on le DROP avant de retirer les colonnes, on le recrée
-- en §5 sur publication_codes.

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;

-- DROP des colonnes générées (l'index idx_decisions_is_recueil tombe en cascade).
ALTER TABLE decisions
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon,
    DROP COLUMN publication_code;

-- =====================================================================
-- 3. docket_numbers : NOT NULL DEFAULT '{}'
-- =====================================================================

UPDATE decisions SET docket_numbers = '{}' WHERE docket_numbers IS NULL;
ALTER TABLE decisions
    ALTER COLUMN docket_numbers SET DEFAULT '{}',
    ALTER COLUMN docket_numbers SET NOT NULL;

-- =====================================================================
-- 4. decision_chunks : publication_codes dénormalisé + drop booléens
-- =====================================================================

ALTER TABLE decision_chunks
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

UPDATE decision_chunks c
SET publication_codes = d.publication_codes
FROM decisions d
WHERE c.decision_id = d.id
  AND c.publication_codes IS DISTINCT FROM d.publication_codes;

-- Index booléens + colonnes (cascade des index idx_chunks_is_*).
ALTER TABLE decision_chunks
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon;

-- =====================================================================
-- 5. Trigger de propagation decisions → decision_chunks (sur publication_codes)
-- =====================================================================

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta() RETURNS TRIGGER AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

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
    )
    EXECUTE FUNCTION sync_chunks_decision_meta();

-- =====================================================================
-- 6. Rebuild chunks_bm25 : publication_codes en fast field (remplace booléens)
-- =====================================================================
--
-- ParadeDB ne permet pas d'altérer les fast fields d'un index BM25 existant.
-- Rebuild complet requis. ``chunks_vec`` n'est pas concerné.

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
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
