-- Recrée l'index BM25 sur decisions.search_title avec :
--   - tokenizer regex `[\p{L}\p{N}-]+` (préserve les tirets, comme chunks_bm25)
--   - filtre stopwords français (cohérence indexation/query)
--   - ascii_folding=true (Cergy = cergy, Île-de-France = ile-de-france)
--
-- Motivation : la migration 0016 utilisait `pdb.simple` pour avoir les
-- stopwords FR, mais ce tokenizer split sur les tirets ⇒ "L 423-13"
-- devient ["L","423","13"] et "13" matche les titres avec "13 décembre/mars".
-- Conséquence observée : pour la query "article L 423-13 du code...",
-- le top-10 était dominé par des décisions "Asile - 15 jours, 13 février
-- 2024" sans rapport avec l'article 423-13.
--
-- Le format JSON `text_fields` accepte regex + stopwords_language au
-- niveau du champ — combinaison impossible via pdb.simple ou le cast
-- pdb.regex_pattern (qui n'expose que pattern).

DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "stopwords_language": "French", "record": "position"}}'
);
