-- Compléter la liste snowball FR de tantivy : elle contient "à" mais pas
-- "a" (verbe avoir 3p sg). Conséquence avant ce fix : titres CAA "5ème
-- chambre A - formation à 3" gardent un token "a" indexé et matchent
-- toute query contenant "a" — y compris notre query du 2026-05-10
-- ("…ARS… a rejeté sa…") qui rapatriait 100 décisions Lyon non pertinentes.
--
-- ParadeDB applique la liste `stopwords` AVANT ascii_folding et
-- `stopwords_language` APRÈS — vérifié via paradedb.tokenize. On exploite
-- les deux passes : preset FR pour le snowball complet, custom pour
-- attraper "a" et "à" (cette dernière car snowball-après-fold ne voit
-- plus que "a" qui n'y est pas).
--
-- Body (chunks_bm25) volontairement intouché : on a besoin des stopwords
-- pour la phrase scoring sur les chunks (cf. signals + paradedb.parse).

DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}}'
);
