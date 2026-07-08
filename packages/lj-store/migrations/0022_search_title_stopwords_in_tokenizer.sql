-- Correctif de 0021 : la clé `stopwords_language` doit être à l'intérieur
-- de l'objet `tokenizer` (à côté de `pattern` et `ascii_folding`), pas au
-- niveau du champ. Avec la position champ-niveau, ParadeDB ignore
-- silencieusement la directive ⇒ "l", "le", "de"… restaient indexés.
--
-- Empiriquement : avant ce correctif, `paradedb.match('search_title', 'l')`
-- matchait 23 décisions (titres "L'Hermine", "L'OCEANE"…). Avec la clé
-- déplacée dans `tokenizer`, 0 match — comportement attendu.

DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French"}, "record": "position"}}'
);
