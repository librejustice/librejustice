-- Migration 0053 — Index BM25 au grain décision (ADR 0084).
--
-- ``decisions_bm25`` indexe ``full_text`` (texte entier de la décision) +
-- ``search_title`` + facettes/arrays légaux en fast fields. Les jambes BM25
-- body et titre l'interrogent directement au grain décision
-- (``LIMIT leg_limit``, plus de sur-récupération ni de pooling — supersede la
-- jambe BM25 chunk de l'ADR 0080). Mêmes tokenizers que ``chunks_bm25`` (0048) :
-- regex ``[\p{L}\p{N}-]+`` + ascii folding + stopwords FR ``["a", "à"]``.
-- ``record: position`` requis pour les requêtes de phrase (``paradedb.parse``).
--
-- ⚠️ PRÉREQUIS : appliquer APRÈS le backfill ``full_text`` (étape 1 ADR 0084)
-- et la migration 0052 (arrays légaux). Sinon l'index ne couvre que les lignes
-- déjà peuplées (les UPDATE de backfill restants l'alimenteraient au fil de
-- l'eau, mais le build d'un coup post-backfill est nettement plus rapide).
--
-- ⚠️ Fenêtre de maintenance : ``chunks_bm25`` reste en service ; le cutover des
-- jambes vers ``decisions_bm25`` se fait côté code (étape 3 ADR 0084), pas ici.
-- Cet index s'ajoute sans rien supprimer (+~11 GB) ; le DROP de ``chunks_bm25``
-- vient à l'étape 4.

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
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
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

ANALYZE decisions;
