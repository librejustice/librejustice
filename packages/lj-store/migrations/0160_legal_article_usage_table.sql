-- ADR 0248 (correctif de la 0159) : le champ usage_terms dans
-- legal_article_bm25 fausse la normalisation de longueur BM25 — Tantivy
-- calcule l'avgdl du champ sur TOUS les docs du segment (2,2 M, presque tous
-- vides) et écrase les gros sacs organiques. Le signal d'usage vit dans une
-- table dédiée avec son propre index (stats justes, le design validé au banc).
DROP INDEX legal_article_bm25;
ALTER TABLE legal_article DROP COLUMN usage_terms;
CREATE INDEX legal_article_bm25 ON legal_article USING bm25 (
  id, search_title, texte, num,
  ((source)::pdb.literal), ((text_uid)::pdb.literal), ((status)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }');

CREATE TABLE legal_article_usage (
    id       bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    num_key  text NOT NULL,
    terms    text NOT NULL,
    UNIQUE (text_uid, num_key)
);

CREATE INDEX legal_article_usage_bm25 ON legal_article_usage USING bm25 (
  id, terms
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
