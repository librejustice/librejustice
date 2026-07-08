-- Migration 0060 — Référentiel normatif multi-source (ADR 0097).
--
-- Supersede le schéma 0057 (tables `legi_codes`/`legi_articles`, identité
-- mono-source `legitext`/`legiarti`, pont `legal_codes.legitext`). Ces tables
-- sont VIDES en prod (aucune donnée 0057 ingérée) : on les restructure, pas de
-- backfill de données curated. DDL bon marché uniquement.
--
-- Modèle d'identité générique discriminé par `source` (#11 : un nom par concept
-- « texte/article de référentiel »). LEGI devient `source='legifrance'`. Charger
-- un nouvel ordre juridique (UE, traités, droit étranger) est additif : zéro DDL,
-- `(source, source_uid)` + `jurisdiction` les accueille.
--
-- Deux vérités-source distinctes (#11) : `legal_codes`/`legal_articles` (libellés
-- bruts cités par les juges, ADR 0079) ≠ `referential_*` (droit positif curated).
-- Le pont `legal_codes.referential_text_id` + la résolution matérialisée
-- `legal_article_resolution` les relient sans les fusionner (ADR 0097 §2).
--
-- Backfill réel (HORS-MIGRATION, par `resolve-refs` côté Rust) : `legal_articles
-- .num_key` (1,5 M lignes, `normalize_article(label)` — fonction pure Rust que
-- le SQL ne peut pas appeler) ; pont code ; résolution article. Sentinelles LEGI
-- (`2999-01-01`/`2222-02-22` → NULL ; vocabulaire `status`) déjà normalisées à la
-- frontière de parsing de la source (#12), jamais en aval.

-- ── DROP de l'ancien schéma 0057 ────────────────────────────────────────────
-- `legal_codes.legitext` FK `legi_codes(legitext)` : la déposer avant les DROP TABLE.
ALTER TABLE legal_codes DROP COLUMN IF EXISTS legitext;
DROP TABLE IF EXISTS legi_articles;
DROP TABLE IF EXISTS legi_codes;

-- ── Catalogue des textes/codes de référentiel (ex-legi_codes) ───────────────
-- `source_uid` = LEGITEXT/JORFTEXT (LEGI), CELEX (UE)… (clé d'idempotence #7).
-- `slug` = code court pour l'URL (jamais le source_uid brut), nullable.
-- `title_key` = normalize_instrument(title), écrit côté Rust à l'upsert ; sert le
-- match exact canonique déterministe en SQL (résolution code §3).
CREATE TABLE referential_texts (
    id            BIGSERIAL PRIMARY KEY,
    source        TEXT NOT NULL,              -- 'legifrance'|'eurlex'|'treaty'|'sn-legi'|…
    source_uid    TEXT NOT NULL,              -- LEGITEXT/JORFTEXT (LEGI), CELEX (UE)…
    jurisdiction  TEXT NOT NULL,              -- 'FR'|'EU'|'INTL'|'SN'|…
    slug          TEXT,                       -- code court pour l'URL (nullable)
    title         TEXT NOT NULL,
    title_key     TEXT NOT NULL,              -- normalize_instrument(title) (match exact)
    nature        TEXT NOT NULL,              -- CODE/LOI/REGLEMENT/DIRECTIVE/TRAITE/…
    last_modified DATE,
    UNIQUE (source, source_uid)
);

CREATE INDEX idx_referential_texts_slug ON referential_texts (slug) WHERE slug IS NOT NULL;
CREATE INDEX idx_referential_texts_title_key ON referential_texts (title_key);

-- Index BM25 ParadeDB sur `title` (fallback flou de la résolution code §3).
-- Mirroir exact du pattern interne (cf. 0016 `pdb.simple` stopwords FR +
-- ascii_folding, et 0021/0053 pour la forme `key_field`). `pdb.simple` retire les
-- mots-outils FR et folde l'ascii à l'indexation ET à la query (cohérence).
CREATE INDEX referential_texts_title_bm25 ON referential_texts
USING bm25 (id, (title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');

-- ── Articles de référentiel (ex-legi_articles) ──────────────────────────────
-- `source_uid` = LEGIARTI (LEGI), CELEX-article… (clé d'idempotence #7).
-- `(source, text_source_uid)` → `referential_texts(source, source_uid)` (PAS de
-- FK : un article peut précéder son texte parent dans un incrément, back-fill
-- `title_path` différé, ADR 0092 inchangé). `num_key` = normalize_article(num),
-- écrit côté Rust. `date_debut` NULLABLE = borne ouverte (sources non versionnées :
-- traités, conventions). `status` 'VIGUEUR' par défaut côté parser.
CREATE TABLE referential_articles (
    id               BIGSERIAL PRIMARY KEY,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL,           -- LEGIARTI (LEGI), CELEX-article… (idempotence #7)
    text_source_uid  TEXT NOT NULL,           -- (source, text_source_uid) → texte parent (pas de FK)
    num              TEXT NOT NULL,           -- "1240", "L. 822-1"
    num_key          TEXT NOT NULL,           -- normalize_article(num) (jointure)
    title_path       TEXT,                    -- fil d'Ariane (back-fillé)
    status           TEXT NOT NULL,           -- VIGUEUR/ABROGE/… ; 'VIGUEUR' par défaut
    date_debut       DATE,                    -- NULL = toujours en vigueur (borne ouverte)
    date_fin         DATE,                    -- NULL = pas de fin (sentinelle 2999→NULL)
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL,         -- xxh3-64 du bloc source brut (cast bit-à-bit i64)
    UNIQUE (source, source_uid)
);

-- Law-at-date : `(source, text_source_uid, num_key, date_debut)` borne la version.
CREATE INDEX idx_ref_articles_at_date
    ON referential_articles (source, text_source_uid, num_key, date_debut);
-- Version en vigueur (date absente ⇒ status='VIGUEUR'). Partiel.
CREATE INDEX idx_ref_articles_vigueur
    ON referential_articles (source, text_source_uid, num_key)
    WHERE status = 'VIGUEUR';

-- ── Pont code legal_codes ↔ referential_texts (recomputable, niveau code) ───
-- Remplace `legal_codes.legitext` de 0057, multi-source. Posé par
-- `bridge_legal_codes_referential` sur les formes canoniques.
ALTER TABLE legal_codes ADD COLUMN referential_text_id BIGINT REFERENCES referential_texts(id);

-- ── legal_articles.num_key (clé de jointure fine du pont, ADR 0097 §2) ──────
-- `normalize_article(label)`, écrite à l'upsert (frontière Rust) ; back-fill des
-- 1,5 M lignes existantes par `resolve-refs` (SQL ne peut pas appeler la fonction
-- pure Rust). NULL = pas encore résolu.
ALTER TABLE legal_articles ADD COLUMN num_key TEXT;
CREATE INDEX idx_legal_articles_num_key ON legal_articles (code_id, num_key);

-- ── Résolution citation→article matérialisée, validée par existence (§2) ────
-- Une ligne SEULEMENT si la citation pointe un article curated réel. Le rendu
-- décision sait alors, sans probe runtime, si afficher un lien `/loi/…` cliquable.
-- Recomputable (`rebuild_legal_article_resolution`).
CREATE TABLE legal_article_resolution (
    article_id          BIGINT PRIMARY KEY REFERENCES legal_articles(id) ON DELETE CASCADE,
    ref_source          TEXT NOT NULL,
    ref_text_source_uid TEXT NOT NULL,        -- referential_texts.source_uid résolu
    num_key             TEXT NOT NULL
);

-- Reverse « décisions citant l'article X » (exact, ADR 0097 §2).
CREATE INDEX idx_lar_reverse
    ON legal_article_resolution (ref_source, ref_text_source_uid, num_key);
