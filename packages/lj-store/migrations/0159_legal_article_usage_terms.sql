-- ADR 0248 : champ usage_terms (signal d'usage des citations) sur
-- legal_article, indexé BM25. Colonne remplie par `lj-ingest usage-terms` ;
-- recréation one-shot de legal_article_bm25 pour embarquer le champ
-- (tokenizer avec `_` : les bigrammes des sacs sont des tokens joints).
ALTER TABLE legal_article ADD COLUMN usage_terms text;

DROP INDEX legal_article_bm25;
CREATE INDEX legal_article_bm25 ON legal_article USING bm25 (
  id, search_title, texte, num, usage_terms,
  ((source)::pdb.literal), ((text_uid)::pdb.literal), ((status)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"},
    "usage_terms":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
