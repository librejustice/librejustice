-- Migration 0046 — jurisdiction_name en fast field ``pdb.literal`` exact (ADR 0070).
--
-- Depuis la migration 0026, ``jurisdiction_name`` est le SEUL champ de
-- ``chunks_bm25`` indexé en ``pdb.literal_normalized`` (lowercase implicite). Le
-- pushdown auto de ParadeDB n'équivaut un prédicat de colonne (``c.col = ANY``)
-- au fast field QUE si le terme indexé == la valeur heap brute — vrai pour
-- ``pdb.literal`` (casse préservée), FAUX pour ``literal_normalized`` (terme
-- lowercasé ≠ valeur heap). Conséquence mesurée sur pg_search 0.23.5 : le facet
-- juridiction tombe en ``heap_filter`` (≈ 9,6 s cold sur la jambe BM25 body,
-- ~460k buffers heap lus), tandis que tous les autres facets (``literal``)
-- poussent dans l'index (term/term_set, < 1 s). Cf. ADR 0070.
--
-- La normalisation n'apportait rien : l'extraction produit déjà une forme
-- canonique (mesuré : 175 valeurs distinctes brutes == 175 en lowercase, zéro
-- collision de casse). Le facet renvoie la valeur stockée exacte → le match
-- exact ``literal`` suffit, comme pour tous les autres facets.
--
-- ParadeDB ne permet pas d'altérer les fast fields d'un index BM25 existant :
-- rebuild complet. ``chunks_vec`` n'est pas concerné.
--
-- ⚠️ Fenêtre de maintenance OBLIGATOIRE : DROP + CREATE ``chunks_bm25`` (15 Go,
-- ≈ plusieurs minutes). Pendant le build, /search en mode lexical/hybrid échoue
-- (l'opérateur ``@@@`` requiert l'index). À programmer off-peak. Rollback :
-- recréer avec ``(jurisdiction_name::pdb.literal_normalized)`` (état 0041).

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

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
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
