-- Migration 0142 — `jurisdiction_type` : un concept, un nom (ADR 0210).
--
-- Renomme la colonne FR `juridiction_type` en `jurisdiction_type` sur ses
-- trois porteurs (`decisions`, `decision_chunks`, `jurisdiction`), aligné sur
-- le reste du schéma déjà anglais (`jurisdiction_name`, `jurisdiction_code`,
-- `jurisdiction_level`…).
--
-- ⚠️ `decisions_bm25` DOIT être recréé dans la même transaction : le schéma
-- Tantivy fige le nom de champ à la création de l'index — après le RENAME,
-- tout INSERT/UPDATE sur `decisions` échoue (« Error getting tokenizer for
-- field: juridiction_type », vérifié sur pg_search 0.23.5) et le pushdown du
-- filtre retombe en heap_filter. Pas de variante CONCURRENTLY possible ici :
-- le nom de champ suit la colonne au CREATE, donc l'index neuf ne peut pas
-- précéder le rename. Rebuild (~11 GB, 3 M décisions) sous lock
-- AccessExclusive sur `decisions` — même classe de fenêtre que 0081.

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decisions RENAME COLUMN juridiction_type TO jurisdiction_type;
ALTER TABLE decision_chunks RENAME COLUMN juridiction_type TO jurisdiction_type;
ALTER TABLE jurisdiction RENAME COLUMN juridiction_type TO jurisdiction_type;

DROP INDEX decisions_bm25;

CREATE INDEX decisions_bm25 ON decisions USING bm25 (id, full_text, search_title, ((jurisdiction_type)::pdb.literal), ((legal_instruments)::pdb.literal), ((legal_article_composite)::pdb.literal), ((publication_codes)::pdb.literal), date_lecture) WITH (key_field=id, text_fields='{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }');

ANALYZE decisions;
