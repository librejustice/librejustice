-- 0164_decisions_jurisdiction_code_date_idx.sql
CREATE INDEX idx_decisions_jur_code_date
    ON decisions (jurisdiction_code, date_lecture DESC);

-- 0165_legal_article_denorm_search_columns.sql
SET LOCAL statement_timeout = 0;

ALTER TABLE legal_article
    ADD COLUMN jurisdiction text,
    ADD COLUMN nature text,
    ADD COLUMN slug text,
    ADD COLUMN searchable boolean;

DROP INDEX legal_article_bm25;

UPDATE legal_article a SET
    jurisdiction = t.jurisdiction,
    nature = upper(t.nature),
    slug = t.slug,
    searchable = (t.slug IS NOT NULL
                  AND t.role NOT IN ('individuel', 'vehicule', 'habilitation'))
FROM legal_text t
WHERE t.text_uid = a.text_uid;

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX legal_article_bm25 ON legal_article
USING bm25 (
    id,
    search_title,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal),
    (jurisdiction::pdb.literal),
    (nature::pdb.literal),
    (slug::pdb.literal),
    searchable
)
WITH (
  key_field = 'id',
  text_fields = '{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

ANALYZE legal_article;

-- 0166_legal_text_norm_browse_index.sql
CREATE INDEX idx_legal_text_norm_fond_browse ON legal_text (
  (CASE
    WHEN nature ILIKE 'code%' OR upper(nature) IN ('CODE', 'CONSTITUTION', 'ETAT_CIVIL') THEN 'codes'
    WHEN jurisdiction = 'UE' OR upper(nature) IN ('DIRECTIVE', 'DIRECTIVE_EURO', 'REGLEMENTEUROPEEN', 'REGLEMENT_EURO', 'DECISION_EURO', 'AVISEURO', 'ARRETEURO', 'ARRETEEURO', 'INSTRUCTIONEURO', 'DELIBERATIONEURO', 'DECLARATIONEURO', 'LETTREEURO') THEN 'textes-ue'
    WHEN jurisdiction = 'INTL' OR upper(nature) IN ('TRAITE', 'TI', 'PROTOCOLE') THEN 'traites'
    WHEN upper(nature) IN ('LOI', 'LOI_ORGANIQUE', 'LOI_CONSTIT', 'LOI_PROGRAMME', 'DECRET_LOI', 'ORDONNANCE') THEN 'lois'
    WHEN upper(nature) = 'DECRET' THEN 'decrets'
    WHEN upper(nature) = 'ARRETE' THEN 'arretes'
    WHEN upper(nature) IN ('IDCC', 'AVENANT', 'ACCORD', 'ACCORD_FONCTION_PUBLIQUE') THEN 'conventions-collectives'
    WHEN upper(nature) IN ('CIRCULAIRE', 'INSTRUCTION') THEN 'circulaires'
    WHEN upper(nature) = 'BOFIP' THEN 'bofip'
    ELSE 'autres'
  END),
  (CASE
    WHEN date_texte >= DATE '1500-01-01' AND date_texte < DATE '2100-01-01' THEN date_texte
    WHEN date_publi >= DATE '1500-01-01' AND date_publi < DATE '2100-01-01' THEN date_publi
  END) DESC,
  id
) WHERE slug IS NOT NULL AND role <> 'individuel';

