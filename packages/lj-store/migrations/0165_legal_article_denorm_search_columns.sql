-- Migration 0165 — Colonnes de recherche dénormalisées sur legal_article
-- (ADR 0254) : jurisdiction / nature (upper) / slug / searchable copiées de
-- legal_text pour rendre les jambes articles de /recherche-textes
-- single-table (prédicat 100 % indexé, éligible Top-K/columnar ParadeDB).
-- Rafraîchies en fin d'ingest par refresh_article_denorm (ex
-- refresh_article_code_titles), même contrat de fraîcheur que code_title.
--
-- ⚠️ Rebuild complet de legal_article_bm25 (ParadeDB n'autorise qu'UN index
-- bm25 par table : impossible d'ajouter des champs en place). Le DROP prend
-- un verrou exclusif tenu jusqu'au COMMIT : la recherche d'articles est
-- indisponible pendant le backfill + rebuild (~5-10 min) — à jouer en creux.

SET LOCAL statement_timeout = 0;

ALTER TABLE legal_article
    ADD COLUMN jurisdiction text,
    ADD COLUMN nature text,
    ADD COLUMN slug text,
    ADD COLUMN searchable boolean;

DROP INDEX legal_article_bm25;

-- Backfill (2,2 M lignes). searchable reproduit exactement le prédicat des
-- jambes articles : t.slug IS NOT NULL AND role visible (ADR 0246 §6).
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

-- Même définition que l'index vivant (tokenizers/record inchangés : les
-- champs texte gardent leur sémantique de scoring) + les quatre colonnes
-- dénormalisées en champs filtrables/fast.
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
