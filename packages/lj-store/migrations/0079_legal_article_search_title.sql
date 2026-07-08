-- Migration 0079 — Titre formé par article + index BM25 titre-primaire (ADR 0114).
--
-- La recherche dans les textes se fait *principalement par le titre* : nom du
-- code + numéro d'article (« code civil 1242 », « code de commerce L145-16-2 »).
-- Or un article n'a pas de titre stocké, et le titre du code vit dans
-- `legal_text` (autre table). On forme donc un titre par article, à la manière
-- de `decisions.search_title` (0015) : colonne générée composée.
--
-- Le titre du code ne peut pas être joint dans une colonne générée (même-ligne
-- seulement) → on le dénormalise dans `code_title`, maintenu par le backfill
-- ci-dessous + une passe de refresh en fin d'ingest référentiel (LEGI streame
-- articles et codes séparément, l'article n'a pas le titre du code au parse).
-- `search_title` = `code_title, num [, division feuille]` (PAS le mot « article » :
-- il serait dans les 3 M lignes → posting-list géante d'un terme qui ne
-- discrimine rien).
--
-- L'index `legal_article_bm25` est recréé en titre-primaire, **sans positions**
-- (la recherche utilise `paradedb.match` = OR de termes, qui n'exploite pas les
-- positions ; les enregistrer ne ferait que gonfler l'index) :
--   * `search_title` : jambe primaire, `record: freq` ;
--   * `texte`        : jambe secondaire, `record: freq` (les snippets sont
--     recalculés en RAM hors index inversé, cf. `lj-api/snippets.rs` → aucun
--     besoin de positions ; l'index du corps fond vs l'ancien `position`, 0078) ;
--   * `num`          : conservé (filtre/boost numéro exact, `basic`).
-- Tokenizer regex `[\p{L}\p{N}-]+` + ascii_folding + stopwords FR, comme 0078.
-- Pas de stemmer : il s'applique avant l'ascii_folding et casse les requêtes
-- sans accent (« responsabilite » ≠ « responsabilité ») sans livrer un folding
-- morphologique fiable — net négatif mesuré (cf. working-notes). Le rappel
-- morphologique passera par la table d'alias/thésaurus, pas le tokenizer.
--
-- ⚠️ ADD COLUMN générée STORED + backfill (3 M lignes) + rebuild BM25 réécrivent
-- la table sous ACCESS EXCLUSIVE (les pages /loi lisent `legal_article`) :
-- appliquer en fenêtre de maintenance.

-- DROP l'index BM25 d'abord : les ADD COLUMN ci-dessous réécrivent la table
-- (générée STORED) et reconstruiraient l'ancien index pour rien avant qu'on le
-- remplace. Sans lui, les réécritures ne maintiennent que les petits btree, puis
-- on bâtit le nouvel index une seule fois.
DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS code_title TEXT;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

-- Backfill du titre de code (recalcule search_title des lignes touchées).
UPDATE legal_article a
SET code_title = t.title
FROM legal_text t
WHERE a.text_uid = t.text_uid
  AND a.code_title IS DISTINCT FROM t.title;

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX IF NOT EXISTS legal_article_bm25 ON legal_article
USING bm25 (
    id,
    search_title,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal)
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
